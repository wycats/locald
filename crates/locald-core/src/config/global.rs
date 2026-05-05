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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct ServerConfig {
    /// Sandbox mode: skip privileged port binding (80/443) and cert trust.
    /// Used for CI, containers, or testing the daemon itself.
    /// When false (the default), the server binds ports 80/443 via the
    /// platform's privileged helper — failure is fatal.
    #[serde(default)]
    pub sandbox: bool,

    // Support reading old config files that use `privileged_ports`.
    #[serde(default, rename = "privileged_ports")]
    #[doc(hidden)]
    privileged_ports_compat: Option<bool>,
}

impl ServerConfig {
    /// Effective sandbox state: explicit `sandbox = true`, or legacy
    /// `privileged_ports = false`.
    pub const fn is_sandbox(&self) -> bool {
        if self.sandbox {
            return true;
        }
        // Legacy compat: privileged_ports = false → sandbox
        matches!(self.privileged_ports_compat, Some(false))
    }
}
