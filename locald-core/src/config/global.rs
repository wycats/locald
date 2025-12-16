use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct GlobalConfig {
    #[serde(default)]
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ServerConfig {
    /// Whether to attempt binding to privileged ports (80/443).
    /// If true, failure to bind these ports will result in an error unless `fallback_ports` is also true.
    #[serde(default = "default_true")]
    pub privileged_ports: bool,

    /// Whether to fallback to unprivileged ports (8080/8443) if privileged ports fail.
    /// Defaults to false to encourage setting up capabilities.
    #[serde(default = "default_false")]
    pub fallback_ports: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            privileged_ports: true,
            fallback_ports: false,
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_false() -> bool {
    false
}
