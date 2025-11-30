use serde::{Deserialize, Serialize};
use crate::config::LocaldConfig;

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcRequest {
    Ping,
    Register(LocaldConfig),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcResponse {
    Pong,
    Ok,
    Error(String),
}
