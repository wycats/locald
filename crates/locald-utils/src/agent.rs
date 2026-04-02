//! Menu bar agent management utilities (macOS).

use anyhow::{Context, Result};
use std::path::Path;

/// Install the agent binary to the specified path.
///
/// Writes the embedded bytes and sets executable permissions (0o755).
/// No setuid or root ownership needed — the agent runs as the current user.
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
