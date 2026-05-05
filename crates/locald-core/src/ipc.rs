use crate::attachments::{AttachmentSource, ProjectFilter, ProjectListEntry, ProjectStatusInfo};
use crate::config::{ServiceConfig, TypedServiceConfig};
use crate::state::{HealthSource, HealthStatus, ServiceState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Represents the stream a log message came from.
///
/// # Example
/// ```rust
/// use locald_core::ipc::LogStream;
/// let stream = LogStream::Stdout;
/// assert_eq!(stream.to_string(), "stdout");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// The type of a service.
///
/// # Example
/// ```rust
/// use locald_core::ipc::ServiceType;
/// let service_type = ServiceType::Exec;
/// assert_eq!(service_type.to_string(), "exec");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    /// A generic executable service.
    #[default]
    Exec,
    /// A managed Postgres database service.
    Postgres,
    /// A background worker service (no port).
    Worker,
    /// A container-based service.
    Container,
    /// A static site service.
    Site,
}

impl std::fmt::Display for ServiceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exec => write!(f, "exec"),
            Self::Postgres => write!(f, "postgres"),
            Self::Worker => write!(f, "worker"),
            Self::Container => write!(f, "container"),
            Self::Site => write!(f, "site"),
        }
    }
}

impl From<&ServiceConfig> for ServiceType {
    fn from(config: &ServiceConfig) -> Self {
        match config {
            ServiceConfig::Typed(typed) => match typed {
                TypedServiceConfig::Exec(_) => Self::Exec,
                TypedServiceConfig::Postgres(_) => Self::Postgres,
                TypedServiceConfig::Worker(_) => Self::Worker,
                TypedServiceConfig::Container(_) => Self::Container,
                TypedServiceConfig::Site(_) => Self::Site,
            },
            ServiceConfig::Legacy(_) => Self::Exec,
        }
    }
}

impl std::fmt::Display for LogStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdout => write!(f, "stdout"),
            Self::Stderr => write!(f, "stderr"),
        }
    }
}

/// Status information for a single service.
///
/// # Example
/// ```rust
/// use locald_core::ipc::{ServiceStatus, ServiceType};
/// use locald_core::state::{ServiceState, HealthStatus, HealthSource};
///
/// let status = ServiceStatus {
///     name: "web".to_string(),
///     service_type: ServiceType::Exec,
///     pid: Some(1234),
///     port: Some(8080),
///     status: ServiceState::Running,
///     url: Some("http://web.local".to_string()),
///     connection_url: Some("http://localhost:8080".to_string()),
///     domain: Some("web.local".to_string()),
///     health_status: HealthStatus::Healthy,
///     health_source: HealthSource::Http,
///     path: None,
///     workspace: None,
///     constellation: None,
///     warnings: vec![],
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ServiceStatus {
    /// The unique name of the service (e.g., "project:web").
    pub name: String,
    /// The type of service (exec, postgres, worker, container, site).
    #[serde(default)]
    pub service_type: ServiceType,
    /// The process ID of the service, if running.
    pub pid: Option<u32>,
    /// The port the service is listening on, if any.
    pub port: Option<u16>,
    /// The current state of the service (running/stopped).
    pub status: ServiceState,
    /// The public URL for the service, if applicable.
    pub url: Option<String>,
    /// The connection URL for the service (e.g., postgres:// for databases, http://localhost:port for others).
    #[serde(default)]
    pub connection_url: Option<String>,
    /// The domain name for the service, if configured.
    pub domain: Option<String>,
    /// The health status of the service.
    #[serde(default)]
    pub health_status: HealthStatus,
    /// The source of the health check information.
    #[serde(default)]
    pub health_source: HealthSource,
    /// The file system path to the service's project root.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// The name of the workspace the service belongs to.
    #[serde(default)]
    pub workspace: Option<String>,
    /// The name of the constellation the service belongs to.
    #[serde(default)]
    pub constellation: Option<String>,
    /// Any warnings associated with the service (e.g. port mismatch).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// A log entry from a service.
///
/// # Example
/// ```rust
/// use locald_core::ipc::{LogEntry, LogStream};
///
/// let entry = LogEntry {
///     timestamp: 1678886400,
///     service: "web".to_string(),
///     stream: LogStream::Stdout,
///     message: "Server started".to_string(),
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LogEntry {
    /// The timestamp of the log entry (Unix epoch seconds).
    pub timestamp: i64,
    /// The name of the service that generated the log.
    pub service: String,
    /// The stream the log came from (stdout/stderr).
    pub stream: LogStream,
    /// The log message content.
    pub message: String,
}

/// Metrics for a service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServiceMetrics {
    /// The name of the service.
    pub name: String,
    /// CPU usage percentage (0.0 - 100.0 * cores).
    pub cpu_percent: f32,
    /// Memory usage in bytes.
    pub memory_bytes: u64,
    /// Timestamp of the metric (Unix epoch seconds).
    pub timestamp: i64,
}

