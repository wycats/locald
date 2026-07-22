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
#[cfg(target_os = "macos")]
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};
use tracing::{debug, error, info};

type ServerNameAuthorizer = dyn Fn(&str) -> Option<String> + Send + Sync;

#[cfg(target_os = "macos")]
static ROOT_CA_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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

/// Ensure a macOS Root CA pair is valid and owned by the configured console user.
///
/// Existing valid material is retained while directory and file permissions are
/// repaired. Partial, malformed, or mismatched material is never overwritten.
///
/// # Errors
///
/// Returns an error when the directory or pair is unsafe or invalid, or when
/// ownership, permissions, or durable publication cannot be established.
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods, clippy::similar_names)]
pub fn repair_root_ca_in_dir(
    certs_dir: &Path,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<EnsureRootCaResult> {
    ensure_real_directory(certs_dir)?;

    let ca_cert_path = certs_dir.join("rootCA.pem");
    let ca_key_path = certs_dir.join("rootCA-key.pem");
    let cert_metadata = regular_file_metadata_if_present(&ca_cert_path)?;
    let key_metadata = regular_file_metadata_if_present(&ca_key_path)?;

    if cert_metadata.is_some() != key_metadata.is_some() {
        anyhow::bail!(
            "Root CA is partially configured; preserving {} and {} for explicit recovery",
            ca_cert_path.display(),
            ca_key_path.display()
        );
    }

    let created = if cert_metadata.is_some() {
        validate_root_ca_pair(&ca_cert_path, &ca_key_path)?;
        false
    } else {
        let (cert_pem, key_pem) = generate_root_ca_pem()?;
        publish_new_root_ca_pair(
            certs_dir,
            &ca_cert_path,
            cert_pem.as_bytes(),
            &ca_key_path,
            key_pem.as_bytes(),
            owner_uid,
            owner_gid,
        )?;
        true
    };

    set_owner_and_mode(certs_dir, owner_uid, owner_gid, 0o700)?;
    set_owner_and_mode(&ca_key_path, owner_uid, owner_gid, 0o600)?;
    set_owner_and_mode(&ca_cert_path, owner_uid, owner_gid, 0o644)?;

    Ok(EnsureRootCaResult {
        paths: RootCaPaths {
            cert_path: ca_cert_path,
            key_path: ca_key_path,
        },
        created,
    })
}

/// Validate an existing macOS Root CA pair without changing it.
///
/// # Errors
///
/// Returns an error when either file is missing, unsafe, malformed, or the
/// certificate and private key do not match.
#[cfg(target_os = "macos")]
pub fn validate_root_ca_material_in_dir(certs_dir: &Path) -> Result<RootCaPaths> {
    let ca_cert_path = certs_dir.join("rootCA.pem");
    let ca_key_path = certs_dir.join("rootCA-key.pem");
    let cert_metadata = regular_file_metadata_if_present(&ca_cert_path)?;
    let key_metadata = regular_file_metadata_if_present(&ca_key_path)?;
    if cert_metadata.is_none() || key_metadata.is_none() {
        anyhow::bail!("Root CA certificate and private key are both required");
    }
    validate_root_ca_pair(&ca_cert_path, &ca_key_path)?;
    Ok(RootCaPaths {
        cert_path: ca_cert_path,
        key_path: ca_key_path,
    })
}

/// Validate exact macOS ownership and permissions for existing Root CA state.
///
/// # Errors
///
/// Returns an error unless the directory is `0700`, the private key is `0600`,
/// the certificate is `0644`, and all three belong to the configured user.
#[cfg(target_os = "macos")]
#[allow(clippy::similar_names)]
pub fn validate_root_ca_permissions_in_dir(
    certs_dir: &Path,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<()> {
    validate_owner_and_mode(certs_dir, owner_uid, owner_gid, 0o700)?;
    validate_owner_and_mode(
        &certs_dir.join("rootCA-key.pem"),
        owner_uid,
        owner_gid,
        0o600,
    )?;
    validate_owner_and_mode(&certs_dir.join("rootCA.pem"), owner_uid, owner_gid, 0o644)
}

#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
fn ensure_real_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "Root CA directory is not a real directory: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
fn regular_file_metadata_if_present(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!("Root CA path is not a regular file: {}", path.display())
        }
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to inspect Root CA path {}", path.display()))
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
fn validate_root_ca_pair(cert_path: &Path, key_path: &Path) -> Result<()> {
    let cert_pem = std::fs::read_to_string(cert_path)
        .with_context(|| format!("Failed to read {}", cert_path.display()))?;
    let key_pem = std::fs::read_to_string(key_path)
        .with_context(|| format!("Failed to read {}", key_path.display()))?;
    let key_pair = KeyPair::from_pem(&key_pem)
        .with_context(|| format!("Root CA private key is malformed: {}", key_path.display()))?;
    let cert = pem::parse(cert_pem.as_bytes())
        .with_context(|| format!("Root CA certificate is malformed: {}", cert_path.display()))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.contents())
        .map_err(|error| anyhow::anyhow!("Root CA certificate is malformed: {error}"))?;
    if !parsed.tbs_certificate.is_ca()
        || parsed.subject() != parsed.issuer()
        || !parsed.validity().is_valid()
        || parsed
            .subject()
            .iter_common_name()
            .next()
            .and_then(|name| name.as_str().ok())
            != Some("locald Development CA")
    {
        anyhow::bail!("Root CA certificate does not match locald's CA profile");
    }
    let certificate_key = parsed.tbs_certificate.subject_pki.subject_public_key.data;
    if certificate_key.as_ref() != key_pair.public_key_raw() {
        anyhow::bail!(
            "Root CA certificate and private key do not match; preserving both files for explicit recovery"
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(clippy::similar_names)]
fn publish_new_root_ca_pair(
    directory: &Path,
    cert_path: &Path,
    cert_bytes: &[u8],
    key_path: &Path,
    key_bytes: &[u8],
    owner_uid: u32,
    owner_gid: u32,
) -> Result<()> {
    let cert_temp = write_root_ca_temp(cert_path, cert_bytes, 0o644, owner_uid, owner_gid)?;
    let key_temp = match write_root_ca_temp(key_path, key_bytes, 0o600, owner_uid, owner_gid) {
        Ok(path) => path,
        Err(error) => {
            drop(std::fs::remove_file(&cert_temp));
            return Err(error);
        }
    };

    let result = (|| -> Result<()> {
        std::fs::rename(&key_temp, key_path)
            .with_context(|| format!("Failed to publish {}", key_path.display()))?;
        File::open(directory)?.sync_all()?;
        std::fs::rename(&cert_temp, cert_path)
            .with_context(|| format!("Failed to publish {}", cert_path.display()))?;
        File::open(directory)?.sync_all()?;
        Ok(())
    })();

    if result.is_err() {
        drop(std::fs::remove_file(&cert_temp));
        drop(std::fs::remove_file(&key_temp));
    }
    result
}

#[cfg(target_os = "macos")]
#[allow(clippy::similar_names)]
fn write_root_ca_temp(
    destination: &Path,
    bytes: &[u8],
    mode: u32,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Root CA path has no parent: {}", destination.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Root CA path has no file name"))?;
    let sequence = ROOT_CA_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.tmp.{}.{}",
        name.to_string_lossy(),
        std::process::id(),
        sequence
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("Failed to create {}", temporary.display()))?;
    file.write_all(bytes)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    nix::unistd::fchown(
        &file,
        Some(nix::unistd::Uid::from_raw(owner_uid)),
        Some(nix::unistd::Gid::from_raw(owner_gid)),
    )?;
    file.sync_all()?;
    Ok(temporary)
}

