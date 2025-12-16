//! Environment variable and path utilities.

use directories::ProjectDirs;
use std::path::PathBuf;

/// Returns true if the sandbox mode is active.
///
/// This is determined by the `LOCALD_SANDBOX_ACTIVE` environment variable.
pub fn is_sandbox_active() -> bool {
    std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok()
}

/// Returns the sandbox name, if sandbox mode is active.
///
/// This is determined by the `LOCALD_SANDBOX_NAME` environment variable.
///
/// Note: sandbox mode can be active without a name (older environments). In that case
/// this returns `None`.
pub fn sandbox_name() -> Option<String> {
    if !is_sandbox_active() {
        return None;
    }

    let name = std::env::var("LOCALD_SANDBOX_NAME").ok()?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Returns the sandbox name, defaulting to "default" when not running in sandbox mode.
pub fn sandbox_name_or_default() -> String {
    sandbox_name().unwrap_or_else(|| "default".to_string())
}

/// Returns the user's home directory.
pub fn get_home_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Returns the XDG data directory for locald.
///
/// Defaults to `~/.local/share/locald` on Linux.
pub fn get_xdg_data_home() -> PathBuf {
    ProjectDirs::from("com", "locald", "locald").map_or_else(
        || PathBuf::from(".locald"),
        |proj_dirs| proj_dirs.data_dir().to_path_buf(),
    )
}

/// Returns the XDG config directory for locald.
///
/// Defaults to `~/.config/locald` on Linux.
pub fn get_xdg_config_home() -> PathBuf {
    ProjectDirs::from("com", "locald", "locald").map_or_else(
        || PathBuf::from(".locald"),
        |proj_dirs| proj_dirs.config_dir().to_path_buf(),
    )
}
