use locald_core::config::{
    DEFAULT_HEALTH_CHECK_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_TIMEOUT_SECS, HealthCheckConfig,
    ProbeType, ServiceConfig, TypedServiceConfig,
};
use locald_core::state::{HealthSource, HealthStatus};
use locald_core::{ProjectInstanceId, SharedDomainIndex};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ControllerIdentity {
    instance_id: ProjectInstanceId,
    controller_generation: u64,
}

/// The single readiness contract that must be satisfied before a service is
/// exposed as ready or a dependent service may start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReadinessRequirement {
    ExplicitHttp {
        port: u16,
        path: String,
        interval: Duration,
        timeout: Duration,
    },
    ExplicitTcp {
        port: u16,
        interval: Duration,
        timeout: Duration,
    },
    ExplicitCommand {
        command: String,
        interval: Duration,
        timeout: Duration,
    },
    AssignedPortTcp {
        port: u16,
    },
    ControllerAndAssignedPortTcp {
        port: u16,
    },
    ProcessRunning,
}

impl ReadinessRequirement {
    pub(crate) fn service_requires_port(config: &ServiceConfig) -> bool {
        let explicit_network_probe = matches!(
            config.health_check(),
            Some(HealthCheckConfig::Probe(probe))
                if matches!(probe.kind, ProbeType::Http | ProbeType::Tcp)
        );
        !matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
            || config.port().is_some()
            || explicit_network_probe
    }

    pub(crate) fn for_service(config: &ServiceConfig, port: Option<u16>) -> anyhow::Result<Self> {
        let default_interval = Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS);
        let default_timeout = Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS);

        if let Some(health_check) = config.health_check() {
            return match health_check {
                HealthCheckConfig::Command(command) => {
                    anyhow::ensure!(
                        !command.trim().is_empty(),
                        "explicit command readiness check must not be empty"
                    );
                    Ok(Self::ExplicitCommand {
                        command: command.clone(),
                        interval: default_interval,
                        timeout: default_timeout,
                    })
                }
                HealthCheckConfig::Probe(probe) => {
                    let interval = probe.interval_duration();
                    let timeout = probe.timeout_duration();
                    match probe.kind {
                        ProbeType::Http => Ok(Self::ExplicitHttp {
                            port: port.ok_or_else(|| {
                                anyhow::anyhow!(
                                    "explicit HTTP readiness check requires an assigned service port"
                                )
                            })?,
                            path: probe.path.clone().unwrap_or_else(|| "/".to_owned()),
                            interval,
                            timeout,
                        }),
                        ProbeType::Tcp => Ok(Self::ExplicitTcp {
                            port: port.ok_or_else(|| {
                                anyhow::anyhow!(
                                    "explicit TCP readiness check requires an assigned service port"
                                )
                            })?,
                            interval,
                            timeout,
                        }),
                        ProbeType::Command => {
                            let command = probe.command.as_ref().ok_or_else(|| {
                                anyhow::anyhow!(
                                    "explicit command readiness probe requires `command`"
                                )
                            })?;
                            anyhow::ensure!(
                                !command.trim().is_empty(),
                                "explicit command readiness probe must not be empty"
                            );
                            Ok(Self::ExplicitCommand {
                                command: command.clone(),
                                interval,
                                timeout,
                            })
                        }
                    }
                }
            };
        }

        match config {
            ServiceConfig::Typed(TypedServiceConfig::Worker(_)) => port.map_or_else(
                || Ok(Self::ProcessRunning),
                |port| Ok(Self::AssignedPortTcp { port }),
            ),
            ServiceConfig::Typed(
                TypedServiceConfig::Container(_) | TypedServiceConfig::Site(_),
            ) => Ok(Self::ControllerAndAssignedPortTcp {
                port: port.ok_or_else(|| {
                    anyhow::anyhow!(
                        "endpoint service has no assigned port for controller and TCP readiness"
                    )
                })?,
            }),
            // Exec and legacy services are endpoint services: their common
            // config receives an assigned PORT even when `port` is omitted.
            // Commands that intentionally have no endpoint use `type = "worker"`.
            ServiceConfig::Typed(TypedServiceConfig::Exec(_) | TypedServiceConfig::Postgres(_))
            | ServiceConfig::Legacy(_) => Ok(Self::AssignedPortTcp {
                port: port.ok_or_else(|| {
                    anyhow::anyhow!("portful service has no assigned port for TCP readiness")
                })?,
            }),
        }
    }

    pub(crate) fn description(&self) -> String {
        match self {
            Self::ExplicitHttp { path, .. } => {
                format!("explicit HTTP probe at `{path}` on the assigned endpoint")
            }
            Self::ExplicitTcp { .. } => "explicit TCP probe on the assigned endpoint".to_owned(),
            Self::ExplicitCommand { .. } => "explicit command probe".to_owned(),
            Self::AssignedPortTcp { .. } => "TCP probe on the assigned endpoint".to_owned(),
            Self::ControllerAndAssignedPortTcp { .. } => {
                "controller health and TCP probe on the assigned endpoint".to_owned()
            }
            Self::ProcessRunning => "owned process liveness".to_owned(),
        }
    }

    pub(crate) const fn health_source(&self) -> HealthSource {
        match self {
            Self::ExplicitHttp { .. } => HealthSource::Http,
            Self::ExplicitTcp { .. }
            | Self::AssignedPortTcp { .. }
            | Self::ControllerAndAssignedPortTcp { .. } => HealthSource::Tcp,
            Self::ExplicitCommand { .. } => HealthSource::Command,
            Self::ProcessRunning => HealthSource::Explicit,
        }
    }
}

