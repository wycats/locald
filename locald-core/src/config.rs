use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocaldConfig {
    pub project: ProjectConfig,
    #[serde(default)]
    pub services: HashMap<String, ServiceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    /// The domain to serve the project on. Defaults to `{name}.local`.
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// The command to run to start the service. Required if `image` is not set.
    pub command: Option<String>,
    /// The Docker image to run. If set, `command` is treated as arguments to the container entrypoint (optional).
    pub image: Option<String>,
    /// The port exposed by the container. Required if `image` is set.
    pub container_port: Option<u16>,
    /// The port the service listens on. If None, locald will assign a port and pass it via PORT env var.
    pub port: Option<u16>,
    /// Environment variables to pass to the service.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory for the command. Defaults to the project root.
    pub workdir: Option<String>,
    /// List of services that must be started before this one.
    #[serde(default)]
    pub depends_on: Vec<String>,
}
