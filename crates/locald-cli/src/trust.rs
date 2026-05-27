use crate::error::{CliError, CliResult};
use crate::style;
use anyhow::Context;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub fn run() -> CliResult<()> {
    // If we're not root and a privileged shim exists, delegate the entire operation to it.
    #[cfg(unix)]
    if !nix::unistd::geteuid().is_root() {
        if let Ok(Some(shim_path)) = locald_utils::shim::find_privileged() {
            if std::env::var("LOCALD_SHIM_ACTIVE").is_err() {
                let err = std::process::Command::new(&shim_path)
                    .arg("admin")
                    .arg("trust")
                    .exec();
                return Err(CliError::message(format!(
                    "Failed to exec shim for trust install: {err}"
                )));
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
pub fn install_root_ca_into_trust_store() -> CliResult<()> {
    let ensure = locald_utils::cert::ensure_root_ca()?;
    install_ca(&ensure.paths.cert_path)?;

    Ok(())
}

fn install_ca(cert_path: &std::path::Path) -> CliResult<()> {
    // On macOS, ca_injector is not supported — use the native security CLI directly.
    #[cfg(target_os = "macos")]
    return install_ca_macos(cert_path);

    #[cfg(not(target_os = "macos"))]
    {
        let path_str = cert_path.to_str().context("Invalid path string")?;

        if let Err(e) = ca_injector::install_ca(path_str) {
            let msg = e.to_string();
            if msg.contains("Permission denied") || msg.contains("os error 13") {
                return Err(CliError::message(
                    "Permission denied. Please run `locald admin setup` to configure HTTPS trust.",
                ));
            }

            #[cfg(target_os = "linux")]
            {
                if msg.contains("cannot find binary path") {
                    return install_ca_linux_fallback(cert_path).map_err(|err| {
                        anyhow::Error::from(err)
                            .context(format!("ca_injector failed: {msg}"))
                            .into()
                    });
                }
            }

            return Err(e.context("Failed to install CA certificate").into());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn install_ca_linux_fallback(cert_path: &std::path::Path) -> CliResult<()> {
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
            return Err(CliError::message(format!(
                "update-ca-trust extract failed with status: {status}"
            )));
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
            return Err(CliError::message(format!(
                "update-ca-certificates failed with status: {status}"
            )));
        }
        return Ok(());
    }

    Err(CliError::message(
        "No known Linux trust-store directories found; install p11-kit-trust / update-ca-trust or equivalent",
    ))
}

#[cfg(target_os = "macos")]
fn install_ca_macos(cert_path: &std::path::Path) -> CliResult<()> {
    // Use the `security` CLI to add the cert as trusted. This is the most reliable
    // approach on macOS — the native SecTrustSettings API has issues with authorization
    // dialogs and session contexts that the CLI handles transparently.
    //
    // Idempotent: `security add-trusted-cert` succeeds silently if the cert is already trusted.
    //
    // When root: add to System keychain with admin-domain trust (system-wide, no GUI prompt).
    // When user: add to login keychain with user-domain trust.
    let status = if nix::unistd::geteuid().is_root() {
        std::process::Command::new("security")
            .args(["add-trusted-cert", "-d", "-r", "trustRoot", "-k"])
            .arg("/Library/Keychains/System.keychain")
            .arg(cert_path)
            .status()
            .context("Failed to execute `security add-trusted-cert`")?
    } else {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        std::process::Command::new("security")
            .args(["add-trusted-cert", "-r", "trustRoot", "-k"])
            .arg(home.join("Library/Keychains/login.keychain-db"))
            .arg(cert_path)
            .status()
            .context("Failed to execute `security add-trusted-cert`")?
    };

    if !status.success() {
        return Err(CliError::message(format!(
            "`security add-trusted-cert` failed with status: {status}. \
             You may need to run `sudo locald admin setup` or manually trust the CA in Keychain Access."
        )));
    }

    Ok(())
}