impl HealthMonitor {
    fn matches_controller(
        service: &crate::manager::Service,
        controller: ControllerIdentity,
    ) -> bool {
        service.instance_id == controller.instance_id
            && service.controller_generation == controller.controller_generation
            && matches!(
                service.runtime_state,
                crate::manager::ServiceRuntime::Controller(_)
            )
    }

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
        instance_id: ProjectInstanceId,
        controller_generation: u64,
        requirement: ReadinessRequirement,
        port: Option<u16>,
        pid: Option<u32>,
        _container_id: Option<String>,
        cwd: Option<std::path::PathBuf>,
    ) {
        let controller = ControllerIdentity {
            instance_id,
            controller_generation,
        };
        // Spawn port mismatch detector if we have a PID and an expected port
        if let (Some(pid), Some(expected_port)) = (pid, port) {
            self.spawn_port_mismatch_monitor(name.clone(), controller, pid, expected_port);
        }

        match requirement {
            ReadinessRequirement::ExplicitHttp {
                port,
                path,
                interval,
                timeout,
            } => self.spawn_http_monitor(name, controller, port, path, interval, timeout),
            ReadinessRequirement::ExplicitTcp {
                port,
                interval,
                timeout,
            } => self.spawn_tcp_monitor(name, controller, port, interval, timeout, false),
            ReadinessRequirement::ExplicitCommand {
                command,
                interval,
                timeout,
            } => self.spawn_command_monitor(name, controller, command, cwd, interval, timeout),
            ReadinessRequirement::AssignedPortTcp { port } => self.spawn_tcp_monitor(
                name,
                controller,
                port,
                Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS),
                Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS),
                false,
            ),
            ReadinessRequirement::ControllerAndAssignedPortTcp { port } => self.spawn_tcp_monitor(
                name,
                controller,
                port,
                Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS),
                Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS),
                true,
            ),
            ReadinessRequirement::ProcessRunning => {}
        }
    }

    fn spawn_port_mismatch_monitor(
        &self,
        name: String,
        controller: ControllerIdentity,
        pid: u32,
        expected_port: u16,
    ) {
        let monitor = self.clone();
        tokio::spawn(async move {
            // Give the service some time to start listening
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            loop {
                // Check if service is still running and managed by us
                {
                    let services = monitor.services.lock().await;
                    if let Some(service) = services
                        .get(&name)
                        .filter(|service| Self::matches_controller(service, controller))
                    {
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

                        monitor.update_warnings(&name, controller, warnings).await;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to check ports for service {}: {}", name, e);
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }

    async fn update_health(
        &self,
        name: &str,
        controller: ControllerIdentity,
        status: HealthStatus,
        source: HealthSource,
    ) {
        let (changed, snapshot_info) = {
            let mut services = self.services.lock().await;
            if let Some(service) = services
                .get_mut(name)
                .filter(|service| Self::matches_controller(service, controller))
                .filter(|service| service.health_status == HealthStatus::Starting)
            {
                if service.health_status != status || service.health_source != source {
                    info!(
                        "Service {} health changed to {:?} (source: {:?})",
                        name, status, source
                    );
                    service.health_status = status;
                    service.health_source = source;
                    let projection_generation =
                        crate::manager::ProcessManager::advance_service_projection(service);

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
                            projection_generation,
                            self.domain_index
                                .snapshot()
                                .domain_for_service(service.instance_id, name)
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
                projection_generation,
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

                let services = self.services.lock().await;
                if services.get(name).is_some_and(|service| {
                    Self::matches_controller(service, controller)
                        && service.projection_generation == projection_generation
                }) {
                    let _ = self
                        .event_sender
                        .send(locald_core::ipc::Event::ServiceUpdate(status));
                }
            }
        }
    }

    async fn update_warnings(
        &self,
        name: &str,
        controller: ControllerIdentity,
        warnings: Vec<String>,
    ) {
        let (changed, snapshot_info) = {
            let mut services = self.services.lock().await;
            if let Some(service) = services
                .get_mut(name)
                .filter(|service| Self::matches_controller(service, controller))
            {
                if service.warnings == warnings {
                    (false, None)
                } else {
                    info!("Service {} warnings changed: {:?}", name, warnings);
                    service.warnings.clone_from(&warnings);
                    let projection_generation =
                        crate::manager::ProcessManager::advance_service_projection(service);

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
                            projection_generation,
                            self.domain_index
                                .snapshot()
                                .domain_for_service(service.instance_id, name)
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
                projection_generation,
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

                let services = self.services.lock().await;
                if services.get(name).is_some_and(|service| {
                    Self::matches_controller(service, controller)
                        && service.projection_generation == projection_generation
                }) {
                    let _ = self
                        .event_sender
                        .send(locald_core::ipc::Event::ServiceUpdate(status));
                }
            }
        }
    }

    fn spawn_http_monitor(
        &self,
        name: String,
        controller: ControllerIdentity,
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
                    if let Some(service) = services
                        .get(&name)
                        .filter(|service| Self::matches_controller(service, controller))
                    {
                        if service.health_status != HealthStatus::Starting {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if locald_utils::probe::check_http(&url, timeout).await {
                    monitor
                        .update_health(&name, controller, HealthStatus::Healthy, HealthSource::Http)
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
        controller: ControllerIdentity,
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
                    if let Some(service) = services
                        .get(&name)
                        .filter(|service| Self::matches_controller(service, controller))
                    {
                        if service.health_status != HealthStatus::Starting {
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
                        .update_health(
                            &name,
                            controller,
                            HealthStatus::Healthy,
                            HealthSource::Command,
                        )
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
        controller: ControllerIdentity,
        assigned_port: u16,
        interval: Duration,
        timeout: Duration,
        require_controller_health: bool,
    ) {
        info!(
            "Starting TCP monitor for {} on port {}",
            name, assigned_port
        );
        let monitor = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            loop {
                {
                    let services = monitor.services.lock().await;
                    if let Some(service) = services
                        .get(&name)
                        .filter(|service| Self::matches_controller(service, controller))
                    {
                        if service.health_status != HealthStatus::Starting {
                            info!("Service {} readiness is terminal, stopping monitor", name);
                            break;
                        }
                    } else {
                        info!(
                            "Service {} not found in services map, stopping monitor",
                            name
                        );
                        break;
                    }
                }

                info!("About to probe {} on {}", name, assigned_port);
                let result =
                    locald_utils::probe::check_tcp(&format!("127.0.0.1:{assigned_port}"), timeout)
                        .await;
                info!(
                    "Probing {} on {}... Success: {}",
                    name, assigned_port, result
                );

                let controller_ready = if result && require_controller_health {
                    let runtime = {
                        let services = monitor.services.lock().await;
                        services
                            .get(&name)
                            .filter(|service| Self::matches_controller(service, controller))
                            .and_then(|service| match &service.runtime_state {
                                crate::manager::ServiceRuntime::Controller(controller) => {
                                    Some(controller.clone())
                                }
                                crate::manager::ServiceRuntime::None => None,
                            })
                    };
                    if let Some(runtime) = runtime {
                        runtime.lock().await.read_state().await.health_status
                            == HealthStatus::Healthy
                    } else {
                        false
                    }
                } else {
                    true
                };

                if result && controller_ready {
                    monitor
                        .update_health(&name, controller, HealthStatus::Healthy, HealthSource::Tcp)
                        .await;
                    break;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{Service, ServiceRuntime};
    use anyhow::Result;
    use futures_util::{StreamExt, stream};
    use locald_core::config::{
        CommonServiceConfig, ContainerServiceConfig, ExecServiceConfig, LocaldConfig, ProbeConfig,
        SiteServiceConfig, WorkerServiceConfig,
    };
    use locald_core::ipc::LogEntry;
    use locald_core::service::{RuntimeState, ServiceCommand, ServiceController};
    use locald_core::state::ServiceState;
    use std::collections::HashMap;

    #[derive(Debug)]
    struct TestController {
        health_status: HealthStatus,
    }

    impl TestController {
        const fn unknown() -> Self {
            Self {
                health_status: HealthStatus::Unknown,
            }
        }
    }

    #[async_trait::async_trait]
    impl ServiceController for TestController {
        fn id(&self) -> &str {
            "health-test"
        }

        async fn prepare(&mut self) -> Result<()> {
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            Ok(())
        }

        async fn read_state(&self) -> RuntimeState {
            RuntimeState {
                pid: Some(std::process::id()),
                port: None,
                status: ServiceState::Running,
                health_status: self.health_status,
            }
        }

        async fn logs(&self) -> futures_util::stream::BoxStream<'static, LogEntry> {
            stream::empty().boxed()
        }

        fn get_metadata(&self, _key: &str) -> Option<String> {
            None
        }

        async fn execute_command(&mut self, _cmd: ServiceCommand) -> Result<()> {
            Ok(())
        }

        fn snapshot(&self) -> serde_json::Value {
            serde_json::Value::Null
        }

        async fn restore(&mut self, _state: serde_json::Value) -> Result<()> {
            Ok(())
        }

        async fn metrics(&self) -> Result<Option<locald_core::ipc::ServiceMetrics>> {
            Ok(None)
        }
    }

    fn instance_id(value: &str) -> ProjectInstanceId {
        value.parse().expect("valid project instance ID")
    }

    fn legacy_config(health_check: Option<HealthCheckConfig>) -> ServiceConfig {
        ServiceConfig::Legacy(ExecServiceConfig {
            common: CommonServiceConfig {
                health_check,
                ..CommonServiceConfig::default()
            },
            ..ExecServiceConfig::default()
        })
    }

    fn monitored_service(
        instance_id: ProjectInstanceId,
        service_config: ServiceConfig,
        sticky_port: Option<u16>,
    ) -> Service {
        Service {
            instance_id,
            controller_generation: 1,
            projection_generation: 1,
            config: LocaldConfig::default(),
            service_config,
            resolved_env: HashMap::new(),
            runtime_state: ServiceRuntime::Controller(Arc::new(Mutex::new(
                TestController::unknown(),
            ))),
            sticky_port,
            path: std::path::PathBuf::from("/readiness-test"),
            health_status: HealthStatus::Starting,
            health_source: HealthSource::None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn readiness_requirement_prefers_explicit_configuration_and_infers_by_service_kind() {
        let explicit_http = legacy_config(Some(HealthCheckConfig::Probe(ProbeConfig {
            kind: ProbeType::Http,
            path: Some("/ready".to_owned()),
            interval: Some(2),
            timeout: Some(3),
            command: None,
        })));
        assert_eq!(
            ReadinessRequirement::for_service(&explicit_http, Some(4123))
                .expect("derive explicit HTTP readiness"),
            ReadinessRequirement::ExplicitHttp {
                port: 4123,
                path: "/ready".to_owned(),
                interval: Duration::from_secs(2),
                timeout: Duration::from_secs(3),
            }
        );

        let explicit_command = legacy_config(Some(HealthCheckConfig::Command("true".to_owned())));
        assert!(matches!(
            ReadinessRequirement::for_service(&explicit_command, Some(4123))
                .expect("derive explicit command readiness"),
            ReadinessRequirement::ExplicitCommand { .. }
        ));
        assert!(matches!(
            ReadinessRequirement::for_service(&legacy_config(None), Some(4123))
                .expect("derive assigned-port readiness"),
            ReadinessRequirement::AssignedPortTcp { port: 4123 }
        ));
        let portless_worker =
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig::default()));
        assert!(!ReadinessRequirement::service_requires_port(
            &portless_worker
        ));
        assert!(matches!(
            ReadinessRequirement::for_service(&portless_worker, None)
                .expect("derive worker readiness"),
            ReadinessRequirement::ProcessRunning
        ));

        let portful_worker =
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                common: CommonServiceConfig {
                    port: Some(4124),
                    ..CommonServiceConfig::default()
                },
                ..WorkerServiceConfig::default()
            }));
        assert!(ReadinessRequirement::service_requires_port(&portful_worker));
        assert!(matches!(
            ReadinessRequirement::for_service(&portful_worker, Some(4124))
                .expect("derive portful worker readiness"),
            ReadinessRequirement::AssignedPortTcp { port: 4124 }
        ));
        assert!(matches!(
            ReadinessRequirement::for_service(
                &ServiceConfig::Typed(TypedServiceConfig::Site(SiteServiceConfig::default())),
                Some(4123),
            )
            .expect("derive site readiness"),
            ReadinessRequirement::ControllerAndAssignedPortTcp { port: 4123 }
        ));
        assert!(matches!(
            ReadinessRequirement::for_service(
                &ServiceConfig::Typed(TypedServiceConfig::Container(ContainerServiceConfig {
                    container_port: Some(6379),
                    ..ContainerServiceConfig::default()
                })),
                Some(4125),
            )
            .expect("derive fail-closed container endpoint readiness"),
            ReadinessRequirement::ControllerAndAssignedPortTcp { port: 4125 }
        ));
    }

    #[test]
    fn readiness_requirement_rejects_incomplete_explicit_probes() {
        let missing_port = legacy_config(Some(HealthCheckConfig::Probe(ProbeConfig {
            kind: ProbeType::Tcp,
            path: None,
            interval: None,
            timeout: None,
            command: None,
        })));
        assert!(
            ReadinessRequirement::for_service(&missing_port, None)
                .expect_err("TCP probe without assigned port must fail")
                .to_string()
                .contains("assigned service port")
        );

        let missing_command = legacy_config(Some(HealthCheckConfig::Probe(ProbeConfig {
            kind: ProbeType::Command,
            path: None,
            interval: None,
            timeout: None,
            command: None,
        })));
        assert!(
            ReadinessRequirement::for_service(&missing_command, Some(4123))
                .expect_err("command probe without command must fail")
                .to_string()
                .contains("requires `command`")
        );
    }

    #[tokio::test]
    async fn tcp_monitor_requires_the_exact_assigned_port() {
        let assigned_reservation = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve assigned port");
        let assigned_port = assigned_reservation
            .local_addr()
            .expect("assigned listener address")
            .port();
        let _unrelated_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("open unrelated listener in the same process");
        assert_ne!(
            _unrelated_listener
                .local_addr()
                .expect("unrelated listener address")
                .port(),
            assigned_port
        );
        drop(assigned_reservation);

        let instance_id = instance_id("00000000-0000-4000-8000-000000000003");
        let services = Arc::new(Mutex::new(HashMap::from([(
            "app:web".to_owned(),
            monitored_service(instance_id, legacy_config(None), Some(assigned_port)),
        )])));
        let (event_sender, _) = tokio::sync::broadcast::channel(8);
        let monitor = HealthMonitor::new(
            services.clone(),
            event_sender,
            Arc::new(Mutex::new((None, None))),
            SharedDomainIndex::default(),
        );
        monitor.spawn_check(
            "app:web".to_owned(),
            instance_id,
            1,
            ReadinessRequirement::ExplicitTcp {
                port: assigned_port,
                interval: Duration::from_millis(20),
                timeout: Duration::from_millis(10),
            },
            Some(assigned_port),
            Some(std::process::id()),
            None,
            None,
        );

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_ne!(
            services.lock().await["app:web"].health_status,
            HealthStatus::Healthy,
            "another listening port owned by the process must not satisfy readiness"
        );

        let _assigned_listener =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, assigned_port))
                .await
                .expect("listen on assigned port");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if services.lock().await["app:web"].health_status == HealthStatus::Healthy {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("assigned TCP port becomes ready");
        assert_eq!(
            services.lock().await["app:web"].health_source,
            HealthSource::Tcp
        );
    }

    #[tokio::test]
    async fn endpoint_monitor_waits_for_controller_health() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("open assigned endpoint");
        let assigned_port = listener
            .local_addr()
            .expect("assigned listener address")
            .port();
        let instance_id = instance_id("00000000-0000-4000-8000-000000000007");
        let controller = Arc::new(Mutex::new(TestController::unknown()));
        let runtime_controller: Arc<Mutex<dyn ServiceController>> = controller.clone();
        let mut service = monitored_service(
            instance_id,
            ServiceConfig::Typed(TypedServiceConfig::Site(SiteServiceConfig::default())),
            Some(assigned_port),
        );
        service.runtime_state = ServiceRuntime::Controller(runtime_controller);
        let services = Arc::new(Mutex::new(HashMap::from([(
            "app:site".to_owned(),
            service,
        )])));
        let (event_sender, _) = tokio::sync::broadcast::channel(8);
        let monitor = HealthMonitor::new(
            services.clone(),
            event_sender,
            Arc::new(Mutex::new((None, None))),
            SharedDomainIndex::default(),
        );
        monitor.spawn_check(
            "app:site".to_owned(),
            instance_id,
            1,
            ReadinessRequirement::ControllerAndAssignedPortTcp {
                port: assigned_port,
            },
            Some(assigned_port),
            None,
            None,
            None,
        );

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            services.lock().await["app:site"].health_status,
            HealthStatus::Starting,
            "a bound endpoint cannot bypass controller health"
        );
        controller.lock().await.health_status = HealthStatus::Healthy;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if services.lock().await["app:site"].health_status == HealthStatus::Healthy {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("controller and endpoint jointly become ready");
    }

    #[tokio::test]
    async fn timed_out_tcp_monitor_cannot_resurrect_readiness() {
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve readiness port");
        let port = reservation
            .local_addr()
            .expect("readiness reservation address")
            .port();
        drop(reservation);

        let instance_id = instance_id("00000000-0000-4000-8000-000000000006");
        let services = Arc::new(Mutex::new(HashMap::from([(
            "app:web".to_owned(),
            monitored_service(instance_id, legacy_config(None), Some(port)),
        )])));
        let (event_sender, _) = tokio::sync::broadcast::channel(8);
        let monitor = HealthMonitor::new(
            services.clone(),
            event_sender,
            Arc::new(Mutex::new((None, None))),
            SharedDomainIndex::default(),
        );
        monitor.spawn_check(
            "app:web".to_owned(),
            instance_id,
            1,
            ReadinessRequirement::ExplicitTcp {
                port,
                interval: Duration::from_millis(10),
                timeout: Duration::from_millis(10),
            },
            Some(port),
            None,
            None,
            None,
        );

        services
            .lock()
            .await
            .get_mut("app:web")
            .expect("readiness service remains present")
            .health_status = HealthStatus::Unhealthy;
        let _listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .expect("make endpoint available after readiness is terminal");
        tokio::time::sleep(Duration::from_millis(100)).await;
        monitor
            .update_health(
                "app:web",
                ControllerIdentity {
                    instance_id,
                    controller_generation: 1,
                },
                HealthStatus::Healthy,
                HealthSource::Tcp,
            )
            .await;

        assert_eq!(
            services.lock().await["app:web"].health_status,
            HealthStatus::Unhealthy,
            "a probe completing after the readiness deadline cannot publish Healthy"
        );
    }

    #[tokio::test]
    async fn explicit_command_monitor_is_authoritative() {
        let instance_id = instance_id("00000000-0000-4000-8000-000000000004");
        let service_config = legacy_config(Some(HealthCheckConfig::Command("exit 0".to_owned())));
        let services = Arc::new(Mutex::new(HashMap::from([(
            "app:worker".to_owned(),
            monitored_service(instance_id, service_config, None),
        )])));
        let (event_sender, _) = tokio::sync::broadcast::channel(8);
        let monitor = HealthMonitor::new(
            services.clone(),
            event_sender,
            Arc::new(Mutex::new((None, None))),
            SharedDomainIndex::default(),
        );
        monitor.spawn_check(
            "app:worker".to_owned(),
            instance_id,
            1,
            ReadinessRequirement::ExplicitCommand {
                command: "exit 0".to_owned(),
                interval: Duration::from_millis(10),
                timeout: Duration::from_secs(1),
            },
            None,
            None,
            None,
            None,
        );

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if services.lock().await["app:worker"].health_status == HealthStatus::Healthy {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("explicit command becomes ready");
        assert_eq!(
            services.lock().await["app:worker"].health_source,
            HealthSource::Command
        );
    }

    #[tokio::test]
    async fn explicit_http_monitor_uses_the_configured_path() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind HTTP readiness fixture");
        let port = listener.local_addr().expect("HTTP fixture address").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("accept HTTP readiness probe");
            let mut request = [0_u8; 1024];
            let bytes = stream
                .read(&mut request)
                .await
                .expect("read HTTP readiness request");
            let request = String::from_utf8_lossy(&request[..bytes]);
            let status = if request.starts_with("GET /ready ") {
                "200 OK"
            } else {
                "404 Not Found"
            };
            stream
                .write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .expect("write HTTP readiness response");
        });

        let instance_id = instance_id("00000000-0000-4000-8000-000000000005");
        let service_config = legacy_config(Some(HealthCheckConfig::Probe(ProbeConfig {
            kind: ProbeType::Http,
            path: Some("/ready".to_owned()),
            interval: Some(1),
            timeout: Some(1),
            command: None,
        })));
        let services = Arc::new(Mutex::new(HashMap::from([(
            "app:web".to_owned(),
            monitored_service(instance_id, service_config, Some(port)),
        )])));
        let (event_sender, _) = tokio::sync::broadcast::channel(8);
        let monitor = HealthMonitor::new(
            services.clone(),
            event_sender,
            Arc::new(Mutex::new((None, None))),
            SharedDomainIndex::default(),
        );
        monitor.spawn_check(
            "app:web".to_owned(),
            instance_id,
            1,
            ReadinessRequirement::ExplicitHttp {
                port,
                path: "/ready".to_owned(),
                interval: Duration::from_millis(10),
                timeout: Duration::from_secs(1),
            },
            Some(port),
            None,
            None,
            None,
        );

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if services.lock().await["app:web"].health_status == HealthStatus::Healthy {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("explicit HTTP path becomes ready");
        server.await.expect("HTTP readiness fixture joins");
        assert_eq!(
            services.lock().await["app:web"].health_source,
            HealthSource::Http
        );
    }

    #[tokio::test]
    async fn stale_instance_or_controller_updates_do_not_mutate_replacement_service() {
        let first_instance = instance_id("00000000-0000-4000-8000-000000000001");
        let second_instance = instance_id("00000000-0000-4000-8000-000000000002");
        let first_controller = ControllerIdentity {
            instance_id: first_instance,
            controller_generation: 1,
        };
        let stale_second_controller = ControllerIdentity {
            instance_id: second_instance,
            controller_generation: 1,
        };
        let current_second_controller = ControllerIdentity {
            instance_id: second_instance,
            controller_generation: 2,
        };
        let services = Arc::new(Mutex::new(HashMap::from([(
            "app:web".to_owned(),
            Service {
                instance_id: second_instance,
                controller_generation: 2,
                projection_generation: 1,
                config: LocaldConfig::default(),
                service_config: ServiceConfig::Legacy(ExecServiceConfig::default()),
                resolved_env: HashMap::new(),
                runtime_state: ServiceRuntime::Controller(Arc::new(Mutex::new(
                    TestController::unknown(),
                ))),
                sticky_port: None,
                path: std::path::PathBuf::from("/second"),
                health_status: HealthStatus::Unknown,
                health_source: HealthSource::None,
                warnings: Vec::new(),
            },
        )])));
        let (event_sender, _) = tokio::sync::broadcast::channel(8);
        let monitor = HealthMonitor::new(
            services.clone(),
            event_sender,
            Arc::new(Mutex::new((None, None))),
            SharedDomainIndex::default(),
        );

        monitor
            .update_health(
                "app:web",
                first_controller,
                HealthStatus::Healthy,
                HealthSource::Tcp,
            )
            .await;
        monitor
            .update_warnings(
                "app:web",
                first_controller,
                vec!["stale warning".to_owned()],
            )
            .await;
        monitor
            .update_health(
                "app:web",
                stale_second_controller,
                HealthStatus::Healthy,
                HealthSource::Tcp,
            )
            .await;
        monitor
            .update_warnings(
                "app:web",
                stale_second_controller,
                vec!["stale controller warning".to_owned()],
            )
            .await;

        services
            .lock()
            .await
            .get_mut("app:web")
            .expect("replacement service")
            .runtime_state = ServiceRuntime::None;
        monitor
            .update_health(
                "app:web",
                current_second_controller,
                HealthStatus::Healthy,
                HealthSource::Tcp,
            )
            .await;

        let services = services.lock().await;
        let replacement = &services["app:web"];
        assert_eq!(replacement.instance_id, second_instance);
        assert_eq!(replacement.health_status, HealthStatus::Unknown);
        assert_eq!(replacement.health_source, HealthSource::None);
        assert!(replacement.warnings.is_empty());
    }

    #[tokio::test]
    async fn projection_change_suppresses_inflight_health_event() {
        let instance_id = instance_id("00000000-0000-4000-8000-000000000001");
        let controller_identity = ControllerIdentity {
            instance_id,
            controller_generation: 1,
        };
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::unknown()));
        let services = Arc::new(Mutex::new(HashMap::from([(
            "app:web".to_owned(),
            Service {
                instance_id,
                controller_generation: 1,
                projection_generation: 1,
                config: LocaldConfig::default(),
                service_config: ServiceConfig::Legacy(ExecServiceConfig::default()),
                resolved_env: HashMap::new(),
                runtime_state: ServiceRuntime::Controller(controller.clone()),
                sticky_port: None,
                path: std::path::PathBuf::from("/app"),
                health_status: HealthStatus::Unknown,
                health_source: HealthSource::None,
                warnings: Vec::new(),
            },
        )])));
        let (event_sender, mut event_receiver) = tokio::sync::broadcast::channel(8);
        let monitor = HealthMonitor::new(
            services.clone(),
            event_sender,
            Arc::new(Mutex::new((None, None))),
            SharedDomainIndex::default(),
        );

        let controller_guard = controller.lock().await;
        let update_task = tokio::spawn({
            let monitor = monitor.clone();
            async move {
                monitor
                    .update_warnings(
                        "app:web",
                        controller_identity,
                        vec!["old projection".to_owned()],
                    )
                    .await;
            }
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let warning_published = services
                    .lock()
                    .await
                    .get("app:web")
                    .is_some_and(|service| !service.warnings.is_empty());
                if warning_published {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("warning update reaches status construction");

        {
            let mut services = services.lock().await;
            let service = services.get_mut("app:web").expect("loaded service");
            service.config.project.domain = Some("new.app.localhost".to_owned());
            crate::manager::ProcessManager::advance_service_projection(service);
        }
        drop(controller_guard);
        update_task.await.expect("health update task");

        assert!(matches!(
            event_receiver.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
}