#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
fn set_owner_and_mode(path: &Path, uid: u32, gid: u32, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    nix::unistd::chown(
        path,
        Some(nix::unistd::Uid::from_raw(uid)),
        Some(nix::unistd::Gid::from_raw(gid)),
    )?;
    let metadata = std::fs::metadata(path)?;
    if metadata.uid() != uid || metadata.gid() != gid || metadata.mode() & 0o7777 != mode {
        anyhow::bail!("Failed to establish owner/mode on {}", path.display());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_owner_and_mode(path: &Path, uid: u32, gid: u32, mode: u32) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("Path must not be a symlink: {}", path.display());
    }
    let actual_mode = metadata.mode() & 0o7777;
    if metadata.uid() != uid || metadata.gid() != gid || actual_mode != mode {
        anyhow::bail!(
            "Expected {} to be owned by {uid}:{gid} with mode {mode:04o}; found {}:{} with mode {actual_mode:04o}",
            path.display(),
            metadata.uid(),
            metadata.gid()
        );
    }
    Ok(())
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
/// Generates and caches locald-CA-signed certificates only for server names
/// accepted by the required authorization policy.
pub struct CertManager {
    issuer: CertifiedIssuer<'static, KeyPair>,
    ca_cert_der: rustls::pki_types::CertificateDer<'static>,
    cache: Mutex<HashMap<String, Arc<CertifiedKey>>>,
    authorize_server_name: Arc<ServerNameAuthorizer>,
}

impl fmt::Debug for CertManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertManager")
            .field("issuer", &"CertifiedIssuer(...)")
            .field("ca_cert_der", &"CertificateDer(...)")
            .field("cache", &self.cache)
            .field("authorize_server_name", &"ServerNameAuthorizer(...)")
            .finish()
    }
}

