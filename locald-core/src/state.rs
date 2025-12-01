use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::config::LocaldConfig;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServiceState {
    pub name: String,
    pub config: LocaldConfig,
    pub path: PathBuf,
    pub pid: Option<u32>,
    pub container_id: Option<String>,
    pub port: Option<u16>,
    pub status: String, // "running", "stopped"
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ServerState {
    pub services: Vec<ServiceState>,
}
