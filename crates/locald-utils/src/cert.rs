use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::crypto::ring::sign;
use rustls::pki_types::PrivateKeyDer;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

/// Manages TLS certificates for locald.
///
/// Generates and caches certificates on the fly for requested domains, signed by the locald CA.
pub struct CertManager {
    issuer: CertifiedIssuer<'static, KeyPair>,
    cache: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl fmt::Debug for CertManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertManager")
            .field("issuer", &"CertifiedIssuer(...)")
            .field("cache", &self.cache)
            .finish()
    }
}

impl CertManager {
    /// Creates a new `CertManager`.
    ///
    /// Loads the root CA key and certificate from the locald certificates directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the root CA files are missing or cannot be read/parsed.
    pub async fn new() -> Result<Self> {
        let certs_dir = get_certs_dir()?;
        let ca_cert_path = certs_dir.join("rootCA.pem");
        let ca_key_path = certs_dir.join("rootCA-key.pem");

        tokio::fs::create_dir_all(&certs_dir)
            .await
            .context("Failed to create locald certs directory")?;

        let cert_exists = ca_cert_path.exists();
        let key_exists = ca_key_path.exists();

        if cert_exists != key_exists {
            anyhow::bail!(
                "Root CA is partially configured (rootCA.pem/rootCA-key.pem mismatch). Run `locald admin setup` to repair HTTPS setup."
            );
        }

        if !cert_exists && !key_exists {
            // Generate a new CA on the fly so HTTPS can function immediately.
            // The user can then run `locald admin setup` to install it into the system trust store.
            let (cert_pem, key_pem) =
                tokio::task::spawn_blocking(|| -> Result<(String, String)> {
                    let mut params = CertificateParams::default();
                    let mut dn = DistinguishedName::new();
                    dn.push(DnType::CommonName, "locald Development CA");
                    dn.push(DnType::OrganizationName, "locald");
                    params.distinguished_name = dn;
                    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
                    params.key_usages =
                        vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

                    let key_pair = KeyPair::generate()?;
                    let cert = params.self_signed(&key_pair)?;
                    Ok((cert.pem(), key_pair.serialize_pem()))
                })
                .await
                .context("CA generation task panicked")??;

            // Write files atomically-ish (best effort). If this fails, surface the error.
            tokio::fs::write(&ca_cert_path, cert_pem)
                .await
                .with_context(|| format!("Failed to write {}", ca_cert_path.display()))?;
            tokio::fs::write(&ca_key_path, key_pem)
                .await
                .with_context(|| format!("Failed to write {}", ca_key_path.display()))?;

            info!(
                "Generated locald Root CA at {} (run `locald admin setup` to install into system trust store)",
                ca_cert_path.display()
            );
        }

        // Use tokio::fs for reading the key
        let ca_key_pem = tokio::fs::read_to_string(&ca_key_path)
            .await
            .context("Failed to read rootCA-key.pem")?;

        // Offload CPU-intensive key parsing and issuer creation
        let issuer = tokio::task::spawn_blocking(move || {
            let ca_key =
                KeyPair::from_pem(&ca_key_pem).context("Failed to parse rootCA-key.pem")?;

            // Reconstruct CA params to match `locald trust`
            let mut ca_params = CertificateParams::default();
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "locald Development CA");
            dn.push(DnType::OrganizationName, "locald");
            ca_params.distinguished_name = dn;
            ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
            ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

            CertifiedIssuer::self_signed(ca_params, ca_key)
                .context("Failed to create CertifiedIssuer")
        })
        .await??;

        Ok(Self {
            issuer,
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn generate_cert(&self, domain: &str) -> Result<Arc<CertifiedKey>> {
        info!("Generating certificate for {}", domain);
        let mut params = CertificateParams::new(vec![domain.to_string()])?;
        params.subject_alt_names = vec![SanType::DnsName(domain.to_string().try_into()?)];

        // Generate a new key pair for this certificate
        let key_pair = KeyPair::generate()?;

        // Sign the certificate with our CA
        let cert = params.signed_by(&key_pair, &self.issuer)?;

        let cert_der = cert.der();
        let private_key_der = key_pair.serialize_der();

        let private_key = PrivateKeyDer::Pkcs8(private_key_der.into());
        let cert_chain = vec![cert_der.clone()];

        let signing_key = sign::any_supported_type(&private_key)
            .map_err(|_| anyhow::anyhow!("Failed to create signing key"))?;

        Ok(Arc::new(CertifiedKey::new(cert_chain, signing_key)))
    }
}

impl ResolvesServerCert for CertManager {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let sni = client_hello.server_name()?;

        // Check cache first
        {
            match self.cache.lock() {
                Ok(cache) => {
                    if let Some(cert) = cache.get(sni) {
                        return Some(cert.clone());
                    }
                }
                Err(e) => {
                    error!("CertManager cache lock poisoned: {}", e);
                    return None;
                }
            }
        }

        // Generate new cert
        // Use block_in_place to avoid stalling the async reactor during heavy CPU ops
        let cert_res = tokio::task::block_in_place(|| self.generate_cert(sni));

        match cert_res {
            Ok(cert) => match self.cache.lock() {
                Ok(mut cache) => {
                    cache.insert(sni.to_string(), cert.clone());
                    Some(cert)
                }
                Err(e) => {
                    error!("CertManager cache lock poisoned: {}", e);
                    None
                }
            },
            Err(e) => {
                error!("Failed to generate certificate for {}: {}", sni, e);
                None
            }
        }
    }
}

/// Returns the directory where locald certificates are stored.
///
/// # Errors
///
/// Returns an error if the user's home directory cannot be determined.
pub fn get_certs_dir() -> Result<PathBuf> {
    let home = directories::UserDirs::new().context("Could not find home directory")?;
    Ok(home.home_dir().join(".locald").join("certs"))
}
