use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::config::LocaldConfig;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum HealthStatus {
    Unknown,
    Starting,
    Healthy,
    Unhealthy,
}

impl Default for HealthStatus {
    fn default() -> Self {
        HealthStatus::Unknown
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum HealthSource {
    None,
    Docker,
    Notify,
    Tcp,
    Explicit,
}

impl Default for HealthSource {
    fn default() -> Self {
        HealthSource::None
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServiceState {
    pub name: String,
    pub config: LocaldConfig,
    pub path: PathBuf,
    pub pid: Option<u32>,
    pub container_id: Option<String>,
    pub port: Option<u16>,
    pub status: String, // "running", "stopped"
    #[serde(default)]
    pub health_status: HealthStatus,
    #[serde(default)]
    pub health_source: HealthSource,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ServerState {
    pub services: Vec<ServiceState>,
}
