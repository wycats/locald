//! Runtime state and health metadata for services.
//!
//! These enums model the high-level lifecycle and health signals reported by
//! service controllers.
//!
//! # Examples
//! ```rust
//! use locald_core::state::{HealthStatus, ServiceState};
//!
//! let status = ServiceState::Stopped;
//! let next = ServiceState::Running;
//! assert!(status != next);
//! assert_eq!(HealthStatus::Unknown.to_string(), "unknown");
//! ```
use crate::config::LocaldConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Current health of a service as observed by locald.
///
/// Health typically progresses from `Unknown` to `Starting`, and eventually to
/// `Healthy` or `Unhealthy` once checks complete.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, JsonSchema)]
pub enum HealthStatus {
    /// No health information is available yet.
    #[default]
    Unknown,
    /// Health checks have begun but are not yet conclusive.
    Starting,
    /// The service has passed its health checks.
    Healthy,
    /// The service failed health checks.
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "unknown"),
            Self::Starting => write!(f, "starting"),
            Self::Healthy => write!(f, "healthy"),
            Self::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, JsonSchema)]
pub enum HealthSource {
    #[default]
    None,
    Notify,
    Tcp,
    Explicit,
    Http,
    Command,
}

impl std::fmt::Display for HealthSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Notify => write!(f, "notify"),
            Self::Tcp => write!(f, "tcp"),
            Self::Explicit => write!(f, "explicit"),
            Self::Http => write!(f, "http"),
            Self::Command => write!(f, "command"),
        }
    }
}

/// High-level lifecycle state of a service.
///
/// Typical transitions are: `Stopped` -> `Building` -> `Running`, with
/// `Running` -> `Stopped` when a service is shut down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    /// The service process is running and should be reachable.
    Running,
    /// The service is not currently running.
    Stopped,
    /// The service is preparing or building before start.
    Building,
}

impl std::fmt::Display for ServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Building => write!(f, "building"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PersistedServiceState {
    pub name: String,
    pub config: LocaldConfig,
    pub path: PathBuf,
    pub pid: Option<u32>,
    pub container_id: Option<String>,
    pub port: Option<u16>,
    pub status: ServiceState,
    #[serde(default)]
    pub health_status: HealthStatus,
    #[serde(default)]
    pub health_source: HealthSource,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ServerState {
    pub services: Vec<PersistedServiceState>,
}
