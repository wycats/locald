use bollard::Docker;
use bollard::container::InspectContainerOptions;
use bollard::exec::CreateExecOptions;
use locald_core::config::{HealthCheckConfig, ProbeType, ServiceConfig};
use locald_core::state::{HealthSource, HealthStatus};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Debug)]
pub(crate) struct HealthMonitor {
    docker: Option<Arc<Docker>>,
    services: Arc<Mutex<std::collections::HashMap<String, crate::manager::Service>>>,
    event_sender: tokio::sync::broadcast::Sender<locald_core::ipc::Event>,
    proxy_ports: Arc<Mutex<(Option<u16>, Option<u16>)>>,
}

impl HealthMonitor {
    pub(crate) fn new(
        docker: Option<Arc<Docker>>,
        services: Arc<Mutex<std::collections::HashMap<String, crate::manager::Service>>>,
        event_sender: tokio::sync::broadcast::Sender<locald_core::ipc::Event>,
        proxy_ports: Arc<Mutex<(Option<u16>, Option<u16>)>>,
    ) -> Self {
        Self {
            docker,
            services,
            event_sender,
            proxy_ports,
        }
    }

    pub(crate) fn spawn_check(
        &self,
        name: String,
        config: &ServiceConfig,
        port: Option<u16>,
        pid: Option<u32>,
        container_id: Option<String>,
        has_docker_healthcheck: bool,
        cwd: Option<std::path::PathBuf>,
    ) {
        // Spawn port mismatch detector if we have a PID and an expected port
        if let (Some(pid), Some(expected_port)) = (pid, port) {
            self.spawn_port_mismatch_monitor(name.clone(), pid, expected_port);
        }

        if let Some(hc) = config.health_check() {
            match hc {
                HealthCheckConfig::Command(cmd) => {
                    self.spawn_command_monitor(name, cmd.clone(), container_id, cwd);
                }
                HealthCheckConfig::Probe(probe) => match probe.kind {
                    ProbeType::Http => {
                        if let Some(p) = port {
                            let path = probe.path.as_deref().unwrap_or("/");
                            self.spawn_http_monitor(name, p, path.to_string());
                        }
                    }
                    ProbeType::Tcp => {
                        if let Some(p) = port {
                            self.spawn_tcp_monitor(name, p);
                        }
                    }
                    ProbeType::Command => {
                        if let Some(cmd) = &probe.command {
                            self.spawn_command_monitor(name, cmd.clone(), container_id, cwd);
                        }
                    }
                },
            }
        } else if has_docker_healthcheck {
            if let Some(cid) = container_id {
                self.spawn_docker_monitor(name, cid);
            }
        } else if let Some(p) = port {
            self.spawn_tcp_monitor(name, p);
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
                            _ => break, // Service stopped
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
                                .map(|p| p.to_string())
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
                            Some(crate::manager::ProcessManager::get_service_domain(
                                name,
                                &service.config.project,
                            )),
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
                if service.warnings != warnings {
                    info!("Service {} warnings changed: {:?}", name, warnings);
                    service.warnings = warnings.clone();

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
                            Some(crate::manager::ProcessManager::get_service_domain(
                                name,
                                &service.config.project,
                            )),
                            Some(service.path.clone()),
                            proxy_ports,
                            snapshot,
                            service.service_config.clone(),
                            service.config.project.workspace.clone(),
                            service.config.project.constellation.clone(),
                            service.warnings.clone(),
                            service.health_status.clone(),
                            service.health_source.clone(),
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

    fn spawn_http_monitor(&self, name: String, port: u16, path: String) {
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
                if locald_utils::probe::check_http(&url).await {
                    monitor
                        .update_health(&name, HealthStatus::Healthy, HealthSource::Http)
                        .await;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
    }

    fn spawn_command_monitor(
        &self,
        name: String,
        command: String,
        container_id: Option<String>,
        cwd: Option<std::path::PathBuf>,
    ) {
        let monitor = self.clone();
        let docker = self.docker.clone();

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

                let success = if let Some(cid) = &container_id {
                    if let Some(docker) = &docker {
                        let config = CreateExecOptions {
                            cmd: Some(vec!["sh", "-c", &command]),
                            attach_stdout: Some(false),
                            attach_stderr: Some(false),
                            ..Default::default()
                        };

                        if let Ok(exec) = docker.create_exec(cid, config).await {
                            if (docker.start_exec(&exec.id, None).await).is_ok() {
                                let mut retries = 0;
                                let mut exit_code = None;
                                loop {
                                    if let Ok(inspect) = docker.inspect_exec(&exec.id).await {
                                        if inspect.running == Some(false) {
                                            exit_code = inspect.exit_code;
                                            break;
                                        }
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                    retries += 1;
                                    if retries > 50 {
                                        break;
                                    }
                                }
                                exit_code == Some(0)
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    locald_utils::probe::check_command(&command, cwd.as_deref()).await
                };

                if success {
                    monitor
                        .update_health(&name, HealthStatus::Healthy, HealthSource::Command)
                        .await;
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
    }

    fn spawn_docker_monitor(&self, name: String, container_id: String) {
        let monitor = self.clone();
        let Some(docker) = self.docker.clone() else {
            return;
        };

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                match docker
                    .inspect_container(&container_id, None::<InspectContainerOptions>)
                    .await
                {
                    Ok(inspect) => {
                        if let Some(state) = inspect.state {
                            if let Some(health) = state.health {
                                let status = match health.status {
                                    Some(bollard::models::HealthStatusEnum::HEALTHY) => {
                                        HealthStatus::Healthy
                                    }
                                    Some(bollard::models::HealthStatusEnum::UNHEALTHY) => {
                                        HealthStatus::Unhealthy
                                    }
                                    Some(bollard::models::HealthStatusEnum::STARTING) => {
                                        HealthStatus::Starting
                                    }
                                    _ => HealthStatus::Unknown,
                                };

                                monitor
                                    .update_health(&name, status, HealthSource::Docker)
                                    .await;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    fn spawn_tcp_monitor(&self, name: String, assigned_port: u16) {
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
                    locald_utils::probe::check_tcp(&format!("127.0.0.1:{assigned_port}")).await;
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

                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
    }
}

impl Clone for HealthMonitor {
    fn clone(&self) -> Self {
        Self {
            docker: self.docker.clone(),
            services: self.services.clone(),
            event_sender: self.event_sender.clone(),
            proxy_ports: self.proxy_ports.clone(),
        }
    }
}
