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
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

/// Paths to the locald Root CA certificate and key files.
#[derive(Debug, Clone)]
pub struct RootCaPaths {
    /// Path to the Root CA certificate (PEM).
    pub cert_path: PathBuf,
    /// Path to the Root CA private key (PEM).
    pub key_path: PathBuf,
}

/// Result of ensuring the Root CA exists on disk.
#[derive(Debug, Clone)]
pub struct EnsureRootCaResult {
    /// Paths to the Root CA certificate and key.
    pub paths: RootCaPaths,
    /// Whether the Root CA was created by this call.
    pub created: bool,
}

fn root_ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "locald Development CA");
    dn.push(DnType::OrganizationName, "locald");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params
}

fn generate_root_ca_pem() -> Result<(String, String)> {
    let params = root_ca_params();
    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

/// Ensure a Root CA exists in the given directory, creating it if needed.
///
/// Returns the CA paths and whether the CA was created during this call.
///
/// # Errors
///
/// Returns an error if the certs directory cannot be created, if the CA
/// is partially configured, or if writing the CA files fails.
#[allow(clippy::disallowed_methods)]
pub fn ensure_root_ca_in_dir(certs_dir: &Path) -> Result<EnsureRootCaResult> {
    std::fs::create_dir_all(certs_dir).context("Failed to create locald certs directory")?;

    let ca_cert_path = certs_dir.join("rootCA.pem");
    let ca_key_path = certs_dir.join("rootCA-key.pem");

    let cert_exists = ca_cert_path.exists();
    let key_exists = ca_key_path.exists();

    if cert_exists != key_exists {
        anyhow::bail!(
            "Root CA is partially configured (rootCA.pem/rootCA-key.pem mismatch). Run `locald admin setup` to repair HTTPS setup."
        );
    }

    if cert_exists {
        return Ok(EnsureRootCaResult {
            paths: RootCaPaths {
                cert_path: ca_cert_path,
                key_path: ca_key_path,
            },
            created: false,
        });
    }

    let (cert_pem, key_pem) = generate_root_ca_pem()?;
    std::fs::write(&ca_cert_path, cert_pem)
        .with_context(|| format!("Failed to write {}", ca_cert_path.display()))?;
    std::fs::write(&ca_key_path, key_pem)
        .with_context(|| format!("Failed to write {}", ca_key_path.display()))?;

    Ok(EnsureRootCaResult {
        paths: RootCaPaths {
            cert_path: ca_cert_path,
            key_path: ca_key_path,
        },
        created: true,
    })
}

/// Ensure a Root CA exists in the default locald certs directory.
///
/// Returns the CA paths and whether the CA was created during this call.
///
/// # Errors
///
/// Returns an error if the user's home directory cannot be determined or if
/// CA creation fails.
pub fn ensure_root_ca() -> Result<EnsureRootCaResult> {
    let certs_dir = get_certs_dir()?;
    ensure_root_ca_in_dir(&certs_dir)
}

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
        let ensure = tokio::task::spawn_blocking({
            let certs_dir = certs_dir.clone();
            move || ensure_root_ca_in_dir(&certs_dir)
        })
        .await
        .context("Root CA ensure task panicked")??;

        if ensure.created {
            info!(
                "Generated locald Root CA at {} (run `locald admin setup` to install into system trust store)",
                ensure.paths.cert_path.display()
            );
        }

        // Use tokio::fs for reading the key
        let ca_key_pem = tokio::fs::read_to_string(&ensure.paths.key_path)
            .await
            .context("Failed to read rootCA-key.pem")?;

        // Offload CPU-intensive key parsing and issuer creation
        let issuer = tokio::task::spawn_blocking(move || {
            let ca_key =
                KeyPair::from_pem(&ca_key_pem).context("Failed to parse rootCA-key.pem")?;

            // Reconstruct CA params to match the stored CA
            let ca_params = root_ca_params();

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
    // When invoked via pkexec/sudo, prefer the invoking user's home directory rather than /root.
    // This keeps the CA location consistent between the privileged setup path and the unprivileged
    // daemon/server path.
    #[cfg(unix)]
    {
        if let Ok(pkexec_uid) = std::env::var("PKEXEC_UID")
            && let Ok(uid) = pkexec_uid.parse::<u32>()
        {
            let uid = nix::unistd::Uid::from_raw(uid);
            if let Ok(Some(user)) = nix::unistd::User::from_uid(uid) {
                return Ok(user.dir.join(".locald").join("certs"));
            }
        }

        if let Ok(sudo_user) = std::env::var("SUDO_USER")
            && let Ok(Some(user)) = nix::unistd::User::from_name(&sudo_user)
        {
            return Ok(user.dir.join(".locald").join("certs"));
        }
    }

    let home = directories::UserDirs::new().context("Could not find home directory")?;
    Ok(home.home_dir().join(".locald").join("certs"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use temp_env::with_vars;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn unique_temp_dir() -> PathBuf {
        let base = std::env::temp_dir();
        base.join(format!("locald-utils-cert-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn ensure_root_ca_creates_both_files() {
        let dir = unique_temp_dir();
        let ensure = ensure_root_ca_in_dir(&dir).expect("ensure_root_ca_in_dir should succeed");
        assert!(ensure.created);
        assert!(ensure.paths.cert_path.exists());
        assert!(ensure.paths.key_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_root_ca_errors_on_partial_state() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rootCA.pem"), "dummy").unwrap();

        let err = ensure_root_ca_in_dir(&dir).unwrap_err();
        assert!(err.to_string().contains("partially configured"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_certs_dir_respects_pkexec_uid_and_sudo_user() {
        let _guard = ENV_LOCK.lock().unwrap();

        let uid = nix::unistd::getuid();
        let user = nix::unistd::User::from_uid(uid).unwrap().unwrap();

        // PKEXEC_UID
        with_vars(
            [
                ("SUDO_USER", None),
                ("PKEXEC_UID", Some(uid.as_raw().to_string())),
            ],
            || {
                let dir = get_certs_dir().unwrap();
                assert_eq!(dir, user.dir.join(".locald").join("certs"));
            },
        );

        // SUDO_USER
        with_vars(
            [("PKEXEC_UID", None), ("SUDO_USER", Some(user.name.clone()))],
            || {
                let dir = get_certs_dir().unwrap();
                assert_eq!(dir, user.dir.join(".locald").join("certs"));
            },
        );
    }
}
