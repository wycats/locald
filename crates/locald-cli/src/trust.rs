use crate::style;
use anyhow::{Context, Result};

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

    let ensure = locald_utils::cert::ensure_root_ca()?;
    let ca_cert_path = ensure.paths.cert_path;

    if ensure.created {
        println!("Generating new Root CA...");
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
    let ensure = locald_utils::cert::ensure_root_ca()?;
    install_ca(&ensure.paths.cert_path)?;

    Ok(())
}

fn install_ca(cert_path: &std::path::Path) -> Result<()> {
    // On macOS, ca_injector is not supported — use the native security CLI directly.
    #[cfg(target_os = "macos")]
    return install_ca_macos(cert_path);

    #[cfg(not(target_os = "macos"))]
    {
        let path_str = cert_path.to_str().context("Invalid path string")?;

        if let Err(e) = ca_injector::install_ca(path_str) {
            let msg = e.to_string();
            if msg.contains("Permission denied") || msg.contains("os error 13") {
                anyhow::bail!(
                    "Permission denied. Please run `locald admin setup` to configure HTTPS trust."
                );
            }

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

#[cfg(target_os = "macos")]
fn install_ca_macos(cert_path: &std::path::Path) -> Result<()> {
    use security_framework::certificate::SecCertificate;
    use security_framework::trust_settings::{Domain, TrustSettings};

    let pem_data = std::fs::read(cert_path)
        .with_context(|| format!("Failed to read certificate at {}", cert_path.display()))?;

    let parsed = pem::parse(&pem_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse PEM certificate: {e}"))?;

    let cert = SecCertificate::from_der(parsed.contents())
        .map_err(|e| anyhow::anyhow!("Failed to create SecCertificate from DER: {e}"))?;

    // Add to the default (login) keychain.
    // This may produce a duplicate-item error if already present — that's fine.
    match cert.add_to_keychain(None) {
        Ok(()) => {}
        Err(e) if e.code() == -25299 => {
            // errSecDuplicateItem — cert already in keychain, continue to trust settings
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to add certificate to login keychain: {e}. \
                 You may need to open Keychain Access and add it manually."
            ));
        }
    }

    // Mark as trusted for all uses.
    // Use Admin domain when running as root (e.g. via `sudo locald admin setup`),
    // because the User domain requires the user's GUI session context which isn't
    // available under sudo. Admin domain trust is system-wide and persists.
    let domain = if nix::unistd::geteuid().is_root() {
        Domain::Admin
    } else {
        Domain::User
    };

    TrustSettings::new(domain)
        .set_trust_settings_always(&cert)
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to set trust settings ({domain:?}): {e}. \
             If running via sudo, ensure you're in a desktop session."
            )
        })?;

    Ok(())
}
