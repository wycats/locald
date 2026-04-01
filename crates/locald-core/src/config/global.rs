use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct GlobalConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub updates: UpdateConfig,
}

/// Configuration for automatic update checks.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct UpdateConfig {
    /// Whether to check for updates when running `locald up`.
    /// When enabled, a background check runs (at most once per 24 hours)
    /// and displays a message if a newer version is available.
    /// This is opt-in and disabled by default.
    #[serde(default)]
    pub auto_check: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ServerConfig {
    /// Whether to attempt binding to privileged ports (80/443).
    /// If true, failure to bind these ports will result in an error.
    /// Use sandbox mode (`locald --sandbox test up`) for unprivileged testing.
    #[serde(default = "default_privileged_ports")]
    pub privileged_ports: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            privileged_ports: default_privileged_ports(),
        }
    }
}

/// Default to privileged ports (80/443) on all platforms.
///
/// This is the intended production experience — `admin setup` configures
/// the platform-specific mechanism (setuid shim on Linux, pfctl on macOS).
/// Sandbox mode explicitly overrides this to false for testing.
const fn default_privileged_ports() -> bool {
    true
}
