use locald_core::config::{
    DEFAULT_HEALTH_CHECK_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_TIMEOUT_SECS, HealthCheckConfig,
    ProbeType, ServiceConfig,
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
        config: &ServiceConfig,
        port: Option<u16>,
        pid: Option<u32>,
        _container_id: Option<String>,
        cwd: Option<std::path::PathBuf>,
    ) {
        let controller = ControllerIdentity {
            instance_id,
            controller_generation,
        };
        let default_interval = Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS);
        let default_timeout = Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS);

        // Spawn port mismatch detector if we have a PID and an expected port
        if let (Some(pid), Some(expected_port)) = (pid, port) {
            self.spawn_port_mismatch_monitor(name.clone(), controller, pid, expected_port);
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
                    self.spawn_command_monitor(
                        name,
                        controller,
                        cmd.clone(),
                        cwd,
                        interval,
                        timeout,
                    );
                }
                HealthCheckConfig::Probe(probe) => match probe.kind {
                    ProbeType::Http => {
                        if let Some(p) = port {
                            let path = probe.path.as_deref().unwrap_or("/");
                            self.spawn_http_monitor(
                                name,
                                controller,
                                p,
                                path.to_string(),
                                interval,
                                timeout,
                            );
                        }
                    }
                    ProbeType::Tcp => {
                        if let Some(p) = port {
                            self.spawn_tcp_monitor(name, controller, p, interval, timeout);
                        }
                    }
                    ProbeType::Command => {
                        if let Some(cmd) = &probe.command {
                            self.spawn_command_monitor(
                                name,
                                controller,
                                cmd.clone(),
                                cwd,
                                interval,
                                timeout,
                            );
                        }
                    }
                },
            }
        } else if let Some(p) = port {
            self.spawn_tcp_monitor(name, controller, p, default_interval, default_timeout);
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
                        if service.health_status == HealthStatus::Healthy {
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
                    if let Some(service) = services
                        .get(&name)
                        .filter(|service| Self::matches_controller(service, controller))
                    {
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
                        .update_health(&name, controller, HealthStatus::Healthy, HealthSource::Tcp)
                        .await;
                    break;
                }

                if let Some(pid) = pid {
                    if let Ok(ports) = locald_utils::discovery::find_listening_ports(pid).await {
                        if let Some(&found_port) = ports.first() {
                            info!("Service {} discovered on port {}", name, found_port);
                            // Port update removed as it requires Controller support
                            monitor
                                .update_health(
                                    &name,
                                    controller,
                                    HealthStatus::Healthy,
                                    HealthSource::Tcp,
                                )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{Service, ServiceRuntime};
    use anyhow::Result;
    use futures_util::{StreamExt, stream};
    use locald_core::config::{ExecServiceConfig, LocaldConfig};
    use locald_core::ipc::LogEntry;
    use locald_core::service::{RuntimeState, ServiceCommand, ServiceController};
    use locald_core::state::ServiceState;
    use std::collections::HashMap;

    #[derive(Debug)]
    struct TestController;

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
                pid: None,
                port: None,
                status: ServiceState::Running,
                health_status: HealthStatus::Unknown,
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
                runtime_state: ServiceRuntime::Controller(Arc::new(Mutex::new(TestController))),
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
        let controller: Arc<Mutex<dyn ServiceController>> = Arc::new(Mutex::new(TestController));
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
