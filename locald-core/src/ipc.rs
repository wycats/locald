use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub status: String,
    pub url: Option<String>,
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: i64,
    pub service: String,
    pub stream: String, // "stdout" or "stderr"
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcRequest {
    Ping,
    Start { path: PathBuf },
    Stop { name: String },
    Status,
    Shutdown,
    Logs { service: Option<String> },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcResponse {
    Pong,
    Ok,
    Status(Vec<ServiceStatus>),
    Error(String),
}