/// Runtime identity for the daemon currently serving IPC requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonIdentity {
    /// The daemon build version.
    pub version: String,
    /// The daemon process id.
    pub pid: u32,
    /// The executable path for the running daemon process.
    pub executable: PathBuf,
}

/// The mode for log streaming.
///
/// # Example
/// ```rust
/// use locald_core::ipc::LogMode;
/// let mode = LogMode::Follow;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogMode {
    /// Follow the log stream (like `tail -f`).
    Follow,
    /// Return a snapshot of recent logs and exit.
    #[default]
    Snapshot,
}

/// Requests sent from the CLI to the Server.
///
/// # Example
/// ```rust
/// use locald_core::ipc::{IpcRequest, LogMode};
///
/// let req = IpcRequest::Logs {
///     service: Some("web".to_string()),
///     mode: LogMode::Follow,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum IpcRequest {
    /// Check if the server is alive.
    ///
    /// **Response:** `IpcResponse::Pong`
    Ping,
    /// Get the server version.
    ///
    /// **Response:** `IpcResponse::Version(String)`
    GetVersion,
    /// Get the daemon runtime identity.
    ///
    /// **Response:** `IpcResponse::DaemonIdentity(DaemonIdentity)`
    GetDaemonIdentity,
    /// Start a project or service at the given path.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    Start {
        /// The path to the project root or configuration file.
        project_path: PathBuf,
        /// Enable verbose output for build steps.
        #[serde(default)]
        verbose: bool,
    },
    /// Stop a service by name.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    Stop { name: String },
    /// Restart a service by name.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    Restart { name: String },
    /// Reset a service (stop and clear data) by name.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    Reset { name: String },
    /// Stop all running services.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    StopAll,
    /// Restart all running services.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    RestartAll,
    /// Get the status of all services.
    ///
    /// **Response:** `IpcResponse::Status(Vec<ServiceStatus>)`
    Status,
    /// Shut down the server.
    ///
    /// **Response:** `IpcResponse::Ok`
    Shutdown,
    /// Stream logs for a service.
    ///
    /// **Response:** Stream of `Event::Log(LogEntry)`
    Logs {
        /// Optional service name filter.
        service: Option<String>,
        /// The mode for log streaming (Follow or Snapshot).
        #[serde(default)]
        mode: LogMode,
    },
    /// Get the AI context (current state).
    ///
    /// **Response:** `IpcResponse::AiContext(String)`
    AiContext,
    /// Get the JSON schema for the configuration.
    ///
    /// **Response:** `IpcResponse::AiSchema(String)`
    AiSchema,
    /// List projects in the registry.
    ///
    /// **Response:** `IpcResponse::RegistryList(Vec<ProjectEntry>)`
    RegistryList,
    /// Pin a project in the registry.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    RegistryPin {
        /// The path to the project to pin.
        project_path: PathBuf,
    },
    /// Unpin a project from the registry.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    RegistryUnpin {
        /// The path to the project to unpin.
        project_path: PathBuf,
    },
    /// Clean up the registry (remove non-existent projects).
    ///
    /// **Response:** `IpcResponse::RegistryCleaned(usize)`
    RegistryClean,
    /// Register a project attachment.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    ProjectAttach {
        /// The path to the project to attach.
        project_path: PathBuf,
        /// The source of the attachment.
        source: AttachmentSource,
    },
    /// Remove a project attachment.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    ProjectDetach {
        /// The path to the project to detach.
        project_path: PathBuf,
        /// Optional attachment source filter.
        #[serde(default)]
        source: Option<AttachmentSource>,
    },
    /// Get the status of a project.
    ///
    /// **Response:** `IpcResponse::ProjectStatus(ProjectStatusInfo)`
    ProjectStatus {
        /// The path to the project to inspect.
        project_path: PathBuf,
    },
    /// List known projects with attachment state.
    ///
    /// **Response:** `IpcResponse::ProjectList(Vec<ProjectListEntry>)`
    ProjectList {
        /// Optional filter for list results.
        #[serde(default)]
        filter: Option<ProjectFilter>,
    },
    /// Force-start services for a project.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    ProjectForceStart {
        /// The path to the project to start.
        project_path: PathBuf,
    },
    /// Force-stop services for a project.
    ///
    /// **Response:** `IpcResponse::Ok` or `IpcResponse::Error`
    ProjectForceStop {
        /// The path to the project to stop.
        project_path: PathBuf,
    },
    /// Get the resolved environment variables for a service.
    ///
    /// **Response:** `IpcResponse::ServiceEnv(HashMap<String, String>)`
    GetServiceEnv { name: String },
    /// Run an ephemeral container.
    ///
    /// **Response:** `IpcResponse::Ok` (detached) or Stream of `Event::Log` (attached)
    RunContainer {
        /// The image to run (e.g., "alpine:latest").
        image: String,
        /// The command to run in the container.
        command: Option<Vec<String>>,
        /// Whether to run in interactive mode (TTY).
        #[serde(default)]
        interactive: bool,
        /// Whether to run in detached mode.
        #[serde(default)]
        detached: bool,
    },
}

