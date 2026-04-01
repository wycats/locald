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

/// On Linux, default to privileged ports (80/443 via setuid shim).
/// On macOS, default to unprivileged ports (the shim model doesn't apply).
const fn default_privileged_ports() -> bool {
    cfg!(target_os = "linux")
}