impl CertManager {
    /// Creates a new `CertManager`.
    ///
    /// Loads the root CA key and certificate from the locald certificates directory.
    /// The authorizer returns the canonical owned server name, or `None` to
    /// reject the TLS handshake. It is invoked before every cache lookup.
    ///
    /// # Errors
    ///
    /// Returns an error if the root CA files are missing or cannot be read/parsed.
    pub async fn new<F>(authorize_server_name: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        let certs_dir = get_certs_dir()?;
        Self::new_in_dir(&certs_dir, authorize_server_name).await
    }

    async fn new_in_dir<F>(certs_dir: &Path, authorize_server_name: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        let ensure = tokio::task::spawn_blocking({
            let certs_dir = certs_dir.to_path_buf();
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

        // Read both the CA key and cert
        let ca_key_pem = tokio::fs::read_to_string(&ensure.paths.key_path)
            .await
            .context("Failed to read rootCA-key.pem")?;
        let ca_cert_pem = tokio::fs::read_to_string(&ensure.paths.cert_path)
            .await
            .context("Failed to read rootCA.pem")?;

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

        // Parse the CA cert DER for inclusion in TLS cert chains.
        // Note: pem::parse() returns only the first PEM block. This is correct
        // for locald's single-block CA cert files.
        let ca_cert_der = tokio::task::spawn_blocking(move || {
            let pem_parsed = pem::parse(ca_cert_pem.as_bytes())
                .map_err(|e| anyhow::anyhow!("Failed to parse CA cert PEM: {e}"))?;
            Ok::<_, anyhow::Error>(rustls::pki_types::CertificateDer::from(
                pem_parsed.into_contents(),
            ))
        })
        .await??;

        Ok(Self {
            issuer,
            ca_cert_der,
            cache: Mutex::new(HashMap::new()),
            authorize_server_name: Arc::new(authorize_server_name),
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
        // Include the CA cert in the chain so browsers can verify the full trust path.
        let cert_chain = vec![cert_der.clone(), self.ca_cert_der.clone()];

        let signing_key = sign::any_supported_type(&private_key)
            .map_err(|_| anyhow::anyhow!("Failed to create signing key"))?;

        Ok(Arc::new(CertifiedKey::new(cert_chain, signing_key)))
    }

    fn resolve_server_name(&self, requested_server_name: &str) -> Option<Arc<CertifiedKey>> {
        let Some(server_name) = (self.authorize_server_name)(requested_server_name) else {
            debug!("Rejecting certificate request for unowned SNI {requested_server_name}");
            return None;
        };

        // Authorization must happen before every cache lookup so removing a
        // claim immediately prevents reuse of a previously generated leaf.
        {
            match self.cache.lock() {
                Ok(cache) => {
                    if let Some(cert) = cache.get(&server_name) {
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
        let cert_res = tokio::task::block_in_place(|| self.generate_cert(&server_name));

        match cert_res {
            Ok(cert) => match self.cache.lock() {
                Ok(mut cache) => {
                    cache.insert(server_name, cert.clone());
                    Some(cert)
                }
                Err(e) => {
                    error!("CertManager cache lock poisoned: {}", e);
                    None
                }
            },
            Err(e) => {
                error!(
                    "Failed to generate certificate for {}: {}",
                    requested_server_name, e
                );
                None
            }
        }
    }
}

impl ResolvesServerCert for CertManager {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        self.resolve_server_name(client_hello.server_name()?)
    }
}

/// Returns the directory where locald certificates are stored.
///
/// On macOS: `~/Library/Application Support/locald/certs/`
/// On Linux: `~/.locald/certs/`
///
/// # Errors
///
/// Returns an error if the user's home directory cannot be determined.
pub fn get_certs_dir() -> Result<PathBuf> {
    // When invoked via pkexec/sudo, prefer the invoking user's home directory rather than /root.
    // This keeps the CA location consistent between the privileged setup path and the unprivileged
    // daemon/server path.
    #[cfg(target_os = "linux")]
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

    Ok(locald_data_dir()?.join("certs"))
}

/// Check whether the locald Root CA is trusted by the system.
///
/// On macOS, evaluates the certificate with Security.framework Trust Settings.
/// Returns `true` if the CA file exists and is trusted, `false` otherwise.
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
pub fn is_ca_trusted() -> bool {
    let Ok(certs_dir) = get_certs_dir() else {
        return false;
    };
    is_ca_path_trusted(&certs_dir.join("rootCA.pem"))
}

/// Evaluate whether one CA certificate is trusted through macOS Trust Settings.
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
pub fn is_ca_path_trusted(ca_path: &Path) -> bool {
    use security_framework::certificate::SecCertificate;
    use security_framework::policy::SecPolicy;
    use security_framework::secure_transport::SslProtocolSide;
    use security_framework::trust::SecTrust;

    const TEST_DOMAIN: &str = "locald-doctor.localhost";
    let Ok(ca_pem_bytes) = std::fs::read(ca_path) else {
        return false;
    };
    let Ok(ca_pem) = pem::parse(ca_pem_bytes) else {
        return false;
    };
    let Some(ca_key_path) = ca_path.parent().map(|parent| parent.join("rootCA-key.pem")) else {
        return false;
    };
    let Ok(ca_key_pem) = std::fs::read_to_string(ca_key_path) else {
        return false;
    };
    let Ok(ca_key) = KeyPair::from_pem(&ca_key_pem) else {
        return false;
    };
    let Ok(issuer) = CertifiedIssuer::self_signed(root_ca_params(), ca_key) else {
        return false;
    };
    let Ok(leaf_key) = KeyPair::generate() else {
        return false;
    };
    let Ok(leaf_params) = CertificateParams::new(vec![TEST_DOMAIN.to_string()]) else {
        return false;
    };
    let Ok(leaf) = leaf_params.signed_by(&leaf_key, &issuer) else {
        return false;
    };
    let Ok(leaf) = SecCertificate::from_der(leaf.der().as_ref()) else {
        return false;
    };
    let Ok(ca) = SecCertificate::from_der(ca_pem.contents()) else {
        return false;
    };
    let policy = SecPolicy::create_ssl(SslProtocolSide::SERVER, Some(TEST_DOMAIN));
    let Ok(mut trust) = SecTrust::create_with_certificates(&[leaf, ca], &[policy]) else {
        return false;
    };
    if trust.set_network_fetch_allowed(false).is_err() {
        return false;
    }
    trust.evaluate_with_error().is_ok()
}

/// Returns the platform-appropriate data directory for locald.
///
/// On macOS: `~/Library/Application Support/locald/`
/// On Linux/other: `~/.locald/`
///
/// When running under `sudo`, resolves the invoking user's home directory
/// rather than root's, so that extracted binaries and certificates end up
/// in the correct user's data directory.
///
/// # Errors
///
/// Returns an error if the home directory cannot be determined.
pub fn locald_data_dir() -> Result<PathBuf> {
    // Under sudo, resolve the real user's home.
    #[cfg(unix)]
    if nix::unistd::geteuid().is_root()
        && let Ok(sudo_user) = std::env::var("SUDO_USER")
        && let Ok(Some(user)) = nix::unistd::User::from_name(&sudo_user)
    {
        let home = user.dir;
        #[cfg(target_os = "macos")]
        {
            return Ok(home
                .join("Library")
                .join("Application Support")
                .join("locald"));
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Ok(home.join(".locald"));
        }
    }

    let home = dirs::home_dir().context("Could not find home directory")?;

    #[cfg(target_os = "macos")]
    {
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("locald"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(home.join(".locald"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    #[cfg(target_os = "linux")]
    use temp_env::with_vars;
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    #[cfg(target_os = "linux")]
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn unique_temp_dir() -> PathBuf {
        let base = std::env::temp_dir();
        base.join(format!("locald-utils-cert-test-{}", uuid::Uuid::new_v4()))
    }

    async fn tls_handshake_results(manager: Arc<CertManager>, server_name: &str) -> (bool, bool) {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(manager.ca_cert_der.clone())
            .expect("trust test root CA");
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let server_config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_cert_resolver(manager),
        );
        let server_name = rustls::pki_types::ServerName::try_from(server_name.to_owned())
            .expect("valid test server name");
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let acceptor = TlsAcceptor::from(server_config);
        let connector = TlsConnector::from(client_config);

        let (server_result, client_result) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                acceptor.accept(server_io),
                connector.connect(server_name, client_io)
            )
        })
        .await
        .expect("TLS handshake completes");

        (server_result.is_ok(), client_result.is_ok())
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
    #[cfg(target_os = "macos")]
    fn macos_repair_retains_valid_material_and_repairs_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = unique_temp_dir();
        let first = repair_root_ca_in_dir(
            &dir,
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw(),
        )
        .expect("create macOS Root CA");
        let cert_before = std::fs::read(&first.paths.cert_path).expect("read certificate");
        let key_before = std::fs::read(&first.paths.key_path).expect("read private key");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("loosen directory");
        std::fs::set_permissions(
            &first.paths.key_path,
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("loosen private key");

        let repaired = repair_root_ca_in_dir(
            &dir,
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw(),
        )
        .expect("repair macOS Root CA");

        assert!(!repaired.created);
        assert_eq!(
            std::fs::read(&repaired.paths.cert_path).unwrap(),
            cert_before
        );
        assert_eq!(std::fs::read(&repaired.paths.key_path).unwrap(), key_before);
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&repaired.paths.key_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&repaired.paths.cert_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o644
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_repair_preserves_partial_material() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create Root CA directory");
        let evidence = b"partial-certificate-evidence";
        std::fs::write(dir.join("rootCA.pem"), evidence).expect("write partial evidence");

        let error = repair_root_ca_in_dir(
            &dir,
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw(),
        )
        .expect_err("partial Root CA must fail closed");

        assert!(error.to_string().contains("partially configured"));
        assert_eq!(std::fs::read(dir.join("rootCA.pem")).unwrap(), evidence);
        assert!(!dir.join("rootCA-key.pem").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_repair_preserves_mismatched_material() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create Root CA directory");
        let (cert_pem, _) = generate_root_ca_pem().expect("generate certificate pair");
        let (_, other_key_pem) = generate_root_ca_pem().expect("generate other certificate pair");
        std::fs::write(dir.join("rootCA.pem"), &cert_pem).expect("write certificate");
        std::fs::write(dir.join("rootCA-key.pem"), &other_key_pem).expect("write mismatched key");

        let error = repair_root_ca_in_dir(
            &dir,
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw(),
        )
        .expect_err("mismatched Root CA must fail closed");

        assert!(error.to_string().contains("do not match"));
        assert_eq!(
            std::fs::read_to_string(dir.join("rootCA.pem")).unwrap(),
            cert_pem
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("rootCA-key.pem")).unwrap(),
            other_key_pem
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tls_handshake_requires_current_authorization_before_cache_reuse() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = unique_temp_dir();
        let authorized = Arc::new(Mutex::new(BTreeSet::from(["owned.localhost".to_owned()])));
        let policy = authorized.clone();
        let manager = Arc::new(
            CertManager::new_in_dir(&dir, move |requested| {
                let canonical = requested
                    .strip_suffix('.')
                    .unwrap_or(requested)
                    .to_ascii_lowercase();
                let is_authorized = policy.lock().ok()?.contains(&canonical);
                is_authorized.then_some(canonical)
            })
            .await
            .expect("create certificate manager"),
        );

        assert!(manager.resolve_server_name("OWNED.LOCALHOST.").is_some());
        assert_eq!(
            manager
                .cache
                .lock()
                .expect("certificate cache lock")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["owned.localhost".to_owned()]
        );
        assert_eq!(
            tls_handshake_results(manager.clone(), "owned.localhost").await,
            (true, true)
        );

        authorized.lock().expect("authorization set lock").clear();
        assert!(manager.resolve_server_name("owned.localhost").is_none());
        assert_eq!(
            tls_handshake_results(manager.clone(), "owned.localhost").await,
            (false, false)
        );
        assert_eq!(
            tls_handshake_results(manager.clone(), "telemetry.vercel.com").await,
            (false, false)
        );
        assert_eq!(
            manager
                .cache
                .lock()
                .expect("certificate cache lock")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["owned.localhost".to_owned()]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
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
