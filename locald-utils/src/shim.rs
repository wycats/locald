use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Find the locald-shim binary and ensure it has the correct permissions.
///
/// This is a convenience wrapper around `find` and `ensure_permissions_with_sudo`.
///
/// # Errors
///
/// Returns an error if the shim cannot be found or if permissions cannot be set.
pub fn get_configured_shim() -> Result<PathBuf> {
    let shim_path = find()?.ok_or_else(|| anyhow::anyhow!("Could not find locald-shim"))?;
    ensure_permissions_with_sudo(&shim_path)?;
    Ok(shim_path)
}

/// Returns whether the shim at `shim_path` is already configured for privileged use.
///
/// On Unix, this checks `root` ownership and the setuid bit.
///
/// # Errors
///
/// Returns an error if the shim exists but its filesystem metadata cannot be read.
pub fn is_privileged(shim_path: &Path) -> Result<bool> {
    if !shim_path.exists() {
        return Ok(false);
    }

    #[cfg(unix)]
    {
        let metadata = std::fs::metadata(shim_path)?;
        let uid = metadata.uid();
        let mode = metadata.mode();

        let is_root = uid == 0;
        let is_setuid = (mode & 0o4000) != 0;
        Ok(is_root && is_setuid)
    }

    #[cfg(not(unix))]
    {
        Ok(false)
    }
}

/// Find the locald-shim binary, but only return it if it is already configured.
///
/// This is appropriate for daemon contexts where we must never trigger an interactive sudo prompt.
///
/// # Errors
///
/// Returns an error if shim discovery fails or if filesystem metadata cannot be read.
pub fn find_privileged() -> Result<Option<PathBuf>> {
    let Some(path) = find()? else {
        return Ok(None);
    };

    if is_privileged(&path)? {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

/// Create a `tokio::process::Command` preconfigured to invoke `locald-shim`.
///
/// This centralizes env hygiene for all shim invocations.
pub fn tokio_command(shim_path: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(shim_path);
    cmd.env_remove("LD_LIBRARY_PATH");
    cmd
}

/// Convenience helper to get an interactive shim command.
///
/// This may prompt for `sudo` to repair permissions.
///
/// # Errors
///
/// Returns an error if the shim cannot be found, or if permissions are not correct and
/// interactive repair is not allowed or fails.
pub fn tokio_command_interactive() -> Result<tokio::process::Command> {
    let shim_path = get_configured_shim()?;
    Ok(tokio_command(&shim_path))
}

/// Convenience helper to get a daemon-safe shim command.
///
/// This never prompts for `sudo`; it requires an already-installed privileged shim.
///
/// # Errors
///
/// Returns an error if a privileged shim cannot be found.
pub fn tokio_command_privileged() -> Result<tokio::process::Command> {
    let shim_path = find_privileged()?.ok_or_else(|| {
        anyhow::anyhow!(
            "locald-shim is not installed or not setuid root; run `sudo locald admin setup`"
        )
    })?;
    Ok(tokio_command(&shim_path))
}

/// Find the locald-shim binary.
///
/// Search order:
/// 1. Sibling of the current executable
/// 2. PATH (not implemented yet, as we prefer explicit location)
///
/// # Errors
///
/// Returns an error if `current_exe` fails.
pub fn find() -> Result<Option<PathBuf>> {
    // 1. Check sibling
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(dir) = exe_path.parent()
    {
        let shim_path = dir.join("locald-shim");
        if shim_path.exists() {
            debug!("Found shim sibling: {:?}", shim_path);
            return Ok(Some(shim_path));
        }

        // 2. Check parent (useful for cargo test where exe is in deps/)
        if let Some(parent) = dir.parent() {
            let shim_path = parent.join("locald-shim");
            if shim_path.exists() {
                debug!("Found shim in parent: {:?}", shim_path);
                return Ok(Some(shim_path));
            }
        }
    }

    Ok(None)
}

/// Verify that the shim at the given path matches the expected binary content.
///
/// This compares the file content on disk with the provided bytes.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::disallowed_methods)]
pub fn verify_integrity(shim_path: &Path, expected_bytes: &[u8]) -> Result<bool> {
    if !shim_path.exists() {
        return Ok(false);
    }

    let file_bytes = std::fs::read(shim_path).context("Failed to read shim file")?;

    // Simple byte comparison
    Ok(file_bytes == expected_bytes)
}

/// Ensure the shim has the correct permissions (root:root, 4755).
///
/// # Errors
///
/// Returns an error if `chown` or `chmod` fails.
#[cfg(unix)]
pub fn ensure_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // 1. Set owner to root:root
    let uid = nix::unistd::Uid::from_raw(0);
    let gid = nix::unistd::Gid::from_raw(0);
    nix::unistd::chown(path, Some(uid), Some(gid)).context("Failed to chown shim")?;

    // 2. Set permissions to 4755 (setuid root)
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o4755);
    std::fs::set_permissions(path, perms).context("Failed to chmod shim")?;

    Ok(())
}

