//! Environment variable and path utilities.

use directories::ProjectDirs;
use std::path::PathBuf;

/// Returns true if the sandbox mode is active.
///
/// This is determined by the `LOCALD_SANDBOX_ACTIVE` environment variable.
pub fn is_sandbox_active() -> bool {
    std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok()
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
