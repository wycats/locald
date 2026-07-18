//! Platform and sandbox-aware locald storage paths.

use std::path::PathBuf;

/// Return locald's durable data directory.
///
/// Explicit sandbox mode honors `XDG_DATA_HOME` on every supported platform.
/// This keeps named macOS sandboxes isolated even though the platform-native
/// directories backend does not ordinarily consult XDG variables there.
/// If platform directory discovery is unavailable, an absolute XDG data path
/// or home directory keeps daemon state attached to the current user.
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

    standard_data_dir(
        directories::ProjectDirs::from("com", "locald", "locald")
            .map(|dirs| dirs.data_local_dir().to_path_buf()),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn standard_data_dir(
    project_data_dir: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> PathBuf {
    project_data_dir
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| fallback_data_dir(xdg_data_home, home))
}

fn fallback_data_dir(xdg_data_home: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    xdg_data_home
        .filter(|path| path.is_absolute())
        .map(|path| path.join("locald"))
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|path| path.join(".local/share/locald"))
        })
        .unwrap_or_else(|| PathBuf::from(".locald"))
}

fn sandbox_data_dir(xdg_data_home: Option<PathBuf>, home: Option<PathBuf>, name: &str) -> PathBuf {
    xdg_data_home.map_or_else(
        || {
            home.map_or_else(
                || {
                    PathBuf::from(".locald/sandboxes")
                        .join(name)
                        .join("data/locald")
                },
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

    #[test]
    fn sandbox_relative_fallback_keeps_the_locald_data_component() {
        assert_eq!(
            sandbox_data_dir(None, None, "alpha"),
            PathBuf::from(".locald/sandboxes/alpha/data/locald")
        );
    }

    #[test]
    fn platform_data_directory_remains_primary() {
        let platform = PathBuf::from("/Users/test/Library/Application Support/com.locald.locald");

        assert_eq!(
            standard_data_dir(
                Some(platform.clone()),
                Some(PathBuf::from("/var/local/test-data")),
                Some(PathBuf::from("/Users/test")),
            ),
            platform
        );
    }

    #[test]
    fn platform_fallback_prefers_xdg_data_home() {
        assert_eq!(
            standard_data_dir(
                None,
                Some(PathBuf::from("/var/local/test-data")),
                Some(PathBuf::from("/Users/test")),
            ),
            PathBuf::from("/var/local/test-data/locald")
        );
    }

    #[test]
    fn platform_fallback_uses_home_when_xdg_data_home_is_unavailable() {
        assert_eq!(
            standard_data_dir(None, None, Some(PathBuf::from("/Users/test"))),
            PathBuf::from("/Users/test/.local/share/locald")
        );
    }

    #[test]
    fn platform_fallback_rejects_relative_candidates() {
        assert_eq!(
            standard_data_dir(
                Some(PathBuf::from("relative-platform-data")),
                Some(PathBuf::from("relative-xdg")),
                Some(PathBuf::from("/Users/test")),
            ),
            PathBuf::from("/Users/test/.local/share/locald")
        );
    }

    #[test]
    fn relative_storage_is_only_the_final_fallback() {
        assert_eq!(
            standard_data_dir(
                Some(PathBuf::from("relative-platform-data")),
                Some(PathBuf::from("relative-xdg")),
                Some(PathBuf::from("relative-home")),
            ),
            PathBuf::from(".locald")
        );
    }
}
