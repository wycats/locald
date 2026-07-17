#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

use locald_core::ServiceState;
use locald_core::ipc::ServiceStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DaemonStatus {
    Checking,
    NotRunning,
    Running { total: usize, running: usize },
}

impl DaemonStatus {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn from_services(services: &[ServiceStatus]) -> Self {
        let running = services
            .iter()
            .filter(|service| service.status == ServiceState::Running)
            .count();
        DaemonStatus::Running {
            total: services.len(),
            running,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            DaemonStatus::Checking => "Status: checking...".to_string(),
            DaemonStatus::NotRunning => "Status: not running".to_string(),
            DaemonStatus::Running { total, running } => {
                if *total == 0 {
                    "Status: running (no services)".to_string()
                } else {
                    format!("Status: {running}/{total} running")
                }
            }
        }
    }
}

/// Health checks that run without root — detect problems the agent can surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HealthStatus {
    pub(crate) helper_installed: bool,
    pub(crate) port_80_reachable: bool,
    pub(crate) ca_trusted: bool,
}

impl HealthStatus {
    pub(crate) fn is_healthy(&self) -> bool {
        self.helper_installed && self.port_80_reachable && self.ca_trusted
    }

    pub(crate) fn warning_label(&self) -> Option<String> {
        let mut problems = Vec::new();
        if !self.helper_installed {
            problems.push("privileged helper not installed");
        } else if !self.port_80_reachable {
            problems.push("port 80 not reachable");
        }
        if !self.ca_trusted {
            problems.push("HTTPS not trusted");
        }
        if problems.is_empty() {
            None
        } else {
            Some(format!("⚠ {}", problems.join(", ")))
        }
    }
}

/// Combined update from the polling thread.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct PollUpdate {
    pub(crate) daemon: DaemonStatus,
    pub(crate) health: HealthStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_status_labels() {
        assert_eq!(DaemonStatus::Checking.label(), "Status: checking...");
        assert_eq!(DaemonStatus::NotRunning.label(), "Status: not running");
        assert_eq!(
            DaemonStatus::Running {
                total: 3,
                running: 2
            }
            .label(),
            "Status: 2/3 running"
        );
        assert_eq!(
            DaemonStatus::Running {
                total: 0,
                running: 0
            }
            .label(),
            "Status: running (no services)"
        );
    }

    #[test]
    fn daemon_status_from_services_counts_only_running_services() {
        let services = vec![
            ServiceStatus::new("web", ServiceState::Running),
            ServiceStatus::new("worker", ServiceState::Stopped),
            ServiceStatus::new("db", ServiceState::Building),
        ];

        assert_eq!(
            DaemonStatus::from_services(&services),
            DaemonStatus::Running {
                total: 3,
                running: 1
            }
        );
    }

    #[test]
    fn health_all_good() {
        let health = HealthStatus {
            helper_installed: true,
            port_80_reachable: true,
            ca_trusted: true,
        };
        assert!(health.is_healthy());
        assert_eq!(health.warning_label(), None);
    }

    #[test]
    fn health_helper_missing() {
        let health = HealthStatus {
            helper_installed: false,
            port_80_reachable: true,
            ca_trusted: true,
        };
        assert!(!health.is_healthy());
        assert_eq!(
            health.warning_label(),
            Some("⚠ privileged helper not installed".to_string())
        );
    }

    #[test]
    fn health_port_unreachable() {
        let health = HealthStatus {
            helper_installed: true,
            port_80_reachable: false,
            ca_trusted: true,
        };
        assert!(!health.is_healthy());
        assert_eq!(
            health.warning_label(),
            Some("⚠ port 80 not reachable".to_string())
        );
    }

    #[test]
    fn health_ca_untrusted() {
        let health = HealthStatus {
            helper_installed: true,
            port_80_reachable: true,
            ca_trusted: false,
        };
        assert!(!health.is_healthy());
        assert_eq!(
            health.warning_label(),
            Some("⚠ HTTPS not trusted".to_string())
        );
    }

    #[test]
    fn health_multiple_problems() {
        let health = HealthStatus {
            helper_installed: false,
            port_80_reachable: false,
            ca_trusted: false,
        };
        assert!(!health.is_healthy());
        assert_eq!(
            health.warning_label(),
            Some("⚠ privileged helper not installed, HTTPS not trusted".to_string())
        );
    }

    #[test]
    fn health_helper_missing_suppresses_port_unreachable_warning() {
        let health = HealthStatus {
            helper_installed: false,
            port_80_reachable: false,
            ca_trusted: true,
        };
        assert!(!health.is_healthy());
        assert_eq!(
            health.warning_label(),
            Some("⚠ privileged helper not installed".to_string())
        );
    }
}
