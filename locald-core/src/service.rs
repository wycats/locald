use crate::config::ServiceConfig;
use crate::ipc::{LogEntry, ServiceMetrics};
use crate::state::{HealthStatus, ServiceState};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::path::PathBuf;

/// The dynamic runtime state of a service.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeState {
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub status: ServiceState,
    pub health_status: HealthStatus,
}

#[derive(Debug, Clone)]
pub enum ServiceCommand {
    /// Reset the service to its initial state (e.g., wipe data).
    Reset,
    /// Custom command (e.g., "run-migration").
    Custom(String, Vec<String>),
}

#[async_trait]
pub trait ServiceController: Send + Sync + std::fmt::Debug {
    /// Unique identifier for this service instance (e.g., "postgres:15").
    fn id(&self) -> &str;

    /// Prepare the service for execution.
    /// This handles heavy lifting: downloading binaries, pulling Docker images,
    /// compiling code, or initializing data directories.
    ///
    /// This step is distinct from `start` to allow the UI to show "Building..."
    /// or "Downloading..." states separately from "Starting...".
    async fn prepare(&mut self) -> Result<()>;

    /// Start the service.
    /// This should be fast and idempotent. It assumes `prepare` has succeeded.
    async fn start(&mut self) -> Result<()>;

    /// Stop the service.
    async fn stop(&mut self) -> Result<()>;

    /// Get the current runtime state of the service.
    /// This returns the dynamic parts of the status (PID, Port, State).
    /// The Manager combines this with static config (Name, Domain) to form the full `ServiceStatus`.
    async fn read_state(&self) -> RuntimeState;

    /// Get a stream of logs from the service.
    async fn logs(&self) -> BoxStream<'static, LogEntry>;

    /// Get metadata about the service (e.g., "port", "url", "connection_string").
    fn get_metadata(&self, key: &str) -> Option<String>;

    /// Execute a specific command on the service.
    /// This provides an escape hatch for capabilities like "reset", "snapshot", etc.
    /// Returns `NotSupported` if the service doesn't handle the command.
    async fn execute_command(&mut self, cmd: ServiceCommand) -> Result<()>;

    /// Serialize the runtime state for persistence.
    fn snapshot(&self) -> serde_json::Value;

    /// Restore runtime state from a snapshot.
    async fn restore(&mut self, state: serde_json::Value) -> Result<()>;

    /// Get current resource usage metrics.
    async fn metrics(&self) -> Result<Option<ServiceMetrics>>;
}

use std::collections::HashMap;

#[derive(Debug)]
pub struct ServiceContext {
    pub project_root: PathBuf,
    pub port: Option<u16>,
    pub env: HashMap<String, String>,
}

use std::sync::Arc;
use tokio::sync::Mutex;

pub trait ServiceFactory: Send + Sync + std::fmt::Debug {
    /// Returns true if this factory can handle the given configuration.
    fn can_handle(&self, config: &ServiceConfig) -> bool;

    /// Creates a new controller for the given configuration.
    /// The `ServiceContext` is injected here, allowing the Factory to pass
    /// necessary dependencies (Docker, StateManager) to the Controller.
    fn create(
        &self,
        name: String,
        config: &ServiceConfig,
        ctx: &ServiceContext,
    ) -> Arc<Mutex<dyn ServiceController>>;
}
