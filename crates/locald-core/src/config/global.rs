use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct GlobalConfig {
    #[serde(default)]
    pub server: ServerConfig,

    #[serde(default)]
    pub container: ContainerConfig,
}

/// Configuration for running locald inside container environments (Toolbx, Distrobox, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct ContainerConfig {
    /// Template for running commands on the host from inside a container.
    ///
    /// The placeholder `{command}` is replaced with the actual command to run.
    /// If not set, auto-detection is attempted (flatpak-spawn, distrobox-host-exec).
    ///
    /// Examples:
    /// - `"flatpak-spawn --host {command}"`
    /// - `"distrobox-host-exec {command}"`
    /// - `"ssh myhost {command}"`
    #[serde(default)]
    pub host_exec: Option<String>,

    /// Override the socket path for the shim daemon.
    ///
    /// Defaults to `~/.locald/shim.sock`. Use this if your home directory
    /// is on a networked filesystem that doesn't support Unix sockets.
    #[serde(default)]
    pub shim_socket: Option<PathBuf>,
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