/// Install the shim binary to the specified path and set permissions.
///
/// This requires root privileges.
///
/// # Errors
///
/// Returns an error if writing the file or setting permissions fails.
#[cfg(unix)]
#[allow(clippy::disallowed_methods)]
pub fn install(path: &Path, bytes: &[u8]) -> Result<()> {
    // 1. Write file
    std::fs::write(path, bytes).context("Failed to write shim binary")?;

    // 2. Set permissions
    ensure_permissions(path)?;

    Ok(())
}

/// Ensure the shim has the correct permissions (root:root, setuid), using sudo if necessary.
///
/// This is useful for development environments where the shim might be rebuilt and lose permissions.
///
/// # Errors
///
/// Returns an error if `sudo` fails or if the user cancels the password prompt.
#[cfg(unix)]
pub fn ensure_permissions_with_sudo(shim_path: &Path) -> Result<()> {
    use std::io::IsTerminal;

    if !shim_path.exists() {
        return Ok(());
    }

    let metadata = std::fs::metadata(shim_path)?;
    let uid = metadata.uid();
    let mode = metadata.mode();

    // Check if owned by root and has setuid bit (0o4000)
    let is_root = uid == 0;
    let is_setuid = (mode & 0o4000) != 0;

    if !is_root || !is_setuid {
        warn!(
            "locald-shim at {} needs setup (uid: {}, mode: {:o})",
            shim_path.display(),
            uid,
            mode
        );

        // Default to no surprises: never block on an interactive sudo password prompt
        // unless the caller explicitly opted in.
        let interactive_opt_in =
            std::env::var("LOCALD_SHIM_INTERACTIVE").ok().as_deref() == Some("1");

        if !interactive_opt_in || !std::io::stdout().is_terminal() {
            #[cfg(debug_assertions)]
            {
                let bt = std::backtrace::Backtrace::force_capture();
                debug!(
                    "Shim permission repair would require sudo (disabled by default). Backtrace:\n{bt}"
                );
            }

            anyhow::bail!(
                "Privileged locald-shim is not configured. Run `sudo locald admin setup` to install/repair the setuid shim."
            );
        }

        info!("Running sudo to fix locald-shim permissions");

        #[allow(clippy::disallowed_methods)]
        let status = Command::new("sudo")
            .arg("chown")
            .arg("root:root")
            .arg(shim_path)
            .status()?;

        if !status.success() {
            anyhow::bail!("Failed to chown locald-shim");
        }

        #[allow(clippy::disallowed_methods)]
        let status = Command::new("sudo")
            .arg("chmod")
            .arg("u+s")
            .arg(shim_path)
            .status()?;

        if !status.success() {
            anyhow::bail!("Failed to chmod locald-shim");
        }

        info!("locald-shim permissions fixed");
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn ensure_permissions_with_sudo(_shim_path: &Path) -> Result<()> {
    Ok(())
}
