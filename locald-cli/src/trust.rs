use crate::style;
use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, IsCa, KeyPair, KeyUsagePurpose,
};
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub fn run() -> Result<()> {
    // If we're not root and a privileged shim exists, delegate the entire operation to it.
    #[cfg(unix)]
    if !nix::unistd::geteuid().is_root() {
        if let Ok(Some(shim_path)) = locald_utils::shim::find_privileged() {
            if std::env::var("LOCALD_SHIM_ACTIVE").is_err() {
                let err = std::process::Command::new(&shim_path)
                    .arg("admin")
                    .arg("trust")
                    .exec();
                anyhow::bail!("Failed to exec shim for trust install: {err}");
            }
        }
    }

    let certs_dir = get_certs_dir()?;
    std::fs::create_dir_all(&certs_dir).context("Failed to create certs directory")?;

    let ca_cert_path = certs_dir.join("rootCA.pem");
    let ca_key_path = certs_dir.join("rootCA-key.pem");

    if !ca_cert_path.exists() || !ca_key_path.exists() {
        println!("Generating new Root CA...");
        generate_ca(&ca_cert_path, &ca_key_path)?;
        println!(
            "{} Root CA generated at {}",
            style::CHECK,
            ca_cert_path.display()
        );
    } else {
        println!("Root CA already exists at {}", ca_cert_path.display());
    }

    println!("Installing Root CA to system trust store...");
    install_ca(&ca_cert_path)?;
    println!("{} Root CA installed successfully.", style::CHECK);
    println!(
        "{} You may need to restart your browser to pick up trust changes.",
        style::WARN
    );

    Ok(())
}

/// Ensure a locald Root CA exists and install it into the system trust store.
///
/// This function performs no user-facing printing so callers can control output.
pub fn install_root_ca_into_trust_store() -> Result<()> {
    let certs_dir = get_certs_dir()?;
    std::fs::create_dir_all(&certs_dir).context("Failed to create certs directory")?;

    let ca_cert_path = certs_dir.join("rootCA.pem");
    let ca_key_path = certs_dir.join("rootCA-key.pem");

    if !ca_cert_path.exists() || !ca_key_path.exists() {
        generate_ca(&ca_cert_path, &ca_key_path)?;
    }

    install_ca(&ca_cert_path)?;

    Ok(())
}

fn get_certs_dir() -> Result<PathBuf> {
    // Under sudo we still want to write the CA into the invoking user's home directory,
    // not /root. When not under sudo, fall back to the current user's home.
    #[cfg(unix)]
    {
        if let Ok(sudo_user) = std::env::var("SUDO_USER") {
            if let Ok(Some(user)) = nix::unistd::User::from_name(&sudo_user) {
                return Ok(user.dir.join(".locald").join("certs"));
            }
        }
    }

    let home = home::home_dir().context("Could not find home directory")?;
    Ok(home.join(".locald").join("certs"))
}

fn generate_ca(cert_path: &std::path::Path, key_path: &std::path::Path) -> Result<()> {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "locald Development CA");
    dn.push(rcgen::DnType::OrganizationName, "locald");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    std::fs::write(cert_path, cert.pem())?;
    std::fs::write(key_path, key_pair.serialize_pem())?;

    Ok(())
}

fn install_ca(cert_path: &std::path::Path) -> Result<()> {
    let path_str = cert_path.to_str().context("Invalid path string")?;

    if let Err(e) = ca_injector::install_ca(path_str) {
        let msg = e.to_string();
        if msg.contains("Permission denied") || msg.contains("os error 13") {
            anyhow::bail!(
                "Permission denied. Please run `locald admin setup` to configure HTTPS trust."
            );
        }

        // Some platforms (or minimal installs) may not be recognized by ca_injector.
        // Provide a Linux fallback using common trust-store tools.
        #[cfg(target_os = "linux")]
        {
            if msg.contains("cannot find binary path") {
                return install_ca_linux_fallback(cert_path)
                    .with_context(|| format!("ca_injector failed: {msg}"));
            }
        }

        return Err(e).context("Failed to install CA certificate");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_ca_linux_fallback(cert_path: &std::path::Path) -> Result<()> {
    use std::process::Command;

    // Fedora/RHEL family
    let anchors_dir = std::path::Path::new("/etc/pki/ca-trust/source/anchors");
    if anchors_dir.exists() {
        let target = anchors_dir.join("locald-rootCA.pem");
        std::fs::copy(cert_path, &target)
            .with_context(|| format!("Failed to copy CA to {} (need root?)", target.display()))?;

        let status = Command::new("update-ca-trust")
            .arg("extract")
            .status()
            .context("Failed to execute update-ca-trust extract")?;
        if !status.success() {
            anyhow::bail!("update-ca-trust extract failed with status: {status}");
        }
        return Ok(());
    }

    // Debian/Ubuntu family
    let debian_dir = std::path::Path::new("/usr/local/share/ca-certificates");
    if debian_dir.exists() {
        let target = debian_dir.join("locald-rootCA.crt");
        std::fs::copy(cert_path, &target)
            .with_context(|| format!("Failed to copy CA to {} (need root?)", target.display()))?;

        let status = Command::new("update-ca-certificates")
            .status()
            .context("Failed to execute update-ca-certificates")?;
        if !status.success() {
            anyhow::bail!("update-ca-certificates failed with status: {status}");
        }
        return Ok(());
    }

    anyhow::bail!(
        "No known Linux trust-store directories found; install p11-kit-trust / update-ca-trust or equivalent"
    );
}
