use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct GlobalConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub updates: UpdateConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct UpdateConfig {
    #[serde(default)]
    pub auto_check: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ServerConfig {
    /// Whether to attempt binding to privileged ports (80/443).
    /// If true, failure to bind these ports will result in an error.
    /// Use sandbox mode (`locald --sandbox test up`) for unprivileged testing.
    #[serde(default = "default_true")]
    pub privileged_ports: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            privileged_ports: true,
        }
    }
}

const fn default_true() -> bool {
    true
}