/// Responses sent from the Server to the CLI.
///
/// # Example
/// ```rust
/// use locald_core::ipc::IpcResponse;
/// let res = IpcResponse::Ok;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum IpcResponse {
    /// Response to Ping.
    Pong,
    /// Response to GetVersion.
    Version(String),
    /// Response to GetDaemonIdentity.
    DaemonIdentity(DaemonIdentity),
    /// Generic success response.
    Ok,
    /// Response to Status request.
    Status(Vec<ServiceStatus>),
    /// Generic error response.
    Error(String),
    /// Response to AiContext request.
    AiContext(String),
    /// Response to AiSchema request.
    AiSchema(String),
    /// Response to RegistryList request.
    RegistryList(Vec<crate::registry::ProjectEntry>),
    /// Response to RegistryClean request.
    RegistryCleaned(usize),
    /// Response to GetServiceEnv request.
    ServiceEnv(std::collections::HashMap<String, String>),
    /// Response to ProjectStatus request.
    ProjectStatus(ProjectStatusInfo),
    /// Response to ProjectList request.
    ProjectList(Vec<ProjectListEntry>),
}

/// Events broadcasted by the Server.
///
/// # Example
/// ```rust
/// use locald_core::ipc::{Event, LogEntry, LogStream};
///
/// let event = Event::Log(LogEntry {
///     timestamp: 123,
///     service: "web".to_string(),
///     stream: LogStream::Stdout,
///     message: "hello".to_string(),
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    /// A new log entry.
    Log(LogEntry),
    /// A service status update.
    ServiceUpdate(ServiceStatus),
    /// Service metrics update.
    Metrics(ServiceMetrics),
}

/// Events emitted during the boot process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "data")]
pub enum BootEvent {
    /// A step has started.
    StepStarted { id: String, description: String },
    /// Progress update for a step.
    StepProgress { id: String, message: String },
    /// A step has finished.
    StepFinished {
        id: String,
        result: Result<(), String>,
    },
    /// Log output associated with a step.
    Log {
        id: String,
        line: String,
        stream: LogStream,
    },
}

impl ServiceStatus {
    /// Create a new ServiceStatus with default values.
    ///
    /// # Example
    /// ```
    /// use locald_core::ipc::ServiceStatus;
    /// use locald_core::state::ServiceState;
    ///
    /// let status = ServiceStatus::new("web", ServiceState::Running);
    /// assert_eq!(status.name, "web");
    /// assert_eq!(status.status, ServiceState::Running);
    /// ```
    pub fn new(name: impl Into<String>, status: ServiceState) -> Self {
        Self {
            name: name.into(),
            service_type: ServiceType::default(),
            pid: None,
            port: None,
            status,
            url: None,
            connection_url: None,
            domain: None,
            health_status: HealthStatus::Unknown,
            health_source: HealthSource::None,
            path: None,
            workspace: None,
            constellation: None,
            warnings: Vec::new(),
        }
    }
}
