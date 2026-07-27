//! Service controller traits and runtime contracts.
//!
//! Controllers encapsulate the lifecycle of a service and expose a stable
//! interface for the daemon, CLI, and UI. The expected lifecycle is:
//! `prepare` -> `start` -> `stop`, where `prepare` handles heavyweight setup
//! (downloads, builds) and `start` is fast and idempotent.
use crate::config::ServiceConfig;
use crate::identity::ProjectInstanceId;
use crate::ipc::{LogEntry, ServiceMetrics};
use crate::state::{HealthStatus, PersistedProcessIdentity, ServiceState};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// The configured name of one service within a locald project.
///
/// Service names are meaningful only inside their owning project instance.
/// Runtime registries therefore use [`ServiceKey`] instead of a display name
/// such as `project:web`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServiceName(String);

impl ServiceName {
    /// Construct a service name from its exact configured value.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Return the exact configured value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServiceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<&str> for ServiceName {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for ServiceName {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

/// The stable runtime identity of one service in one physical project
/// instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ServiceKey {
    instance: ProjectInstanceId,
    name: ServiceName,
}

impl ServiceKey {
    /// Construct an instance-scoped service key.
    #[must_use]
    pub fn new(instance: ProjectInstanceId, name: impl Into<ServiceName>) -> Self {
        Self {
            instance,
            name: name.into(),
        }
    }

    /// Return the owning physical project instance.
    #[must_use]
    pub const fn instance(&self) -> ProjectInstanceId {
        self.instance
    }

    /// Return the configured service name.
    #[must_use]
    pub const fn name(&self) -> &ServiceName {
        &self.name
    }

    /// Render the existing human-facing `project:service` label.
    #[must_use]
    pub fn display_name(&self, project_name: &str) -> String {
        format!("{project_name}:{}", self.name)
    }
}

/// The dynamic runtime state of a service.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeState {
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub status: ServiceState,
    pub health_status: HealthStatus,
}

/// Commands supported by controllers beyond start/stop.
#[derive(Debug, Clone)]
pub enum ServiceCommand {
    /// Reset the service to its initial state (e.g., wipe data).
    Reset,
    /// Custom command (e.g., "run-migration").
    Custom(String, Vec<String>),
}

/// A controller manages the lifecycle and runtime I/O for a single service.
///
/// Implementations must honor the lifecycle contract: `prepare` does heavy work,
/// `start` transitions to a running state, and `stop` shuts the service down.
#[async_trait]
pub trait ServiceController: Send + Sync + std::fmt::Debug {
    /// Unique identifier for this service instance (e.g., "postgres:15").
    fn id(&self) -> &str;

    /// Prepare the service for execution.
    ///
    /// This handles heavy lifting: downloading binaries, pulling Docker images,
    /// compiling code, or initializing data directories. Keeping this separate
    /// allows the UI to show "Building..." states independently of `start`.
    async fn prepare(&mut self) -> Result<()>;

    /// Start the service.
    ///
    /// This should be fast and idempotent. It assumes `prepare` has succeeded.
    async fn start(&mut self) -> Result<()>;

    /// Stop the service and release resources.
    async fn stop(&mut self) -> Result<()>;

    /// Get the current runtime state of the service.
    ///
    /// This returns the dynamic parts of the status (PID, port, lifecycle state).
    /// The manager combines this with static config to form the full status view.
    async fn read_state(&self) -> RuntimeState;

    /// Return the immutable process ID captured when this controller spawned
    /// its current child. Unlike `RuntimeState::pid`, this cleanup handle stays
    /// present after the leader exits and is cleared only after `stop()` has
    /// confirmed that the owned process or process group is gone.
    fn owned_process_id(&self) -> Option<u32> {
        None
    }

    /// Return the immutable OS identity captured while this controller owned
    /// its current child. It has the same lifetime as `owned_process_id`;
    /// controllers without an OS process return `None`.
    fn process_identity(&self) -> Option<PersistedProcessIdentity> {
        None
    }

    /// Get a stream of logs from the service.
    async fn logs(&self) -> BoxStream<'static, LogEntry>;

    /// Subscribe to PTY output if available.
    fn subscribe_pty(&self) -> Option<tokio::sync::broadcast::Receiver<Vec<u8>>> {
        None
    }

    /// Get metadata about the service (e.g., "port", "url", "connection_string").
    fn get_metadata(&self, key: &str) -> Option<String>;

    /// Execute a specific command on the service.
    ///
    /// This provides an escape hatch for capabilities like "reset", "snapshot", etc.
    /// Returns `NotSupported` if the service doesn't handle the command.
    async fn execute_command(&mut self, cmd: ServiceCommand) -> Result<()>;

    /// Write data to the service's standard input (PTY).
    async fn write_stdin(&self, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    /// Resize the service's PTY.
    async fn resize_pty(&self, _rows: u16, _cols: u16) -> Result<()> {
        Ok(())
    }

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
    pub key: ServiceKey,
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
