use locald_core::SharedDomainIndex;
use locald_core::config::{
    DEFAULT_HEALTH_CHECK_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_TIMEOUT_SECS, HealthCheckConfig,
    ProbeType, ServiceConfig,
};
use locald_core::state::{HealthSource, HealthStatus};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Debug)]
pub(crate) struct HealthMonitor {
    services: Arc<Mutex<std::collections::HashMap<String, crate::manager::Service>>>,
    event_sender: tokio::sync::broadcast::Sender<locald_core::ipc::Event>,
    proxy_ports: Arc<Mutex<(Option<u16>, Option<u16>)>>,
    domain_index: SharedDomainIndex,
}

impl HealthMonitor {
    pub(crate) fn new(
        services: Arc<Mutex<std::collections::HashMap<String, crate::manager::Service>>>,
        event_sender: tokio::sync::broadcast::Sender<locald_core::ipc::Event>,
        proxy_ports: Arc<Mutex<(Option<u16>, Option<u16>)>>,
        domain_index: SharedDomainIndex,
    ) -> Self {
        Self {
            services,
            event_sender,
            proxy_ports,
            domain_index,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_check(
        &self,
        name: String,
        config: &ServiceConfig,
        port: Option<u16>,
        pid: Option<u32>,
        _container_id: Option<String>,
        cwd: Option<std::path::PathBuf>,
    ) {
        let default_interval = Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS);
        let default_timeout = Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS);

        // Spawn port mismatch detector if we have a PID and an expected port
        if let (Some(pid), Some(expected_port)) = (pid, port) {
            self.spawn_port_mismatch_monitor(name.clone(), pid, expected_port);
        }

        if let Some(hc) = config.health_check() {
            let (interval, timeout) = match hc {
                HealthCheckConfig::Command(_) => (default_interval, default_timeout),
                HealthCheckConfig::Probe(probe) => {
                    (probe.interval_duration(), probe.timeout_duration())
                }
            };
            match hc {
                HealthCheckConfig::Command(cmd) => {
                    self.spawn_command_monitor(name, cmd.clone(), cwd, interval, timeout);
                }
                HealthCheckConfig::Probe(probe) => match probe.kind {
                    ProbeType::Http => {
                        if let Some(p) = port {
                            let path = probe.path.as_deref().unwrap_or("/");
                            self.spawn_http_monitor(name, p, path.to_string(), interval, timeout);
                        }
                    }
                    ProbeType::Tcp => {
                        if let Some(p) = port {
                            self.spawn_tcp_monitor(name, p, interval, timeout);
                        }
                    }
                    ProbeType::Command => {
                        if let Some(cmd) = &probe.command {
                            self.spawn_command_monitor(name, cmd.clone(), cwd, interval, timeout);
                        }
                    }
                },
            }
        } else if let Some(p) = port {
            self.spawn_tcp_monitor(name, p, default_interval, default_timeout);
        }
    }

