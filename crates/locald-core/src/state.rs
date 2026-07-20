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

/// Platform-native process birth identity captured while locald still owns a
/// running service process.
///
/// Each variant uses the highest-resolution stable value exposed by the host
/// platform. The value is paired with the recorded PID and process group before
/// locald authorizes a signal after a daemon restart.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub enum PersistedProcessBirth {
    /// Darwin process creation time from `proc_bsdinfo`.
    #[serde(rename = "macos")]
    Macos {
        start_seconds: u64,
        start_microseconds: u64,
    },
    /// Linux boot identity plus `/proc/<pid>/stat` start ticks.
    Linux { boot_id: String, start_ticks: u64 },
}

/// OS identity captured while locald still owns a running service process.
///
/// A PID alone is never sufficient authorization to signal a process after a
/// daemon restart because the operating system can reuse it. These fields let
/// startup reconciliation prove that the live process is the one locald
/// recorded before sending either a graceful or forceful signal.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PersistedProcessIdentity {
    /// High-resolution birth authority. `None` exists only so state written by
    /// an older locald remains decodable; it never authorizes a live process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birth: Option<PersistedProcessBirth>,
    pub process_group_id: i32,
    /// Executable observed at spawn time for diagnostics. A normal `exec`
    /// changes this path without changing process identity, so cleanup never
    /// treats it as authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PersistedServiceState {
    pub name: String,
    pub config: LocaldConfig,
    pub path: PathBuf,
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_identity: Option<PersistedProcessIdentity>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn service_state(process_identity: Option<PersistedProcessIdentity>) -> PersistedServiceState {
        PersistedServiceState {
            name: "example:web".to_owned(),
            config: LocaldConfig::default(),
            path: PathBuf::from("/tmp/example"),
            pid: Some(42),
            process_identity,
            container_id: None,
            port: Some(3000),
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
            health_source: HealthSource::Tcp,
        }
    }

    #[test]
    fn legacy_service_state_without_process_identity_remains_readable() {
        let mut value = serde_json::to_value(service_state(None)).expect("serialize service state");
        value
            .as_object_mut()
            .expect("service state is an object")
            .remove("process_identity");

        let decoded: PersistedServiceState =
            serde_json::from_value(value).expect("deserialize legacy service state");
        assert!(decoded.process_identity.is_none());
        assert_eq!(decoded.pid, Some(42));
    }

    #[test]
    fn process_identity_round_trip_preserves_cleanup_authority() {
        for birth in [
            PersistedProcessBirth::Macos {
                start_seconds: 1_234,
                start_microseconds: 567_890,
            },
            PersistedProcessBirth::Linux {
                boot_id: "test-boot".to_owned(),
                start_ticks: 98_765,
            },
        ] {
            let identity = PersistedProcessIdentity {
                birth: Some(birth),
                process_group_id: 42,
                executable: Some(PathBuf::from("/bin/example")),
            };
            let encoded = serde_json::to_vec(&service_state(Some(identity.clone())))
                .expect("serialize fingerprinted service state");
            let decoded: PersistedServiceState =
                serde_json::from_slice(&encoded).expect("deserialize fingerprinted service state");

            assert_eq!(decoded.process_identity, Some(identity));
        }
    }

    #[test]
    fn legacy_seconds_only_process_identity_decodes_without_cleanup_authority() {
        let value = serde_json::json!({
            "name": "example:web",
            "config": LocaldConfig::default(),
            "path": "/tmp/example",
            "pid": 42,
            "process_identity": {
                "start_time": 1234,
                "process_group_id": 42,
                "executable": "/bin/example"
            },
            "container_id": null,
            "port": 3000,
            "status": "running",
            "health_status": "Healthy",
            "health_source": "Tcp"
        });

        let decoded: PersistedServiceState =
            serde_json::from_value(value).expect("deserialize legacy process identity");
        let identity = decoded
            .process_identity
            .expect("preserve legacy process identity diagnostics");

        assert!(identity.birth.is_none());
        assert_eq!(identity.process_group_id, 42);
        assert_eq!(identity.executable, Some(PathBuf::from("/bin/example")));
    }
}
