//! Menu bar agent management utilities (macOS).

use anyhow::{Context, Result};
use std::path::Path;

/// Install the agent binary to the specified path.
///
/// Writes the embedded bytes and sets executable permissions (0o755).
/// When running as root under `sudo`, chowns the file and parent directory
/// to the invoking user so that later non-root auto-updates can overwrite it.
///
/// # Errors
///
/// Returns an error if writing the file or setting permissions fails.
#[cfg(unix)]
#[allow(clippy::disallowed_methods)]
pub fn install(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create agent directory")?;
    }

    std::fs::write(path, bytes).context("Failed to write agent binary")?;

    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).context("Failed to chmod agent binary")?;

    // When running under sudo, chown the agent binary and parent dir to the
    // invoking user so non-root auto-updates (e.g. `locald tray start`) can
    // overwrite the file later.
    if nix::unistd::geteuid().is_root()
        && let Ok(sudo_uid) = std::env::var("SUDO_UID")
        && let Ok(sudo_gid) = std::env::var("SUDO_GID")
        && let Ok(uid) = sudo_uid.parse::<u32>()
        && let Ok(gid) = sudo_gid.parse::<u32>()
    {
        let uid = nix::unistd::Uid::from_raw(uid);
        let gid = nix::unistd::Gid::from_raw(gid);
        if let Err(e) = nix::unistd::chown(path, Some(uid), Some(gid)) {
            tracing::warn!("Failed to chown agent binary: {e}");
        }
        if let Some(parent) = path.parent()
            && let Err(e) = nix::unistd::chown(parent, Some(uid), Some(gid))
        {
            tracing::warn!("Failed to chown agent directory: {e}");
        }
    }

    Ok(())
}

/// Verify the on-disk agent binary matches the expected bytes.
///
/// Returns `Ok(true)` if the binary exists and matches, `Ok(false)` if it
/// doesn't exist or differs, or an error if the file can't be read.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read.
#[allow(clippy::disallowed_methods)]
pub fn verify_integrity(agent_path: &Path, expected_bytes: &[u8]) -> Result<bool> {
    if !agent_path.exists() {
        return Ok(false);
    }

    let file_bytes = std::fs::read(agent_path).context("Failed to read agent binary")?;
    Ok(file_bytes == expected_bytes)
}

/// Returns the expected agent binary path inside the locald data directory.
///
/// On macOS: `~/Library/Application Support/locald/locald-agent`
///
/// # Errors
///
/// Returns an error if the home directory cannot be determined.
pub fn agent_path() -> Result<std::path::PathBuf> {
    let data_dir = crate::cert::locald_data_dir()?;
    Ok(data_dir.join("locald-agent"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_integrity_returns_false_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent-agent");
        assert!(!verify_integrity(&path, b"anything").unwrap());
    }

    #[test]
    fn verify_integrity_returns_true_for_matching_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locald-agent");
        std::fs::write(&path, b"hello agent").unwrap();
        assert!(verify_integrity(&path, b"hello agent").unwrap());
    }

    #[test]
    fn verify_integrity_returns_false_for_mismatched_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locald-agent");
        std::fs::write(&path, b"old version").unwrap();
        assert!(!verify_integrity(&path, b"new version").unwrap());
    }

    #[test]
    fn install_creates_dir_and_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("locald-agent");
        install(&path, b"binary content").unwrap();

        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"binary content");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