    fn spawn_port_mismatch_monitor(&self, name: String, pid: u32, expected_port: u16) {
        let monitor = self.clone();
        tokio::spawn(async move {
            // Give the service some time to start listening
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            loop {
                // Check if service is still running and managed by us
                {
                    let services = monitor.services.lock().await;
                    if let Some(service) = services.get(&name) {
                        match &service.runtime_state {
                            crate::manager::ServiceRuntime::Controller(_) => {
                                // Still running
                            }
                            crate::manager::ServiceRuntime::None => break, // Service stopped
                        }
                    } else {
                        break; // Service removed
                    }
                }

                match locald_utils::discovery::find_listening_ports(pid).await {
                    Ok(ports) => {
                        let mut warnings = Vec::new();
                        if !ports.contains(&expected_port) && !ports.is_empty() {
                            // Sort ports for consistent message
                            let mut sorted_ports = ports.clone();
                            sorted_ports.sort_unstable();

                            let ports_str = sorted_ports
                                .iter()
                                .map(std::string::ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ");

                            warnings.push(format!(
                                "Service is listening on port(s) {} but configured for {}. Update locald.toml or the service configuration.",
                                ports_str, expected_port
                            ));
                        }

                        monitor.update_warnings(&name, warnings).await;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to check ports for service {}: {}", name, e);
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }

    async fn update_health(&self, name: &str, status: HealthStatus, source: HealthSource) {
        let (changed, snapshot_info) = {
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(name) {
                if service.health_status != status || service.health_source != source {
                    info!(
                        "Service {} health changed to {:?} (source: {:?})",
                        name, status, source
                    );
                    service.health_status = status;
                    service.health_source = source;

                    let proxy_ports = { *self.proxy_ports.lock().await };

                    let snapshot = match &service.runtime_state {
                        crate::manager::ServiceRuntime::Controller(c) => {
                            crate::manager::RuntimeSnapshot::Controller(c.clone())
                        }
                        crate::manager::ServiceRuntime::None => {
                            crate::manager::RuntimeSnapshot::Static {
                                is_running: false,
                                pid: None,
                                port: None,
                            }
                        }
                    };

                    (
                        true,
                        Some((
                            self.domain_index
                                .snapshot()
                                .domain_for_service(name)
                                .map(ToString::to_string),
                            Some(service.path.clone()),
                            proxy_ports,
                            snapshot,
                            service.service_config.clone(),
                            service.config.project.workspace.clone(),
                            service.config.project.constellation.clone(),
                            service.warnings.clone(),
                        )),
                    )
                } else {
                    (false, None)
                }
            } else {
                (false, None)
            }
        };

        if changed {
            if let Some((
                domain,
                path,
                proxy_ports,
                snapshot,
                service_config,
                workspace,
                constellation,
                warnings,
            )) = snapshot_info
            {
                let status = crate::manager::ProcessManager::build_service_status(
                    name.to_string(),
                    domain,
                    path,
                    proxy_ports,
                    status,
                    source,
                    snapshot,
                    Some(&service_config),
                    workspace,
                    constellation,
                    warnings,
                )
                .await;

                let _ = self
                    .event_sender
                    .send(locald_core::ipc::Event::ServiceUpdate(status));
            }
        }
    }

    async fn update_warnings(&self, name: &str, warnings: Vec<String>) {
        let (changed, snapshot_info) = {
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(name) {
                if service.warnings == warnings {
                    (false, None)
                } else {
                    info!("Service {} warnings changed: {:?}", name, warnings);
                    service.warnings.clone_from(&warnings);

                    let proxy_ports = { *self.proxy_ports.lock().await };

                    let snapshot = match &service.runtime_state {
                        crate::manager::ServiceRuntime::Controller(c) => {
                            crate::manager::RuntimeSnapshot::Controller(c.clone())
                        }
                        crate::manager::ServiceRuntime::None => {
                            crate::manager::RuntimeSnapshot::Static {
                                is_running: false,
                                pid: None,
                                port: None,
                            }
                        }
                    };

                    (
                        true,
                        Some((
                            self.domain_index
                                .snapshot()
                                .domain_for_service(name)
                                .map(ToString::to_string),
                            Some(service.path.clone()),
                            proxy_ports,
                            snapshot,
                            service.service_config.clone(),
                            service.config.project.workspace.clone(),
                            service.config.project.constellation.clone(),
                            service.warnings.clone(),
                            service.health_status,
                            service.health_source,
                        )),
                    )
                }
            } else {
                (false, None)
            }
        };

        if changed {
            if let Some((
                domain,
                path,
                proxy_ports,
                snapshot,
                service_config,
                workspace,
                constellation,
                warnings,
                health_status,
                health_source,
            )) = snapshot_info
            {
                let status = crate::manager::ProcessManager::build_service_status(
                    name.to_string(),
                    domain,
                    path,
                    proxy_ports,
                    health_status,
                    health_source,
                    snapshot,
                    Some(&service_config),
                    workspace,
                    constellation,
                    warnings,
                )
                .await;

                let _ = self
                    .event_sender
                    .send(locald_core::ipc::Event::ServiceUpdate(status));
            }
        }
    }

    fn spawn_http_monitor(
        &self,
        name: String,
        port: u16,
        path: String,
        interval: Duration,
        timeout: Duration,
    ) {
        let monitor = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let url = format!("http://127.0.0.1:{port}{path}");

            loop {
                {
                    let services = monitor.services.lock().await;
                    if let Some(service) = services.get(&name) {
                        if service.health_status == HealthStatus::Healthy {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if locald_utils::probe::check_http(&url, timeout).await {
                    monitor
                        .update_health(&name, HealthStatus::Healthy, HealthSource::Http)
                        .await;
                    break;
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    fn spawn_command_monitor(
        &self,
        name: String,
        command: String,
        cwd: Option<std::path::PathBuf>,
        interval: Duration,
        timeout: Duration,
    ) {
        let monitor = self.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            loop {
                {
                    let services = monitor.services.lock().await;
                    if let Some(service) = services.get(&name) {
                        if service.health_status == HealthStatus::Healthy {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                let success =
                    locald_utils::probe::check_command(&command, cwd.as_deref(), timeout).await;

                if success {
                    monitor
                        .update_health(&name, HealthStatus::Healthy, HealthSource::Command)
                        .await;
                    break;
                }

                tokio::time::sleep(interval).await;
            }
        });
    }

    fn spawn_tcp_monitor(
        &self,
        name: String,
        assigned_port: u16,
        interval: Duration,
        timeout: Duration,
    ) {
        info!(
            "Starting TCP monitor for {} on port {}",
            name, assigned_port
        );
        let monitor = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            loop {
                let pid = {
                    let services = monitor.services.lock().await;
                    if let Some(service) = services.get(&name) {
                        if service.health_status == HealthStatus::Healthy {
                            info!("Service {} is already healthy, stopping monitor", name);
                            break;
                        }
                        match &service.runtime_state {
                            crate::manager::ServiceRuntime::Controller(c) => {
                                c.lock().await.read_state().await.pid
                            }
                            crate::manager::ServiceRuntime::None => None,
                        }
                    } else {
                        info!(
                            "Service {} not found in services map, stopping monitor",
                            name
                        );
                        break;
                    }
                };

                info!("About to probe {} on {}", name, assigned_port);
                let result =
                    locald_utils::probe::check_tcp(&format!("127.0.0.1:{assigned_port}"), timeout)
                        .await;
                info!(
                    "Probing {} on {}... Success: {}",
                    name, assigned_port, result
                );

                if result {
                    monitor
                        .update_health(&name, HealthStatus::Healthy, HealthSource::Tcp)
                        .await;
                    break;
                }

                if let Some(pid) = pid {
                    if let Ok(ports) = locald_utils::discovery::find_listening_ports(pid).await {
                        if let Some(&found_port) = ports.first() {
                            info!("Service {} discovered on port {}", name, found_port);
                            // Port update removed as it requires Controller support
                            monitor
                                .update_health(&name, HealthStatus::Healthy, HealthSource::Tcp)
                                .await;
                            break;
                        }
                    }
                }

                tokio::time::sleep(interval).await;
            }
        });
    }
}

impl Clone for HealthMonitor {
    fn clone(&self) -> Self {
        Self {
            services: self.services.clone(),
            event_sender: self.event_sender.clone(),
            proxy_ports: self.proxy_ports.clone(),
            domain_index: self.domain_index.clone(),
        }
    }
}
