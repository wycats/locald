//! Container environment detection helpers.
//!
//! This module intentionally only provides minimal detection helpers.
//! Host-exec and shim-daemon workflows were removed per RFC 0138.

/// Check if we're probably running inside a container.
///
/// This is a lightweight heuristic intended for user-facing guidance only.
pub async fn is_probably_container() -> bool {
    blocking::is_probably_container()
}

/// Blocking (synchronous) versions of container functions.
///
/// Use these when calling from non-async code, such as early startup
/// before the tokio runtime is initialized. These functions use
/// `tokio::runtime::Runtime::block_on` internally.
pub mod blocking {

    /// Check if we're probably running inside a container (blocking version).
    ///
    /// This creates a temporary tokio runtime to execute the async detection.
    /// Prefer the async version when possible.
    #[allow(clippy::disallowed_methods)] // Intentionally blocking for early startup
    pub fn is_probably_container() -> bool {
        // For container detection, we can use a simpler sync approach
        // that doesn't require the full tokio runtime for most checks.

        // Tier 1: Environment variables (instant, no I/O)
        if std::env::var("FLATPAK_ID").is_ok() {
            return true;
        }
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            return true;
        }
        if std::env::var("TOOLBOX_PATH").is_ok() {
            return true;
        }
        if std::env::var("DISTROBOX_ENTER_PATH").is_ok() {
            return true;
        }
        if std::env::var("container").is_ok() {
            return true;
        }

        // Tier 2: Marker files
        std::path::Path::new("/.flatpak-info").exists()
            || std::path::Path::new("/run/.containerenv").exists()
            || std::path::Path::new("/.dockerenv").exists()
            || std::path::Path::new("/run/systemd/container").exists()
    }

    /// Check if a command exists on PATH (blocking version).
    #[allow(clippy::disallowed_methods)] // Intentionally blocking for early startup
    pub fn command_exists(name: &str) -> bool {
        if name.contains('/') {
            return std::path::Path::new(name).is_file();
        }

        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };

        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return true;
            }
        }

        false
    }
}
