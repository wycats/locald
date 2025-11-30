use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub status: String,
    pub url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcRequest {
    Ping,
    Start { path: PathBuf },
    Stop { name: String },
    Status,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcResponse {
    Pong,
    Ok,
    Status(Vec<ServiceStatus>),
    Error(String),
}
