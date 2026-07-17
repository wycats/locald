//! Platform and sandbox-aware locald storage paths.

use std::path::PathBuf;

/// Return locald's durable data directory.
///
/// Explicit sandbox mode honors `XDG_DATA_HOME` on every supported platform.
/// This keeps named macOS sandboxes isolated even though the platform-native
/// directories backend does not ordinarily consult XDG variables there.
#[must_use]
pub fn data_dir() -> PathBuf {
    if std::env::var_os("LOCALD_SANDBOX_ACTIVE").is_some() {
        let sandbox_name = std::env::var("LOCALD_SANDBOX_NAME")
            .ok()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "default".to_owned());
        return sandbox_data_dir(
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
            &sandbox_name,
        );
    }

    directories::ProjectDirs::from("com", "locald", "locald").map_or_else(
        || PathBuf::from(".locald"),
        |dirs| dirs.data_local_dir().to_path_buf(),
    )
}

fn sandbox_data_dir(xdg_data_home: Option<PathBuf>, home: Option<PathBuf>, name: &str) -> PathBuf {
    xdg_data_home.map_or_else(
        || {
            home.map_or_else(
                || PathBuf::from(".locald/sandboxes").join(name).join("data"),
                |home| {
                    home.join(".local/share/locald/sandboxes")
                        .join(name)
                        .join("data/locald")
                },
            )
        },
        |xdg| xdg.join("locald"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_sandbox_data_isolated_from_platform_directory() {
        let platform =
            std::path::Path::new("/Users/test/Library/Application Support/com.locald.locald");
        let sandbox = sandbox_data_dir(
            Some(PathBuf::from("/tmp/locald/sandboxes/alpha/data")),
            Some(PathBuf::from("/Users/test")),
            "alpha",
        );

        assert_eq!(
            sandbox,
            PathBuf::from("/tmp/locald/sandboxes/alpha/data/locald")
        );
        assert_ne!(sandbox, platform);
    }

    #[test]
    fn sandbox_fallback_remains_named_and_user_scoped() {
        assert_eq!(
            sandbox_data_dir(None, Some(PathBuf::from("/Users/test")), "alpha"),
            PathBuf::from("/Users/test/.local/share/locald/sandboxes/alpha/data/locald")
        );
    }
}
