pub mod config;

pub use config::LocaldConfig;
pub mod ipc;
pub use ipc::{IpcRequest, IpcResponse};
pub mod hosts;
pub use hosts::HostsFileSection;
