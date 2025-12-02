use crate::config::LocaldConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthStatus {
    #[default]
    Unknown,
    Starting,
    Healthy,
    Unhealthy,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthSource {
    #[default]
    None,
    Docker,
    Notify,
    Tcp,
    Explicit,
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
