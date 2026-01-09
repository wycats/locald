//! Plugin discovery for locald.
//!
//! Searches for WASM plugin components in standard locations:
//! 1. Project-local: `.locald/plugins/*.wasm`
//! 2. User-global: `$XDG_DATA_HOME/locald/plugins/*.wasm`

use std::path::{Path, PathBuf};

/// Discover all plugin WASM components in the given project directory.
///
/// Searches in order:
/// 1. `{project_root}/.locald/plugins/*.wasm`
/// 2. `$XDG_DATA_HOME/locald/plugins/*.wasm` (fallback: `~/.local/share/locald/plugins`)
///
/// Returns paths to all discovered `.wasm` files.
#[must_use]
pub fn discover_plugins(project_root: &Path) -> Vec<PathBuf> {
    let mut plugins = Vec::new();

    // Project-local plugins
    let project_plugins_dir = project_root.join(".locald/plugins");
    if project_plugins_dir.is_dir() {
        plugins.extend(scan_plugin_dir(&project_plugins_dir));
    }

    // User-global plugins
    if let Some(user_plugins_dir) = user_plugins_dir() {
        if user_plugins_dir.is_dir() {
            plugins.extend(scan_plugin_dir(&user_plugins_dir));
        }
    }

    plugins
}

/// Get the user-global plugins directory.
///
/// Uses `$XDG_DATA_HOME/locald/plugins` if set, otherwise `~/.local/share/locald/plugins`.
#[must_use]
pub fn user_plugins_dir() -> Option<PathBuf> {
    std::env::var("XDG_DATA_HOME")
        .ok()
        .map(|xdg_data| PathBuf::from(xdg_data).join("locald/plugins"))
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| PathBuf::from(home).join(".local/share/locald/plugins"))
        })
}

/// Scan a directory for `.wasm` plugin files.
fn scan_plugin_dir(dir: &Path) -> Vec<PathBuf> {
    let mut plugins = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return plugins;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "wasm" {
                    plugins.push(path);
                }
            }
        }
    }

    // Sort for deterministic ordering
    plugins.sort();
    plugins
}

/// Default host capabilities for plugin execution.
///
/// Phase 29.1.3 provides minimal capabilities:
/// - IR version 1
/// - No special grants (plugins are sandboxed)
#[must_use]
pub fn default_capabilities() -> super::HostCapabilities {
    super::HostCapabilities {
        supported_ir_versions: vec![1],
        granted: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discovers_project_plugins() {
        let temp = TempDir::new().unwrap();
        let plugins_dir = temp.path().join(".locald/plugins");
        fs::create_dir_all(&plugins_dir).unwrap();

        // Create some test files
        fs::write(plugins_dir.join("redis.wasm"), b"wasm").unwrap();
        fs::write(plugins_dir.join("postgres.wasm"), b"wasm").unwrap();
        fs::write(plugins_dir.join("not-a-plugin.txt"), b"text").unwrap();

        let discovered = discover_plugins(temp.path());

        assert_eq!(discovered.len(), 2);
        assert!(discovered[0].ends_with("postgres.wasm")); // Sorted alphabetically
        assert!(discovered[1].ends_with("redis.wasm"));
    }

    #[test]
    fn returns_empty_if_no_plugins_dir() {
        let temp = TempDir::new().unwrap();
        let discovered = discover_plugins(temp.path());
        assert!(discovered.is_empty());
    }

    #[test]
    fn default_capabilities_has_ir_version_1() {
        let caps = default_capabilities();
        assert!(caps.supported_ir_versions.contains(&1));
        assert!(caps.granted.is_empty());
    }
}
