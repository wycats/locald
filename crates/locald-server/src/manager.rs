#![allow(clippy::collapsible_if)]
#![allow(clippy::option_if_let_else)]
use crate::config_loader::ConfigLoader;
use crate::health::HealthMonitor;
use crate::plugins;
use crate::port_allocator::PortAllocator;
use crate::runtime::Runtime;
use crate::state::StateManager;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use futures_util::StreamExt;
use locald_core::attachments::{
    Attachment, AttachmentSource, AttachmentStore, ProjectFilter, ProjectListEntry, ProjectSection,
    ProjectStatusInfo,
};
use locald_core::config::{LocaldConfig, ServiceConfig, TypedServiceConfig};
use locald_core::ipc::{BootEvent, Event, LogEntry, ServiceStatus};
use locald_core::registry::Registry;
use locald_core::resolver::ServiceResolver;
use locald_core::service::{ServiceContext, ServiceController, ServiceFactory};
use locald_core::state::{
    HealthSource, HealthStatus, PersistedServiceState, ServerState, ServiceState,
};
use locald_core::{
    CatalogError, DomainClaim, DomainName, DomainTarget, ProjectInstanceId, SharedDomainIndex,
    sanitize_project_name_for_dns,
};
use nix::sys::signal::Signal;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::SystemTime;
use tokio::sync::{Mutex, broadcast};
use tracing::{error, info, warn};

const LOG_BUFFER_SIZE: usize = 2000;

#[derive(Debug)]
struct LogBuffer {
    buffer: VecDeque<LogEntry>,
    capacity: usize,
}

impl LogBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn push(&mut self, entry: LogEntry) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(entry);
    }

    fn get_all(&self) -> Vec<LogEntry> {
        self.buffer.iter().cloned().collect()
    }
}

#[async_trait::async_trait]
pub trait HostSyncer: Send + Sync + 'static {
    async fn sync(&self, domains: Vec<String>) -> Result<()>;
}

struct DefaultHostSyncer;

#[async_trait::async_trait]
impl HostSyncer for DefaultHostSyncer {
    async fn sync(&self, domains: Vec<String>) -> Result<()> {
        // Try to read hosts file to see if we need to update
        let hosts = locald_core::HostsFileSection::new();
        let needs_update = match hosts.read().await {
            Ok(content) => {
                let new_content = hosts.update_content(&content, &domains);
                content != new_content
            }
            Err(e) => {
                warn!("Failed to read hosts file: {}", e);
                true // Assume update needed
            }
        };

        if !needs_update {
            info!("Hosts file is up to date, skipping sync");
            return Ok(());
        }

        let shim_path = match locald_utils::shim::find_privileged()? {
            Some(path) => path,
            None => {
                anyhow::bail!(
                    "locald-shim is not installed or not setuid root. Run `sudo locald admin setup` to configure it."
                );
            }
        };

        info!("Auto-syncing hosts using {}", shim_path.display());

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            locald_utils::shim::tokio_command(&shim_path)
                .arg("admin")
                .arg("sync-hosts")
                .args(&domains)
                .output(),
        )
        .await??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            if stderr.is_empty() {
                anyhow::bail!("Failed to sync hosts");
            }
            anyhow::bail!("Failed to sync hosts: {}", stderr);
        }

        Ok(())
    }
}

impl fmt::Debug for dyn HostSyncer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HostSyncer")
    }
}

struct TaskGuard(tokio::task::JoinHandle<()>);
impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone, Debug)]
struct ConcurrencyGuard {
    lock: Arc<Mutex<()>>,
}

impl ConcurrencyGuard {
    fn new() -> Self {
        Self {
            lock: Arc::new(Mutex::new(())),
        }
    }

    async fn run<F, Fut>(&self, f: F) -> Result<()>
    where
        F: Fn() -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        let _guard = self.lock.lock().await;
        f().await
    }
}

#[derive(Clone)]
pub(crate) enum ServiceRuntime {
    Controller(Arc<tokio::sync::Mutex<dyn ServiceController>>),
    None,
}

impl fmt::Debug for ServiceRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Controller(c) => f.debug_tuple("Controller").field(c).finish(),
            Self::None => write!(f, "None"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Service {
    pub config: LocaldConfig,
    #[allow(clippy::struct_field_names)]
    pub service_config: ServiceConfig,
    pub resolved_env: HashMap<String, String>,
    pub runtime_state: ServiceRuntime,
    pub sticky_port: Option<u16>,
    pub path: PathBuf,
    pub health_status: HealthStatus,
    pub health_source: HealthSource,
    pub warnings: Vec<String>,
}

impl Service {}

/// Manages the lifecycle of services (processes, containers, databases).
///
/// The `ProcessManager` is the central brain of `locald`. It handles:
/// - Starting and stopping services.
/// - Monitoring health and status.
/// - Persisting state across restarts.
/// - Broadcasting logs and events.
pub(crate) enum RuntimeSnapshot {
    Static {
        is_running: bool,
        pid: Option<u32>,
        port: Option<u16>,
    },
    Controller(Arc<tokio::sync::Mutex<dyn ServiceController>>),
}

#[derive(Clone, Debug)]
pub struct ProcessManager {
    services: Arc<Mutex<HashMap<String, Service>>>,
    pub log_sender: broadcast::Sender<LogEntry>,
    pub event_sender: broadcast::Sender<Event>,
    log_buffers: Arc<StdMutex<HashMap<String, LogBuffer>>>,
    state_manager: Arc<StateManager>,
    runtime: Arc<Runtime>,
    proxy_ports: Arc<Mutex<(Option<u16>, Option<u16>)>>, // (http, https)
    watchers: Arc<Mutex<HashMap<PathBuf, RecommendedWatcher>>>,
    registry: Arc<Mutex<Registry>>,
    domain_index: SharedDomainIndex,
    attachments: Arc<Mutex<AttachmentStore>>,
    health_monitor: HealthMonitor,
    factories: Vec<Arc<dyn ServiceFactory>>,
    hosts_sync_guard: ConcurrencyGuard,
    host_syncer: Arc<dyn HostSyncer>,
    port_allocator: PortAllocator,
}

impl ProcessManager {
    fn postgres_data_dir(name: &str) -> PathBuf {
        ProjectDirs::from("com", "locald", "locald")
            .map(|d| d.data_dir().join("postgres").join(name))
            .unwrap_or_else(|| PathBuf::from(".locald/postgres").join(name))
    }

    pub async fn get_service_controller(
        &self,
        name: &str,
    ) -> Option<Arc<tokio::sync::Mutex<dyn ServiceController>>> {
        let services = self.services.lock().await;
        if let Some(service) = services.get(name) {
            if let ServiceRuntime::Controller(c) = &service.runtime_state {
                return Some(c.clone());
            }
        }
        None
    }

    /// Create a new `ProcessManager`.
    ///
    /// # Arguments
    ///
    /// * `notify_socket_path` - Path to the Unix socket for `sd_notify` messages.
    /// * `state_manager` - State persistence manager.
    /// * `registry` - Project registry.
    pub fn new(
        notify_socket_path: PathBuf,
        state_manager: Arc<StateManager>,
        registry: Arc<Mutex<Registry>>,
        attachments: Arc<Mutex<AttachmentStore>>,
        external_log_sender: Option<broadcast::Sender<LogEntry>>,
    ) -> Result<Self> {
        let (tx, _) = if let Some(tx) = external_log_sender {
            (tx, broadcast::channel(1).1) // Dummy receiver
        } else {
            broadcast::channel(100)
        };
        let (event_tx, _) = broadcast::channel(100);

        let services = Arc::new(Mutex::new(HashMap::new()));
        let proxy_ports = Arc::new(Mutex::new((None, None)));

        let domain_index = {
            let registry_snapshot = registry
                .try_lock()
                .context("project identity catalog is busy during manager initialization")?;
            SharedDomainIndex::new(registry_snapshot.domain_index().clone())
        };

        let health_monitor = HealthMonitor::new(
            services.clone(),
            event_tx.clone(),
            proxy_ports.clone(),
            domain_index.clone(),
        );

        let runtime = Arc::new(Runtime::new(notify_socket_path));

        let factories: Vec<Arc<dyn ServiceFactory>> = vec![
            Arc::new(crate::service::postgres::PostgresFactory),
            Arc::new(crate::service::site::SiteFactory),
            Arc::new(crate::service::exec::ExecFactory::new(
                runtime.process.clone(),
            )),
        ];

        Ok(Self {
            services,
            log_sender: tx,
            event_sender: event_tx,
            log_buffers: Arc::new(StdMutex::new(HashMap::new())),
            state_manager,
            runtime,
            proxy_ports,
            watchers: Arc::new(Mutex::new(HashMap::new())),
            registry,
            domain_index,
            attachments,
            health_monitor,
            factories,
            hosts_sync_guard: ConcurrencyGuard::new(),
            host_syncer: Arc::new(DefaultHostSyncer),
            port_allocator: PortAllocator::new(),
        })
    }

    #[cfg(test)]
    pub fn set_host_syncer(&mut self, syncer: Arc<dyn HostSyncer>) {
        self.host_syncer = syncer;
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn build_service_status(
        name: String,
        domain: Option<String>,
        path: Option<PathBuf>,
        proxy_ports: (Option<u16>, Option<u16>),
        health_status: HealthStatus,
        health_source: HealthSource,
        snapshot: RuntimeSnapshot,
        service_config: Option<&ServiceConfig>,
        workspace: Option<String>,
        constellation: Option<String>,
        warnings: Vec<String>,
    ) -> ServiceStatus {
        use locald_core::ipc::ServiceType;

        let (status, pid, port) = match snapshot {
            RuntimeSnapshot::Static {
                is_running,
                pid,
                port,
            } => (
                if is_running {
                    locald_core::state::ServiceState::Running
                } else {
                    locald_core::state::ServiceState::Stopped
                },
                pid,
                port,
            ),
            RuntimeSnapshot::Controller(c) => {
                let state = c.lock().await.read_state().await;
                (state.status, state.pid, state.port)
            }
        };

        // Determine service type from config
        let service_type = service_config.map(ServiceType::from).unwrap_or_default();

        // Compute the public URL (domain-based or localhost)
        let url = if status == locald_core::state::ServiceState::Running && port.is_some() {
            if let Some(ServiceConfig::Typed(TypedServiceConfig::Postgres(_))) = service_config {
                None
            } else {
                domain.as_ref().map_or_else(
                    || port.map(|p| format!("http://localhost:{p}")),
                    |d| {
                        let (proxy_http, proxy_https) = proxy_ports;
                        if let Some(p) = proxy_https {
                            if p == 443 {
                                Some(format!("https://{d}"))
                            } else {
                                Some(format!("https://{d}:{p}"))
                            }
                        } else if let Some(p) = proxy_http {
                            if p == 80 {
                                Some(format!("http://{d}"))
                            } else {
                                Some(format!("http://{d}:{p}"))
                            }
                        } else {
                            // Default to HTTPS (implied port 443)
                            Some(format!("https://{d}"))
                        }
                    },
                )
            }
        } else {
            None
        };

        // Compute the connection URL (raw connection string)
        let connection_url = if status == locald_core::state::ServiceState::Running {
            match service_config {
                Some(ServiceConfig::Typed(TypedServiceConfig::Postgres(_))) => {
                    port.map(|p| format!("postgres://postgres@localhost:{p}/postgres"))
                }
                _ => port.map(|p| format!("http://localhost:{p}")),
            }
        } else {
            None
        };

        ServiceStatus {
            name: name.clone(),
            service_type,
            pid,
            port,
            status,
            url,
            connection_url,
            health_status,
            health_source,
            path,
            domain,
            workspace,
            constellation,
            warnings,
        }
    }

    pub async fn set_http_port(&self, port: Option<u16>) {
        self.proxy_ports.lock().await.0 = port;
    }

    pub async fn set_https_port(&self, port: Option<u16>) {
        self.proxy_ports.lock().await.1 = port;
    }

    fn reap_dead_services(_name: &str, _service: &mut Service) {
        // Controllers handle their own reaping/status updates
    }

    /// Return a cloneable exact-domain snapshot handle for routing and TLS.
    #[must_use]
    pub fn domain_index(&self) -> SharedDomainIndex {
        self.domain_index.clone()
    }

    fn build_domain_claims(
        instance_id: ProjectInstanceId,
        config: &LocaldConfig,
    ) -> Result<Vec<DomainClaim>> {
        let base_domain = config.project.domain.clone().unwrap_or_else(|| {
            format!(
                "{}.localhost",
                sanitize_project_name_for_dns(&config.project.name)
            )
        });
        let base_domain = base_domain.parse::<DomainName>().with_context(|| {
            format!(
                "project `{}` has an invalid exact base domain `{base_domain}`",
                config.project.name
            )
        })?;
        let mut service_names = config.services.keys().cloned().collect::<Vec<_>>();
        service_names.sort();

        let mut claims = Vec::with_capacity(service_names.len());
        for service_name in service_names {
            let domain = if service_name == "web" {
                base_domain.clone()
            } else {
                base_domain.with_prefix(&service_name).with_context(|| {
                    format!("service `{service_name}` has an invalid exact domain label")
                })?
            };
            claims.push(DomainClaim::service(
                domain,
                instance_id,
                format!("{}:{service_name}", config.project.name),
            ));
        }
        Ok(claims)
    }

    fn effective_service_env(
        config: &LocaldConfig,
        dot_env_vars: &HashMap<String, String>,
        service_config: &ServiceConfig,
    ) -> (HashMap<String, String>, Option<String>) {
        let mut combined_env = dot_env_vars.clone();
        combined_env.extend(service_config.env().clone());

        let injected_database = if combined_env.contains_key("DATABASE_URL") {
            None
        } else {
            service_config.depends_on().iter().find_map(|dependency| {
                matches!(
                    config.services.get(dependency),
                    Some(ServiceConfig::Typed(TypedServiceConfig::Postgres(_)))
                )
                .then(|| dependency.clone())
            })
        };

        if let Some(dependency) = &injected_database {
            combined_env.insert(
                "DATABASE_URL".to_owned(),
                format!("${{services.{dependency}.url}}"),
            );
        }

        (combined_env, injected_database)
    }

    fn configured_service_name<'a>(full_name: &'a str, config: &LocaldConfig) -> &'a str {
        full_name
            .strip_prefix(config.project.name.as_str())
            .and_then(|suffix| suffix.strip_prefix(':'))
            .unwrap_or(full_name)
    }

    fn domain_for_service(&self, name: &str) -> Option<String> {
        self.domain_index
            .snapshot()
            .domain_for_service(name)
            .map(ToString::to_string)
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn get_service_status(&self, name: &str) -> Option<ServiceStatus> {
        let proxy_ports = { *self.proxy_ports.lock().await };
        let (
            domain,
            path,
            health_status,
            health_source,
            snapshot,
            service_config,
            workspace,
            constellation,
            warnings,
        ) = {
            let mut services = self.services.lock().await;
            let service = services.get_mut(name)?;
            // We reap here to ensure status is up to date for single service query too
            Self::reap_dead_services(name, service);

            let snapshot = match &service.runtime_state {
                ServiceRuntime::Controller(c) => RuntimeSnapshot::Controller(c.clone()),
                ServiceRuntime::None => RuntimeSnapshot::Static {
                    is_running: false,
                    pid: None,
                    port: None,
                },
            };

            (
                self.domain_for_service(name),
                Some(service.path.clone()),
                service.health_status,
                service.health_source,
                snapshot,
                service.service_config.clone(),
                service.config.project.workspace.clone(),
                service.config.project.constellation.clone(),
                service.warnings.clone(),
            )
        };

        Some(
            Self::build_service_status(
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
            .await,
        )
    }

    async fn broadcast_service_update(&self, name: &str) {
        if let Some(status) = self.get_service_status(name).await {
            let _ = self.event_sender.send(Event::ServiceUpdate(status));
        }
    }

    fn broadcast_log(&self, entry: LogEntry) {
        info!("Broadcasting log for {}: {}", entry.service, entry.message);
        // Add to buffer
        {
            #[allow(clippy::expect_used)]
            let mut buffers = self.log_buffers.lock().expect("log buffer mutex poisoned");
            let buffer = buffers
                .entry(entry.service.clone())
                .or_insert_with(|| LogBuffer::new(LOG_BUFFER_SIZE));
            buffer.push(entry.clone());
        }

        // Broadcast (ignore error if no receivers)
        let _ = self.log_sender.send(entry.clone());
        let _ = self.event_sender.send(Event::Log(entry));
    }

    #[must_use]
    pub fn get_recent_logs(&self) -> Vec<LogEntry> {
        #[allow(clippy::expect_used)]
        let buffers = self.log_buffers.lock().expect("log buffer mutex poisoned");
        let mut all_logs = Vec::new();
        for buffer in buffers.values() {
            all_logs.extend(buffer.get_all());
        }
        all_logs.sort_by_key(|e| e.timestamp);
        all_logs
    }

    async fn persist_state(&self) {
        let mut services_data = Vec::new();
        {
            let services = self.services.lock().await;
            for (name, service) in services.iter() {
                services_data.push((
                    name.clone(),
                    service.config.clone(),
                    service.path.clone(),
                    service.health_status,
                    service.health_source,
                    service.runtime_state.clone(),
                ));
            }
        }

        let mut service_states = Vec::new();
        for (name, config, path, health_status, health_source, runtime) in services_data {
            let (pid, port, status, container_id) = match runtime {
                ServiceRuntime::Controller(c) => {
                    let guard = c.lock().await;
                    let state = guard.read_state().await;
                    let container_id = guard.get_metadata("container_id");
                    (state.pid, state.port, state.status, container_id)
                }
                ServiceRuntime::None => {
                    (None, None, locald_core::state::ServiceState::Stopped, None)
                }
            };

            service_states.push(PersistedServiceState {
                name,
                config,
                path,
                pid,
                container_id,
                port,
                status,
                health_status,
                health_source,
            });
        }

        let state = ServerState {
            services: service_states,
        };

        if let Err(e) = self.state_manager.save(&state).await {
            error!("Failed to persist state: {e}");
        }
    }

    pub async fn restore(&self) -> Result<()> {
        let Ok(state) = self.state_manager.load().await else {
            return Ok(()); // No state to restore
        };

        info!("Restoring state: found {} services", state.services.len());

        // Cleanup old processes and containers
        for service_state in &state.services {
            if let Some(pid) = service_state.pid {
                if let Err(e) = self.runtime.process.kill_pid(pid as i32, Signal::SIGTERM) {
                    warn!("Cleanup warning (kill_pid): {:#}", e);
                }
            }
            if let Some(container_id) = &service_state.container_id {
                if let Err(e) = self.runtime.process.stop_shim_container(container_id) {
                    warn!("Cleanup warning (stop_shim_container): {:#}", e);
                }
            }
        }

        // Restart projects
        let mut paths = HashSet::new();
        for service_state in state.services {
            // Only restore if it was running or we want to be aggressive?
            // For now, let's restore everything that was in the state file as "running"
            // But wait, the state file has a "status" field.
            if service_state.status == ServiceState::Running {
                paths.insert(service_state.path);
            }
        }

        for path in paths {
            info!("Restoring project at {path:?}");
            if let Err(e) = self.start(path.clone(), None, false).await {
                error!("Failed to restore project at {path:?}: {e}");
            }
        }

        Ok(())
    }

    pub async fn handle_notify(&self, pid: u32) {
        let mut services = self.services.lock().await;
        for (name, service) in services.iter_mut() {
            if let ServiceRuntime::Controller(c) = &service.runtime_state {
                let state = c.lock().await.read_state().await;
                if state.pid == Some(pid) {
                    info!("Service {} is ready (via notify)", name);
                    service.health_status = HealthStatus::Healthy;
                    service.health_source = HealthSource::Notify;
                    break;
                }
            }
        }
    }

    async fn wait_for_health(&self, name: &str) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30); // TODO: Make configurable

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Service {name} timed out waiting for health check");
            }

            {
                let mut services = self.services.lock().await;
                if let Some(service) = services.get_mut(name) {
                    match &service.runtime_state {
                        ServiceRuntime::Controller(c) => {
                            let state = c.lock().await.read_state().await;
                            // info!("Controller state for {}: status={:?}, health={:?}", name, state.status, state.health_status);
                            if state.status == locald_core::state::ServiceState::Stopped {
                                anyhow::bail!("Service {name} stopped unexpectedly during startup");
                            }
                            if state.health_status == HealthStatus::Healthy {
                                service.health_status = HealthStatus::Healthy;
                            }
                        }
                        ServiceRuntime::None => {
                            anyhow::bail!("Service {name} is not running");
                        }
                    }

                    if service.health_status == HealthStatus::Healthy {
                        return Ok(());
                    }
                } else {
                    anyhow::bail!("Service {name} disappeared");
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn get_service_field(&self, name: &str, field: &str) -> Result<String> {
        // Re-acquire lock to get port, or just get it all at once?
        // The issue is holding the lock across await points or significant drops.
        // Let's get everything we need in one go.
        let (service_config, port_result) = {
            let services = self.services.lock().await;
            let service = services
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Service {name} not found"))?;

            let port_result = match &service.runtime_state {
                ServiceRuntime::Controller(c) => Err(c.clone()),
                ServiceRuntime::None => Ok(None),
            };

            (service.config.clone(), port_result)
        };

        let port = match port_result {
            Ok(p) => p,
            Err(c) => c.lock().await.read_state().await.port,
        };

        match field {
            "port" => Ok(port
                .ok_or_else(|| anyhow::anyhow!("Service {name} has no port"))?
                .to_string()),
            "host" => Ok("localhost".to_string()),
            "url" => {
                let short_name = Self::configured_service_name(name, &service_config);
                let svc_config = service_config
                    .services
                    .get(short_name)
                    .ok_or_else(|| anyhow::anyhow!("Config for {name} not found"))?;

                let port = port.ok_or_else(|| anyhow::anyhow!("Service {name} has no port"))?;

                match svc_config {
                    ServiceConfig::Typed(TypedServiceConfig::Postgres(_)) => Ok(format!(
                        "postgres://postgres:postgres@localhost:{port}/postgres"
                    )),
                    ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => {
                        Ok(format!("http://localhost:{port}"))
                    }
                }
            }
            _ => anyhow::bail!("Unknown field {field} for service {name}"),
        }
    }

    pub async fn sync_hosts(&self) -> Result<()> {
        let manager = self.clone();
        let syncer = self.host_syncer.clone();

        self.hosts_sync_guard
            .run(move || {
                let manager = manager.clone();
                let syncer = syncer.clone();
                async move {
                    let domains = manager.domain_index.snapshot().service_domains();
                    syncer.sync(domains).await
                }
            })
            .await
    }

    async fn sync_hosts_after_catalog_publish(&self) {
        if let Err(error) = self.sync_hosts().await {
            warn!("Failed to synchronize published domain claims: {error}");
        }
    }

    /// Starts a project from the given path.
    ///
    /// This method:
    /// 1. Sets up a file watcher for configuration changes.
    /// 2. Loads and applies the configuration (starting services).
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The configuration file cannot be read or parsed.
    /// - Services fail to start.
    /// - Dependencies cannot be satisfied.
    pub async fn start(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
    ) -> Result<()> {
        self.watch_config(path.clone()).await;
        self.apply_config(path, event_tx, verbose).await
    }

    async fn watch_config(&self, path: PathBuf) {
        {
            let watchers = self.watchers.lock().await;
            if watchers.contains_key(&path) {
                return;
            }
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(100);
        let manager = self.clone();
        let path_clone = path.clone();

        // Spawn debouncer task
        tokio::spawn(async move {
            loop {
                // Wait for first event
                if rx.recv().await.is_none() {
                    break;
                }

                // Debounce loop
                loop {
                    let timeout = tokio::time::sleep(std::time::Duration::from_millis(500));
                    tokio::select! {
                        res = rx.recv() => {
                            if res.is_none() {
                                return;
                            }
                            // Received another event, loop again (reset timeout)
                        }
                        () = timeout => {
                            // Timeout expired, trigger reload
                            info!("Reloading config for {:?}", path_clone);
                            if let Err(e) = manager.apply_config(path_clone.clone(), None, false).await {
                                error!("Failed to reload config: {e}");
                            }
                            break; // Break inner loop, go back to waiting for first event
                        }
                    }
                }
            }
        });

        let handle = tokio::runtime::Handle::current();

        let watcher_res = notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    if event.kind.is_modify() || event.kind.is_create() {
                        let relevant = event.paths.iter().any(|p| {
                            p.ends_with("locald.toml")
                                || p.ends_with("Procfile")
                                || p.ends_with(".env")
                        });

                        if relevant {
                            info!("Config changed: {:?}", event.paths);
                            let tx = tx.clone();
                            handle.spawn(async move {
                                let _ = tx.send(()).await;
                            });
                        }
                    }
                }
                Err(e) => error!("Watch error: {e}"),
            },
        );

        match watcher_res {
            Ok(mut watcher) => {
                if let Err(e) = watcher.watch(&path, RecursiveMode::NonRecursive) {
                    error!("Failed to watch config: {e}");
                } else {
                    let mut watchers = self.watchers.lock().await;
                    watchers.insert(path, watcher);
                }
            }
            Err(e) => error!("Failed to create watcher: {e}"),
        }
    }

    pub async fn apply_config(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
    ) -> Result<()> {
        let path = Self::canonicalize_path(&path);

        // Setup log forwarding if verbose
        let _log_guard = if verbose {
            event_tx.as_ref().map(|tx| {
                let tx = tx.clone();
                let mut rx = self.log_sender.subscribe();
                TaskGuard(tokio::spawn(async move {
                    while let Ok(entry) = rx.recv().await {
                        let _ = tx
                            .send(BootEvent::Log {
                                id: entry.service,
                                line: entry.message,
                                stream: entry.stream,
                            })
                            .await;
                    }
                }))
            })
        } else {
            None
        };

        if let Some(tx) = &event_tx {
            let _ = tx
                .send(BootEvent::StepStarted {
                    id: "config".to_string(),
                    description: "Loading configuration".to_string(),
                })
                .await;
        }

        let (mut config, dot_env_vars) = ConfigLoader::load_project_config(&path).await?;

        // Apply plugins to configuration
        // Plugin discovery and application failures are logged but do not fail startup
        // The returned guards keep plugin-allocated ports reserved until services bind
        let mut plugin_port_guards =
            match plugins::apply_plugins_to_config(&mut config, &path, &self.port_allocator) {
                Ok(guards) => guards,
                Err(e) => {
                    warn!("Plugin processing failed: {}", e);
                    Vec::new()
                }
            };

        // Release plugin guard listeners so services can bind to their allocated ports.
        // Guards still track ports as pending until they drop at end of scope.
        for guard in &mut plugin_port_guards {
            guard.release_listener();
        }

        // Validate the complete post-plugin configuration before publishing
        // identity or domain ownership.
        let sorted_services = ConfigLoader::resolve_startup_order(&config)?;
        for (service_name, service_config) in &config.services {
            let (effective_env, _) =
                Self::effective_service_env(&config, &dot_env_vars, service_config);
            ConfigLoader::validate_env_references(&effective_env, &config, service_name)
                .with_context(|| {
                    format!("service `{service_name}` contains an invalid environment reference")
                })?;
        }
        let desired_service_names = config
            .services
            .keys()
            .map(|service_name| format!("{}:{service_name}", config.project.name))
            .collect::<HashSet<_>>();
        let discovery = Registry::discover(path.clone()).await?;
        let (commit_result, removed_service_names, published_domain_index) = {
            let mut registry = self.registry.lock().await;
            let mut candidate = registry.clone();
            let instance_id =
                candidate.register_project(discovery, Some(config.project.name.clone()))?;
            let claims = Self::build_domain_claims(instance_id, &config)?;
            candidate.replace_domain_claims(instance_id, claims)?;

            // Keep the previous claim set published until every service removed
            // by this reload has stopped. A failed stop retains both ownership
            // and the retryable service record.
            let mut removed_service_names = {
                let services = self.services.lock().await;
                services
                    .iter()
                    .filter(|(name, service)| {
                        service.path == path && !desired_service_names.contains(name.as_str())
                    })
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>()
            };
            removed_service_names.sort();
            for name in &removed_service_names {
                info!("Service {name} removed from config, stopping before domain publication...");
                self.stop(name).await?;
            }

            // `commit_candidate` advances the in-memory catalog at the atomic
            // rename commit point, including PublishedNotDurable.
            let commit_result = registry.commit_candidate(candidate).await;
            let catalog_published = commit_result.is_ok()
                || matches!(
                    &commit_result,
                    Err(CatalogError::PublishedNotDurable { .. })
                );
            let published_domain_index = catalog_published.then(|| registry.domain_index().clone());
            (commit_result, removed_service_names, published_domain_index)
        };

        // The catalog rename is the ownership commit point. Synchronize hosts
        // from that exact snapshot even when the parent-directory fsync reports
        // PublishedNotDurable, then surface the durability result. Removed
        // service records leave runtime state at the same publication point.
        if let Some(published_domain_index) = published_domain_index {
            self.domain_index.store(published_domain_index);
            {
                let mut services = self.services.lock().await;
                for name in &removed_service_names {
                    services.remove(name);
                }
            }
            self.persist_state().await;
            self.sync_hosts_after_catalog_publish().await;
        }
        commit_result?;

        if let Some(tx) = &event_tx {
            let _ = tx
                .send(BootEvent::StepFinished {
                    id: "config".to_string(),
                    result: Ok(()),
                })
                .await;
        }

        for service_name in sorted_services {
            let service_config = &config.services[&service_name];
            info!(
                "Service {}:{} config: {:?}",
                config.project.name, service_name, service_config
            );
            let name = format!("{}:{}", config.project.name, service_name);

            let (combined_env, injected_database) =
                Self::effective_service_env(&config, &dot_env_vars, service_config);
            if let Some(dependency) = injected_database {
                info!(
                    "Auto-injected DATABASE_URL for {name} from Postgres dependency {dependency}"
                );
            }

            let manager = self.clone();
            let lookup = move |service_name: String, field: String| {
                let manager = manager.clone();
                async move { manager.get_service_field(&service_name, &field).await }
            };

            let resolved_env = ConfigLoader::resolve_env(&combined_env, &config, lookup).await?;

            // A domain-only reload updates the shared claim snapshot and the
            // service's display configuration without restarting its process.
            let is_up_to_date = {
                let mut services = self.services.lock().await;
                if let Some(service) = services.get_mut(&name) {
                    // Check if actually running
                    let is_running = match &mut service.runtime_state {
                        ServiceRuntime::Controller(c) => {
                            c.lock().await.read_state().await.status
                                == locald_core::state::ServiceState::Running
                        }
                        ServiceRuntime::None => false,
                    };

                    if is_running {
                        if &service.service_config == service_config
                            && service.resolved_env == resolved_env
                        {
                            service.config = config.clone();
                            service.path.clone_from(&path);
                            true
                        } else {
                            info!("Service {name} config changed, restarting...");
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            if is_up_to_date {
                info!("Service {name} is already running and up to date");
                if let Some(tx) = &event_tx {
                    let _ = tx
                        .send(BootEvent::StepStarted {
                            id: name.clone(),
                            description: format!("Service {name} up to date"),
                        })
                        .await;
                    let _ = tx
                        .send(BootEvent::StepFinished {
                            id: name.clone(),
                            result: Ok(()),
                        })
                        .await;
                }
                self.broadcast_service_update(&name).await;
                continue;
            }

            // Stop if running (restarting)
            self.stop(&name).await?;

            let needs_port = !matches!(
                service_config,
                ServiceConfig::Typed(TypedServiceConfig::Worker(_))
            );

            info!(
                "Service {name}: needs_port={needs_port}, config type={:?}",
                service_config
            );

            // Find free port or use configured port
            // Use PortGuard to prevent race conditions between parallel service starts

            let (port, mut port_guard): (Option<u16>, Option<crate::port_allocator::PortGuard>) =
                if !needs_port {
                    (None, None)
                } else if let Some(p) = service_config.port() {
                    (Some(p), None)
                } else {
                    // Check for sticky port
                    let sticky = {
                        let services = self.services.lock().await;
                        services.get(&name).and_then(|s| s.sticky_port)
                    };

                    if let Some(p) = sticky {
                        // Try to bind to sticky port to ensure it's free
                        if let Some(guard) = self.port_allocator.try_allocate_specific(p) {
                            info!("Reusing sticky port {p} for service {name}");
                            (Some(p), Some(guard))
                        } else {
                            warn!(
                                "Sticky port {p} for service {name} is taken, assigning new port"
                            );
                            let guard = self.port_allocator.allocate()?;
                            (Some(guard.port()), Some(guard))
                        }
                    } else {
                        let guard = self.port_allocator.allocate()?;
                        (Some(guard.port()), Some(guard))
                    }
                };

            info!("Starting service {name} on port {:?}", port);

            if let Some(tx) = &event_tx {
                let _ = tx
                    .send(BootEvent::StepStarted {
                        id: name.clone(),
                        description: format!("Starting service {}", name),
                    })
                    .await;
            }

            let mut handled = false;
            for factory in &self.factories {
                if factory.can_handle(service_config) {
                    info!("Using factory for service {}", name);
                    let ctx = ServiceContext {
                        project_root: path.clone(),
                        port,
                        env: resolved_env.clone(),
                    };

                    // Release the port guard's listener so the service can bind.
                    // The guard stays alive (preventing re-allocation) until we're done.
                    if let Some(ref mut guard) = port_guard {
                        guard.release_listener();
                    }

                    let controller = factory.create(name.clone(), service_config, &ctx);

                    // Hook up logs immediately so we catch build logs
                    let manager = self.clone();
                    let controller_logs = {
                        let c = controller.lock().await;
                        c.logs().await
                    };
                    tokio::spawn(async move {
                        let mut logs = controller_logs;
                        while let Some(entry) = logs.next().await {
                            manager.broadcast_log(entry);
                        }
                    });

                    // Insert into map immediately so status is visible
                    {
                        let mut services = self.services.lock().await;
                        services.insert(
                            name.clone(),
                            Service {
                                config: config.clone(),
                                service_config: service_config.clone(),
                                resolved_env: resolved_env.clone(),
                                runtime_state: ServiceRuntime::Controller(controller.clone()),
                                sticky_port: port,
                                path: path.clone(),
                                health_status: HealthStatus::Unknown,
                                health_source: HealthSource::None,
                                warnings: Vec::new(),
                            },
                        );
                    }

                    self.broadcast_service_update(&name).await;

                    {
                        let mut c = controller.lock().await;
                        c.prepare().await.context("Failed to prepare service")?;
                        // Broadcast update after prepare (state might be Building)
                    }
                    self.broadcast_service_update(&name).await;

                    {
                        let mut c = controller.lock().await;
                        c.start().await.context("Failed to start service")?;
                    }

                    let state = controller.lock().await.read_state().await;

                    // Update service with final state (port might have changed if dynamic?)
                    {
                        let mut services = self.services.lock().await;
                        if let Some(service) = services.get_mut(&name) {
                            service.sticky_port = state.port;
                            service.health_status = state.health_status;
                        }
                    }

                    self.broadcast_service_update(&name).await;

                    self.health_monitor.spawn_check(
                        name.clone(),
                        service_config,
                        state.port,
                        state.pid,
                        None,
                        Some(path.clone()),
                    );

                    handled = true;
                    break;
                }
            }

            if !handled {
                anyhow::bail!("No factory found for service {name}");
            }

            // Wait for health before starting next service (which might depend on this one)
            info!("Waiting for service {} to be ready...", name);
            if let Err(e) = self.wait_for_health(&name).await {
                error!("Dependency failed: {}", e);
                return Err(e);
            }

            if let Some(tx) = &event_tx {
                let _ = tx
                    .send(BootEvent::StepFinished {
                        id: name.clone(),
                        result: Ok(()),
                    })
                    .await;
            }
        }

        self.persist_state().await;
        Ok(())
    }

    /// Stops a specific service by name.
    ///
    /// This method:
    /// 1. Identifies the runtime type (Process, Container, Postgres).
    /// 2. Sends the configured stop signal (default: SIGTERM).
    /// 3. Cleans up associated resources (containers, PTYs).
    /// 4. Persists the new state.
    ///
    /// # Errors
    ///
    /// Returns an error if the service state cannot be persisted, though
    /// cleanup errors are generally logged as warnings rather than returned.
    pub async fn stop(&self, name: &str) -> Result<()> {
        let runtime_state = {
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(name) {
                std::mem::replace(&mut service.runtime_state, ServiceRuntime::None)
            } else {
                return Ok(());
            }
        };

        match runtime_state {
            ServiceRuntime::Controller(c) => {
                let stop_result = c.lock().await.stop().await;
                if let Err(e) = stop_result {
                    warn!("Failed to stop service {name}: {e}");
                    // Restore the controller — the service is still running
                    let mut services = self.services.lock().await;
                    if let Some(service) = services.get_mut(name) {
                        service.runtime_state = ServiceRuntime::Controller(c);
                    }
                    return Err(e).with_context(|| format!("failed to stop service `{name}`"));
                }
            }
            ServiceRuntime::None => {}
        }

        // Clear health and broadcast after stop
        {
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(name) {
                // Note: We do NOT clear sticky_port here, so we can reuse it on restart.
                service.health_status = HealthStatus::Unknown;
            }
        }

        self.persist_state().await;
        self.broadcast_service_update(name).await;
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<()> {
        let names: Vec<String> = {
            let services = self.services.lock().await;
            services.keys().cloned().collect()
        };

        for name in names {
            if let Err(e) = self.stop(&name).await {
                error!("Failed to stop service {}: {}", name, e);
            }
        }
        Ok(())
    }

    pub async fn stop_project(&self, project_path: &Path) -> Result<()> {
        let service_names: Vec<String> = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter(|(_, service)| service.path == project_path)
                .map(|(name, _)| name.clone())
                .collect()
        };

        for name in service_names {
            self.stop(&name).await?;
        }
        Ok(())
    }

    pub async fn restart_all(&self) -> Result<()> {
        // 1. Collect unique project paths
        let paths: HashSet<PathBuf> = {
            let services = self.services.lock().await;
            services.values().map(|s| s.path.clone()).collect()
        };

        // 2. Stop all services
        self.stop_all().await?;

        // 3. Start each project
        for path in paths {
            if let Err(e) = self.start(path.clone(), None, false).await {
                error!("Failed to restart project at {:?}: {}", path, e);
            }
        }
        Ok(())
    }

    /// Resets a service to its initial state.
    ///
    /// This method:
    /// 1. Stops the service.
    /// 2. Clears any sticky port assignment.
    /// 3. Wipes data directories (for stateful services like Postgres).
    /// 4. Restarts the service.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The service cannot be stopped.
    /// - Data directories cannot be removed.
    /// - The service fails to restart.
    pub async fn reset(&self, name: &str) -> Result<()> {
        info!("Resetting service {}", name);

        // 1. Stop the service
        self.stop(name).await?;

        // Clear sticky port on reset
        {
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(name) {
                service.sticky_port = None;
            }
        }

        // 2. Wipe data (if applicable)
        let data_dir = {
            let services = self.services.lock().await;
            services.get(name).and_then(|service| {
                if matches!(
                    &service.service_config,
                    ServiceConfig::Typed(TypedServiceConfig::Postgres(_))
                ) {
                    // TODO: This path logic is duplicated. Should be centralized.
                    // For now, we only support resetting Postgres services which use this path.
                    // If we add other stateful services, we need a better way to know their data dir.
                    Some(Self::postgres_data_dir(name))
                } else {
                    None
                }
            })
        };

        if let Some(dir) = data_dir {
            if dir.exists() {
                info!("Removing data directory {:?}", dir);
                if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                    warn!(
                        "Failed to remove data directory: {}. Attempting privileged cleanup...",
                        e
                    );
                    // use locald_builder::ShimRuntime;
                    locald_builder::ShimRuntime::cleanup_path(&dir)
                        .await
                        .context("Failed to remove data directory (privileged)")?;
                }
            }
        }

        // 3. Restart (by calling start with the project path)
        // We need the project path.
        let path = {
            let services = self.services.lock().await;
            services.get(name).map(|s| s.path.clone())
        };

        if let Some(path) = path {
            self.start(path, None, false).await?;
        } else {
            anyhow::bail!("Service {name} not found");
        }

        Ok(())
    }

    pub async fn list(&self) -> Vec<ServiceStatus> {
        let proxy_ports = { *self.proxy_ports.lock().await };
        let mut snapshots = Vec::new();

        {
            let mut services = self.services.lock().await;

            // First pass: Reap dead processes
            for (name, service) in services.iter_mut() {
                Self::reap_dead_services(name, service);
            }

            // Second pass: Collect snapshots
            for (name, service) in services.iter() {
                let snapshot = match &service.runtime_state {
                    ServiceRuntime::Controller(c) => RuntimeSnapshot::Controller(c.clone()),
                    ServiceRuntime::None => RuntimeSnapshot::Static {
                        is_running: false,
                        pid: None,
                        port: None,
                    },
                };

                snapshots.push((
                    name.clone(),
                    self.domain_for_service(name),
                    Some(service.path.clone()),
                    service.health_status,
                    service.health_source,
                    snapshot,
                    service.service_config.clone(),
                    service.config.project.workspace.clone(),
                    service.config.project.constellation.clone(),
                    service.warnings.clone(),
                ));
            }
        }

        let mut results = Vec::new();
        for (
            name,
            domain,
            path,
            health_status,
            health_source,
            snapshot,
            service_config,
            workspace,
            constellation,
            warnings,
        ) in snapshots
        {
            results.push(
                Self::build_service_status(
                    name,
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
                .await,
            );
        }
        results
    }

    pub async fn shutdown(&self) -> Result<()> {
        let mut controllers_to_stop = Vec::new();

        {
            let mut services = self.services.lock().await;
            for (name, service) in services.iter_mut() {
                let runtime_state =
                    std::mem::replace(&mut service.runtime_state, ServiceRuntime::None);

                match runtime_state {
                    ServiceRuntime::Controller(c) => {
                        controllers_to_stop.push((name.clone(), c));
                    }
                    ServiceRuntime::None => {}
                }
            }
        }

        // Parallel shutdown for Controllers
        let mut futures = Vec::new();
        for (name, controller) in controllers_to_stop {
            futures.push(async move {
                if let Err(e) = controller.lock().await.stop().await {
                    warn!("Failed to stop service {}: {}", name, e);
                }
            });
        }
        futures_util::future::join_all(futures).await;

        Ok(())
    }

    pub async fn resolve_service_by_domain(
        &self,
        domain: &str,
    ) -> Option<locald_core::resolver::DomainResolution> {
        let service_name = {
            let index = self.domain_index.snapshot();
            match index.resolve(domain) {
                Some(DomainTarget::Service {
                    service_name: Some(service_name),
                    ..
                }) => service_name.clone(),
                Some(DomainTarget::Service {
                    service_name: None, ..
                }) => return Some(locald_core::resolver::DomainResolution::OwnershipOnly),
                Some(DomainTarget::Platform { .. }) | None => return None,
            }
        };
        let runtime = {
            let services = self.services.lock().await;
            services
                .get(&service_name)
                .map(|service| service.runtime_state.clone())
        };
        match runtime {
            None => Some(locald_core::resolver::DomainResolution::OwnershipOnly),
            Some(ServiceRuntime::None) => Some(locald_core::resolver::DomainResolution::Service {
                name: service_name,
                port: None,
                status: locald_core::state::ServiceState::Stopped,
            }),
            Some(ServiceRuntime::Controller(controller)) => {
                let runtime = controller.lock().await.read_state().await;
                let port = (runtime.status == locald_core::state::ServiceState::Running)
                    .then_some(runtime.port)
                    .flatten();
                Some(locald_core::resolver::DomainResolution::Service {
                    name: service_name,
                    port,
                    status: runtime.status,
                })
            }
        }
    }

    pub async fn registry_list(&self) -> Vec<locald_core::registry::ProjectEntry> {
        let registry = self.registry.lock().await;
        registry.project_entries()
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn project_attach(
        &self,
        project_path: PathBuf,
        source: AttachmentSource,
    ) -> Result<()> {
        anyhow::ensure!(
            !matches!(&source, AttachmentSource::Runtime),
            "Runtime attachment evidence is accepted only from persisted legacy state"
        );
        // Canonicalize to prevent duplicate attachments from different relative paths.
        let canonical =
            std::fs::canonicalize(&project_path).unwrap_or_else(|_| project_path.clone());

        let is_first = {
            let mut attachments = self.attachments.lock().await;
            attachments.reap_stale_attachments_for(&canonical);
            let attachment = Attachment {
                project_path: canonical.clone(),
                source,
                created_at: SystemTime::now(),
            };
            let first = attachments.attach(attachment)?;
            attachments.clear_stopped(&canonical);
            if let Err(e) = attachments.save().await {
                error!("Failed to save attachments: {e}");
            }
            first
        };

        if is_first {
            info!(
                "First attachment for {}, starting services",
                canonical.display()
            );
            if let Err(e) = self.start(canonical.clone(), None, false).await {
                // Roll back attachment so future attaches retry start.
                warn!("Failed to start project on attach: {e}");
                let mut attachments = self.attachments.lock().await;
                attachments.detach_all_non_pin(&canonical);
                let _ = attachments.save().await;
                return Err(e);
            }
        }
        Ok(())
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn project_detach(
        &self,
        project_path: PathBuf,
        source: Option<AttachmentSource>,
    ) -> Result<()> {
        anyhow::ensure!(
            !matches!(&source, Some(AttachmentSource::Runtime)),
            "Runtime attachment evidence is retained for the availability migration"
        );
        let canonical = Self::canonicalize_path(&project_path);
        let should_stop = {
            let mut attachments = self.attachments.lock().await;
            let is_last = if let Some(source) = &source {
                attachments.detach(&canonical, source)
            } else {
                attachments.detach_all_non_pin(&canonical)
            };
            if let Err(e) = attachments.save().await {
                error!("Failed to save attachments: {e}");
            }
            is_last && !attachments.is_stopped(&canonical)
        };

        if should_stop {
            info!(
                "Last attachment removed for {}, stopping services",
                canonical.display()
            );
            self.stop_project(&canonical).await?;
        }
        Ok(())
    }

    pub async fn project_force_start(&self, project_path: PathBuf) -> Result<()> {
        {
            let mut attachments = self.attachments.lock().await;
            attachments.clear_stopped(&project_path);
            let _ = attachments.save().await;
        }
        self.start(project_path, None, false).await
    }

    pub async fn project_force_stop(&self, project_path: PathBuf) -> Result<()> {
        {
            let mut attachments = self.attachments.lock().await;
            attachments.mark_stopped(&project_path);
            let _ = attachments.save().await;
        }
        self.stop_project(&project_path).await
    }

    pub async fn remove_project(&self, project_path: &Path) -> Result<()> {
        let canonical = Self::canonicalize_path(project_path);

        // Stop services.
        self.stop_project(&canonical).await?;

        // Keep both compatibility stores stable until each candidate is ready.
        // Catalog removal is the authoritative commit point; recoverable
        // failures before it restore the attachment file before releasing either
        // store lock.
        let mut registry = self.registry.lock().await;
        let mut attachments = self.attachments.lock().await;
        let original_attachments = attachments.clone();
        let mut attachment_candidate = original_attachments.clone();
        attachment_candidate.forget_project(&canonical);
        let mut catalog_candidate = registry.clone();
        catalog_candidate.unregister_project(&canonical)?;

        attachment_candidate.save().await?;
        let commit_result = registry.commit_candidate(catalog_candidate).await;
        self.domain_index.store(registry.domain_index().clone());
        let durability_error = match commit_result {
            Ok(()) => {
                *attachments = attachment_candidate;
                None
            }
            Err(error @ CatalogError::PublishedNotDurable { .. }) => {
                *attachments = attachment_candidate;
                Some(error)
            }
            Err(catalog_error) => {
                original_attachments.save().await.with_context(|| {
                    format!(
                        "catalog removal failed ({catalog_error}); failed to restore attachment state"
                    )
                })?;
                return Err(catalog_error.into());
            }
        };

        drop(attachments);
        drop(registry);

        self.services
            .lock()
            .await
            .retain(|_, service| service.path != canonical);
        self.sync_hosts_after_catalog_publish().await;

        if let Some(error) = durability_error {
            return Err(error.into());
        }

        Ok(())
    }

    pub async fn project_status(&self, project_path: &Path) -> Result<ProjectStatusInfo> {
        self.refresh_attachments().await?;
        let canonical = Self::canonicalize_path(project_path);

        let project_name = {
            let registry = self.registry.lock().await;
            registry
                .get_project(&canonical)
                .and_then(|entry| entry.name)
        };

        let attachments = {
            let attachments = self.attachments.lock().await;
            attachments
                .attachments_for(&canonical)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        };

        let statuses = self.list().await;
        let mut services = Vec::new();
        let mut service_details = Vec::new();
        let mut is_running = false;

        for status in statuses {
            if status.path.as_ref() == Some(&canonical) {
                if status.status == ServiceState::Running {
                    is_running = true;
                }
                services.push(status.name.clone());
                service_details.push(status);
            }
        }

        Ok(ProjectStatusInfo {
            project_path: canonical,
            project_name,
            attachments,
            is_running,
            services,
            service_details,
        })
    }

    pub async fn project_list(
        &self,
        filter: Option<ProjectFilter>,
    ) -> Result<Vec<ProjectListEntry>> {
        self.refresh_attachments().await?;

        let registry_projects = {
            let registry = self.registry.lock().await;
            registry.project_entries_by_path()
        };

        let attachment_projects = {
            let attachments = self.attachments.lock().await;
            attachments.all_projects()
        };

        let mut all_projects = HashSet::new();
        for path in registry_projects.keys() {
            all_projects.insert(path.clone());
        }
        for path in attachment_projects {
            all_projects.insert(path);
        }

        let statuses = self.list().await;
        let mut running_by_path: HashMap<PathBuf, bool> = HashMap::new();

        for status in statuses {
            let Some(path) = status.path else {
                continue;
            };
            if status.status == ServiceState::Running {
                running_by_path.insert(path, true);
            }
        }

        let mut entries = Vec::new();
        let filter = filter.unwrap_or(ProjectFilter::All);
        let attachments = self.attachments.lock().await;
        for path in all_projects {
            let attachments_for = attachments
                .attachments_for(&path)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let section = attachments.section_for(&path);
            let is_running = running_by_path.get(&path).copied().unwrap_or(false);
            let project_name = registry_projects
                .get(&path)
                .and_then(|entry| entry.name.clone());

            let entry = ProjectListEntry {
                project_path: path,
                project_name,
                attachments: attachments_for,
                is_running,
                section,
            };

            let include = match &filter {
                ProjectFilter::All => true,
                ProjectFilter::Active => entry.section == ProjectSection::Active,
                ProjectFilter::Pinned => entry.section == ProjectSection::AlwaysOn,
                ProjectFilter::Recent => entry.section == ProjectSection::Recent,
            };

            if include {
                entries.push(entry);
            }
        }

        entries.sort_by(|a, b| a.project_path.cmp(&b.project_path));
        Ok(entries)
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_pin(&self, path: &std::path::Path) -> Result<()> {
        let mut registry = self.registry.lock().await;
        let mut updated = registry.clone();
        if updated.pin_project(path) {
            registry.commit_candidate(updated).await?;
            Ok(())
        } else {
            anyhow::bail!("Project not found in registry")
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_unpin(&self, path: &std::path::Path) -> Result<()> {
        let mut registry = self.registry.lock().await;
        let mut updated = registry.clone();
        if updated.unpin_project(path) {
            registry.commit_candidate(updated).await?;
            Ok(())
        } else {
            anyhow::bail!("Project not found in registry")
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_clean(&self) -> Result<usize> {
        let (count, removed_paths, commit_result) = {
            let mut registry = self.registry.lock().await;
            let mut updated = registry.clone();
            let count = updated.prune_missing_projects()?;
            let removed_paths = registry
                .instances
                .iter()
                .filter(|(instance_id, _)| !updated.instances.contains_key(instance_id))
                .map(|(_, record)| record.last_known_path.clone())
                .collect::<HashSet<_>>();

            for path in &removed_paths {
                self.stop_project(path).await?;
            }

            let commit_result = if updated == *registry {
                None
            } else {
                let commit_result = registry.commit_candidate(updated).await;
                self.domain_index.store(registry.domain_index().clone());
                Some(commit_result)
            };
            (count, removed_paths, commit_result)
        };
        if let Some(commit_result) = commit_result {
            let catalog_published = commit_result.is_ok()
                || matches!(
                    &commit_result,
                    Err(CatalogError::PublishedNotDurable { .. })
                );
            if catalog_published {
                self.services
                    .lock()
                    .await
                    .retain(|_, service| !removed_paths.contains(&service.path));
                self.sync_hosts_after_catalog_publish().await;
            }
            commit_result?;
        }
        Ok(count)
    }

    async fn refresh_attachments(&self) -> Result<()> {
        let mut attachments = self.attachments.lock().await;
        let removed = attachments.reap_stale_attachments();
        if !removed.is_empty() {
            attachments.save().await?;
        }
        Ok(())
    }

    pub async fn reap_and_stop_orphans(&self) {
        let orphaned = {
            let mut attachments = self.attachments.lock().await;
            let orphaned = attachments.reap_stale_attachments();
            if !orphaned.is_empty() {
                let _ = attachments.save().await;
            }
            orphaned
        };

        for path in orphaned {
            info!(
                "Stale attachments reaped for {}, stopping services",
                path.display()
            );
            if let Err(e) = self.stop_project(&path).await {
                warn!("Failed to stop orphaned project: {e}");
            }
        }
    }

    fn canonicalize_path(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    pub async fn get_service_path(&self, name: &str) -> Option<PathBuf> {
        let services = self.services.lock().await;
        services.get(name).map(|s| s.path.clone())
    }

    pub async fn get_service_env(&self, name: &str) -> Result<HashMap<String, String>> {
        let (config, service_config, path, port_result, sticky_port) = {
            let services = self.services.lock().await;
            let service = services
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Service {name} not found"))?;

            let port_result = match &service.runtime_state {
                ServiceRuntime::Controller(c) => Err(c.clone()),
                ServiceRuntime::None => Ok(None),
            };

            (
                service.config.clone(),
                service.service_config.clone(),
                service.path.clone(),
                port_result,
                service.sticky_port,
            )
        };

        let port = match port_result {
            Ok(p) => p,
            Err(c) => c.lock().await.read_state().await.port,
        };

        // Load .env if exists
        let env_path = path.join(".env");
        let mut combined_env = HashMap::new();
        if env_path.exists() {
            if let Ok(iter) = dotenvy::from_path_iter(&env_path) {
                for (k, v) in iter.flatten() {
                    combined_env.insert(k, v);
                }
            }
        }

        for (k, v) in service_config.env() {
            combined_env.insert(k.clone(), v.clone());
        }

        if let Some(p) = port.or(sticky_port) {
            combined_env.insert("PORT".to_string(), p.to_string());
        }

        // Auto-inject DATABASE_URL for services that depend on Postgres
        // (mirrors the logic in start_project)
        if !combined_env.contains_key("DATABASE_URL") {
            for dep in service_config.depends_on() {
                if let Some(dep_config) = config.services.get(dep) {
                    if matches!(
                        dep_config,
                        ServiceConfig::Typed(TypedServiceConfig::Postgres(_))
                    ) {
                        // Use local service name - resolve_env adds the project prefix
                        combined_env.insert(
                            "DATABASE_URL".to_string(),
                            format!("${{services.{}.url}}", dep),
                        );
                        break;
                    }
                }
            }
        }

        let manager = self.clone();
        let lookup = move |service_name: String, field: String| {
            let manager = manager.clone();
            async move { manager.get_service_field(&service_name, &field).await }
        };

        let resolved_env = ConfigLoader::resolve_env(&combined_env, &config, lookup).await?;
        Ok(resolved_env)
    }

    /// Inspects the runtime details of a service.
    ///
    /// Returns a JSON value containing:
    /// - Configuration
    /// - PID / Container ID
    /// - Port assignments
    /// - Health status
    ///
    /// # Errors
    ///
    /// Returns an error if the service is not found.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn inspect(&self, name: &str) -> Result<serde_json::Value> {
        let proxy_ports = { *self.proxy_ports.lock().await };
        let (service_config, path, health_status, health_source, runtime_info, domain, warnings) = {
            let services = self.services.lock().await;
            let service = services
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Service not found"))?;

            let short_name = Self::configured_service_name(name, &service.config);
            let config = service.config.services.get(short_name).cloned();

            let runtime_info = match &service.runtime_state {
                ServiceRuntime::Controller(c) => Err(c.clone()),
                ServiceRuntime::None => Ok((None::<u32>, None::<String>, None::<u16>, false)),
            };

            (
                config,
                service.path.clone(),
                service.health_status,
                service.health_source,
                runtime_info,
                self.domain_for_service(name),
                service.warnings.clone(),
            )
        };

        let (pid, container_id, port, status) = match runtime_info {
            Ok((pid, container_id, port, is_running)) => (
                pid,
                container_id,
                port,
                if is_running {
                    locald_core::state::ServiceState::Running
                } else {
                    locald_core::state::ServiceState::Stopped
                },
            ),
            Err(c) => {
                let state = c.lock().await.read_state().await;
                (state.pid, None, state.port, state.status)
            }
        };

        let url = if status == locald_core::state::ServiceState::Running && port.is_some() {
            domain.as_ref().map_or_else(
                || port.map(|p| format!("http://localhost:{p}")),
                |d| {
                    let (proxy_http, proxy_https) = proxy_ports;
                    if let Some(p) = proxy_https {
                        if p == 443 {
                            Some(format!("https://{d}"))
                        } else {
                            Some(format!("https://{d}:{p}"))
                        }
                    } else if let Some(p) = proxy_http {
                        if p == 80 {
                            Some(format!("http://{d}"))
                        } else {
                            Some(format!("http://{d}:{p}"))
                        }
                    } else {
                        // Default to HTTPS (implied port 443)
                        Some(format!("https://{d}"))
                    }
                },
            )
        } else {
            None
        };

        let mut info = serde_json::json!({
            "name": name,
            "pid": pid,
            "container_id": container_id,
            "port": port,
            "path": path,
            "health_status": health_status,
            "health_source": health_source,
            "domain": domain,
            "url": url,
            "warnings": warnings,
        });

        if let Some(obj) = info.as_object_mut() {
            obj.insert("config".to_string(), serde_json::to_value(&service_config)?);
        }

        Ok(info)
    }

    pub fn spawn_metrics_collector(&self) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                manager.collect_metrics().await;
            }
        });
    }

    async fn collect_metrics(&self) {
        let services = {
            let services = self.services.lock().await;
            services
                .values()
                .map(|s| s.runtime_state.clone())
                .collect::<Vec<_>>()
        };

        for runtime in services {
            if let ServiceRuntime::Controller(c) = runtime {
                let metrics = {
                    let c = c.lock().await;
                    c.metrics().await
                };

                if let Ok(Some(m)) = metrics {
                    let _ = self.event_sender.send(Event::Metrics(m));
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl ServiceResolver for ProcessManager {
    async fn resolve_service_by_domain(
        &self,
        domain: &str,
    ) -> Option<locald_core::resolver::DomainResolution> {
        self.resolve_service_by_domain(domain).await
    }
    async fn set_http_port(&self, port: Option<u16>) {
        self.set_http_port(port).await;
    }
    async fn set_https_port(&self, port: Option<u16>) {
        self.set_https_port(port).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::{StreamExt, stream};
    use locald_core::config::{ExecServiceConfig, LocaldConfig, ProjectConfig, ServiceConfig};
    use locald_core::registry::Registry;
    use locald_core::service::{RuntimeState, ServiceCommand};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    #[derive(Debug)]
    struct TestController {
        id: String,
        state: RuntimeState,
        fail_stop: bool,
    }

    impl TestController {
        fn new(id: impl Into<String>, state: RuntimeState) -> Self {
            Self {
                id: id.into(),
                state,
                fail_stop: false,
            }
        }

        fn failing_stop(id: impl Into<String>, state: RuntimeState) -> Self {
            Self {
                id: id.into(),
                state,
                fail_stop: true,
            }
        }
    }

    #[async_trait]
    impl ServiceController for TestController {
        fn id(&self) -> &str {
            &self.id
        }

        async fn prepare(&mut self) -> Result<()> {
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            if self.fail_stop {
                anyhow::bail!("injected stop failure");
            }
            Ok(())
        }

        async fn read_state(&self) -> RuntimeState {
            self.state
        }

        async fn logs(&self) -> futures::stream::BoxStream<'static, LogEntry> {
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

    struct NoopHostSyncer;

    #[async_trait]
    impl HostSyncer for NoopHostSyncer {
        async fn sync(&self, _domains: Vec<String>) -> Result<()> {
            Ok(())
        }
    }

    struct RecordingHostSyncer {
        calls: Arc<StdMutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl HostSyncer for RecordingHostSyncer {
        async fn sync(&self, domains: Vec<String>) -> Result<()> {
            self.calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .push(domains);
            Ok(())
        }
    }

    fn test_instance_id() -> ProjectInstanceId {
        "00000000-0000-4000-8000-000000000001"
            .parse()
            .expect("valid project instance ID")
    }

    fn config_with_services(project: ProjectConfig, service_names: &[&str]) -> LocaldConfig {
        LocaldConfig {
            project,
            services: service_names
                .iter()
                .map(|name| {
                    (
                        (*name).to_owned(),
                        ServiceConfig::Legacy(ExecServiceConfig::default()),
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    fn claim_domains(config: &LocaldConfig) -> HashMap<String, String> {
        ProcessManager::build_domain_claims(test_instance_id(), config)
            .expect("valid claims")
            .into_iter()
            .filter_map(|claim| match claim.target {
                DomainTarget::Service {
                    service_name: Some(service_name),
                    ..
                } => Some((service_name, claim.domain.to_string())),
                DomainTarget::Platform { .. }
                | DomainTarget::Service {
                    service_name: None, ..
                } => None,
            })
            .collect()
    }

    fn install_test_claim(manager: &ProcessManager, domain: &str, service_name: &str) {
        let current = manager.domain_index.snapshot();
        let replacement = current
            .replacing_instance(
                test_instance_id(),
                [DomainClaim::service(
                    domain.parse().expect("valid test domain"),
                    test_instance_id(),
                    service_name.to_owned(),
                )],
            )
            .expect("install test claim");
        manager.domain_index.store(replacement);
    }

    fn install_legacy_test_claim(manager: &ProcessManager, domain: &str) {
        let current = manager.domain_index.snapshot();
        let replacement = current
            .replacing_instance(
                test_instance_id(),
                [DomainClaim::legacy(
                    domain.parse().expect("valid test domain"),
                    test_instance_id(),
                )],
            )
            .expect("install legacy test claim");
        manager.domain_index.store(replacement);
    }

    #[test]
    fn effective_service_env_applies_overrides_before_reference_validation() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"

[services.web]
type = "worker"
command = "run-web"

[services.web.env]
API_URL = "https://api.example"
"#,
        )
        .expect("parse test config");
        let dot_env_vars =
            HashMap::from([("API_URL".to_owned(), "${services.missing.url}".to_owned())]);

        let (effective_env, injected_database) =
            ProcessManager::effective_service_env(&config, &dot_env_vars, &config.services["web"]);

        assert_eq!(
            effective_env.get("API_URL").map(String::as_str),
            Some("https://api.example")
        );
        assert!(injected_database.is_none());
        ConfigLoader::validate_env_references(&effective_env, &config, "web")
            .expect("shadowed project value is not active config");
    }

    #[test]
    fn configured_service_name_uses_the_exact_project_prefix() {
        let config = LocaldConfig {
            project: ProjectConfig {
                name: "team:app".to_owned(),
                domain: Some("app.localhost".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            ProcessManager::configured_service_name("team:app:db", &config),
            "db"
        );
        assert_eq!(
            ProcessManager::configured_service_name("another:web", &config),
            "another:web"
        );
    }

    #[test]
    fn test_get_service_domain_default() {
        let config = config_with_services(
            ProjectConfig {
                name: "myproject".to_string(),
                domain: None,
                ..Default::default()
            },
            &["web"],
        );

        assert_eq!(
            claim_domains(&config).get("myproject:web"),
            Some(&"myproject.localhost".to_owned())
        );
    }

    #[test]
    fn test_get_service_domain_sanitizes_only_implicit_project_names() {
        let config = config_with_services(
            ProjectConfig {
                name: "My_App v2!".to_owned(),
                domain: None,
                ..Default::default()
            },
            &["web", "api"],
        );
        let claims = claim_domains(&config);

        assert_eq!(
            claims.get("My_App v2!:web"),
            Some(&"my-app-v2.localhost".to_owned())
        );
        assert_eq!(
            claims.get("My_App v2!:api"),
            Some(&"api.my-app-v2.localhost".to_owned())
        );

        let colliding = config_with_services(
            ProjectConfig {
                name: "My App v2".to_owned(),
                domain: None,
                ..Default::default()
            },
            &["web"],
        );
        assert_eq!(
            claim_domains(&colliding).get("My App v2:web"),
            Some(&"my-app-v2.localhost".to_owned())
        );
    }

    #[test]
    fn explicit_project_and_service_domains_remain_strict() {
        let invalid_project_domain = config_with_services(
            ProjectConfig {
                name: "project".to_owned(),
                domain: Some("My_Project.localhost".to_owned()),
                ..Default::default()
            },
            &["web"],
        );
        assert!(
            ProcessManager::build_domain_claims(test_instance_id(), &invalid_project_domain)
                .expect_err("explicit project domains are not rewritten")
                .to_string()
                .contains("invalid exact base domain")
        );

        let invalid_service_domain = config_with_services(
            ProjectConfig {
                name: "project".to_owned(),
                domain: None,
                ..Default::default()
            },
            &["web", "api_v2"],
        );
        assert!(
            ProcessManager::build_domain_claims(test_instance_id(), &invalid_service_domain)
                .expect_err("service labels are not rewritten")
                .to_string()
                .contains("invalid exact domain label")
        );
    }

    #[test]
    fn test_get_service_domain_explicit() {
        let config = config_with_services(
            ProjectConfig {
                name: "myproject".to_string(),
                domain: Some("example.com".to_string()),
                ..Default::default()
            },
            &["api"],
        );

        assert_eq!(
            claim_domains(&config).get("myproject:api"),
            Some(&"api.example.com".to_owned())
        );
    }

    #[test]
    fn test_get_service_domain_main_service() {
        let config = config_with_services(
            ProjectConfig {
                name: "shop".to_string(),
                domain: Some("shop.local".to_string()),
                ..Default::default()
            },
            &["web", "shop", "api"],
        );
        let claims = claim_domains(&config);

        assert_eq!(claims.get("shop:web"), Some(&"shop.local".to_owned()));
        assert_eq!(claims.get("shop:shop"), Some(&"shop.shop.local".to_owned()));
        assert_eq!(claims.get("shop:api"), Some(&"api.shop.local".to_owned()));
    }

    #[test]
    fn test_postgres_reset_uses_xdg_path() {
        let name = "test-postgres";
        let expected = ProjectDirs::from("com", "locald", "locald")
            .map(|d| d.data_dir().join("postgres").join(name))
            .unwrap_or_else(|| PathBuf::from(".locald/postgres").join(name));

        let actual = ProcessManager::postgres_data_dir(name);

        assert_eq!(actual, expected);

        let workspace_local = PathBuf::from(".locald/services/postgres").join(name);
        assert!(!actual.ends_with(&workspace_local));
    }

    #[tokio::test]
    async fn test_url_generation_clean_ports() {
        let name = "test".to_string();
        let path = None;
        let health_status = locald_core::state::HealthStatus::Healthy;
        let health_source = locald_core::state::HealthSource::None;

        // Helper to create a running snapshot
        let running_snapshot = || RuntimeSnapshot::Static {
            is_running: true,
            pid: Some(123),
            port: Some(3000),
        };

        // Case 1: HTTP on port 80 -> No port in URL
        let status = ProcessManager::build_service_status(
            name.clone(),
            Some("app.test".to_string()),
            path.clone(),
            (Some(80), None), // HTTP=80, HTTPS=None
            health_status,
            health_source,
            running_snapshot(),
            None,
            None,
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(status.url, Some("http://app.test".to_string()));

        // Case 2: HTTPS on port 443 -> No port in URL
        let status = ProcessManager::build_service_status(
            name.clone(),
            Some("app.test".to_string()),
            path.clone(),
            (Some(80), Some(443)), // HTTP=80, HTTPS=443
            health_status,
            health_source,
            running_snapshot(),
            None,
            None,
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(status.url, Some("https://app.test".to_string()));

        // Case 3: HTTP on non-standard port -> Port in URL
        let status = ProcessManager::build_service_status(
            name.clone(),
            Some("app.test".to_string()),
            path.clone(),
            (Some(8080), None),
            health_status,
            health_source,
            running_snapshot(),
            None,
            None,
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(status.url, Some("http://app.test:8080".to_string()));

        // Case 4: HTTPS on non-standard port -> Port in URL
        let status = ProcessManager::build_service_status(
            name.clone(),
            Some("app.test".to_string()),
            path.clone(),
            (None, Some(8443)),
            health_status,
            health_source,
            running_snapshot(),
            None,
            None,
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(status.url, Some("https://app.test:8443".to_string()));

        // Case 5: Privileged ports (80/443). URLs should NOT contain port numbers.
        let status = ProcessManager::build_service_status(
            name.clone(),
            Some("myapp.localhost".to_string()),
            path.clone(),
            (Some(80), Some(443)),
            health_status,
            health_source,
            running_snapshot(),
            None,
            None,
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(status.url, Some("https://myapp.localhost".to_string()));
        // Verify no high port leaks into URLs
        assert!(!status.url.as_deref().unwrap_or("").contains("8443"));
        assert!(!status.url.as_deref().unwrap_or("").contains("8080"));
    }

    #[test]
    fn test_log_buffer_capacity() {
        let mut buffer = LogBuffer::new(3);
        let entry = LogEntry {
            service: "test".to_string(),
            message: "msg".to_string(),
            stream: locald_core::ipc::LogStream::Stdout,
            timestamp: 0,
        };

        buffer.push(entry.clone());
        buffer.push(entry.clone());
        buffer.push(entry.clone());
        assert_eq!(buffer.get_all().len(), 3);

        buffer.push(entry.clone());
        assert_eq!(buffer.get_all().len(), 3);
    }

    #[test]
    fn test_log_buffer_fifo() {
        let mut buffer = LogBuffer::new(2);
        let entry1 = LogEntry {
            service: "test".to_string(),
            message: "1".to_string(),
            stream: locald_core::ipc::LogStream::Stdout,
            timestamp: 1,
        };
        let entry2 = LogEntry {
            service: "test".to_string(),
            message: "2".to_string(),
            stream: locald_core::ipc::LogStream::Stdout,
            timestamp: 2,
        };
        let entry3 = LogEntry {
            service: "test".to_string(),
            message: "3".to_string(),
            stream: locald_core::ipc::LogStream::Stdout,
            timestamp: 3,
        };

        buffer.push(entry1.clone());
        buffer.push(entry2.clone());

        let logs = buffer.get_all();
        assert_eq!(logs[0].message, "1");
        assert_eq!(logs[1].message, "2");

        buffer.push(entry3.clone());

        let logs = buffer.get_all();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].message, "2");
        assert_eq!(logs[1].message, "3");
    }

    #[tokio::test]
    async fn test_registry_clean_skips_persistence_when_catalog_is_unchanged() {
        let dir = tempdir().expect("create temporary directory");
        let catalog_path = dir.path().join("catalog.json");
        std::fs::create_dir(&catalog_path).expect("reserve catalog path as a directory");
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::with_path(catalog_path.clone())));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("create process manager");

        assert_eq!(
            manager
                .registry_clean()
                .await
                .expect("unchanged clean does not write the catalog"),
            0
        );
        assert!(catalog_path.is_dir());
    }

    #[tokio::test]
    async fn test_registry_clean_persists_reconciliation_without_pruning() {
        let dir = tempdir().expect("create temporary directory");
        let catalog_path = dir.path().join("catalog.json");
        let project_path = dir.path().join("pinned-project");
        std::fs::create_dir(&project_path).expect("create pinned project");
        let mut catalog = Registry::with_path(catalog_path.clone());
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover pinned project");
        let instance_id = catalog
            .register_project(discovery, Some("pinned".to_owned()))
            .expect("register pinned project");
        assert!(catalog.pin_project(&project_path));
        catalog.save().await.expect("persist active pinned project");
        let registry = Arc::new(Mutex::new(catalog));
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry,
            attachments,
            None,
        )
        .expect("create process manager");
        std::fs::remove_dir(&project_path).expect("remove pinned project");

        assert_eq!(
            manager
                .registry_clean()
                .await
                .expect("persist reconciled presence"),
            0
        );
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&catalog_path).expect("read reconciled catalog"))
                .expect("parse reconciled catalog");
        assert_eq!(
            persisted["instances"][instance_id.to_string()]["presence"],
            "missing"
        );
    }

    #[tokio::test]
    async fn registry_clean_synchronizes_released_domain_claims() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("missing-project");
        std::fs::create_dir(&project_path).expect("create project");
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project(discovery, Some("missing".to_owned()))
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "missing.localhost".parse().expect("valid domain"),
                    instance_id,
                    "missing:web".to_owned(),
                )],
            )
            .expect("record claim");
        catalog.save().await.expect("persist catalog");
        std::fs::remove_dir(&project_path).expect("remove project");

        let registry = Arc::new(Mutex::new(catalog));
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry,
            attachments,
            None,
        )
        .expect("create process manager");
        let calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: calls.clone(),
        }));

        assert_eq!(manager.registry_clean().await.expect("clean registry"), 1);
        assert_eq!(
            calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[Vec::<String>::new()]
        );
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("missing.localhost")
                .is_none()
        );
    }

    #[tokio::test]
    async fn registry_clean_preserves_claim_and_controller_when_stop_fails() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("busy-missing-project");
        std::fs::create_dir(&project_path).expect("create project");
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project(discovery, Some("busy-missing".to_owned()))
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "busy-missing.localhost".parse().expect("valid domain"),
                    instance_id,
                    "busy-missing:web".to_owned(),
                )],
            )
            .expect("record claim");
        catalog.save().await.expect("persist catalog");
        let canonical_path = std::fs::canonicalize(&project_path).expect("canonical project path");

        let registry = Arc::new(Mutex::new(catalog));
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        let controller = Arc::new(Mutex::new(TestController::failing_stop(
            "busy-missing:web",
            RuntimeState {
                pid: Some(42),
                port: Some(3000),
                status: ServiceState::Running,
                health_status: HealthStatus::Healthy,
            },
        )));
        manager.services.lock().await.insert(
            "busy-missing:web".to_owned(),
            test_service(
                test_config_with_domain("busy-missing", "busy-missing.localhost"),
                ServiceConfig::Legacy(ExecServiceConfig::default()),
                ServiceRuntime::Controller(controller),
                canonical_path,
            ),
        );
        let calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: calls.clone(),
        }));
        std::fs::remove_dir(&project_path).expect("remove project");

        let error = manager
            .registry_clean()
            .await
            .expect_err("stop failure must block cleanup");

        assert!(format!("{error:#}").contains("injected stop failure"));
        assert!(registry.lock().await.instances.contains_key(&instance_id));
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("busy-missing.localhost")
                .is_some()
        );
        assert!(
            manager
                .get_service_controller("busy-missing:web")
                .await
                .is_some()
        );
        assert!(
            calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_stop_project_targets_matching_paths() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::with_path(
            dir.path().join("catalog.json"),
        )));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("Failed to create ProcessManager");

        let project_path = dir.path().join("project");
        let other_path = dir.path().join("other");

        let service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        let service = |path: PathBuf| Service {
            config: LocaldConfig::default(),
            service_config: service_config.clone(),
            resolved_env: HashMap::new(),
            runtime_state: ServiceRuntime::None,
            sticky_port: None,
            path,
            health_status: HealthStatus::Healthy,
            health_source: HealthSource::None,
            warnings: Vec::new(),
        };

        {
            let mut services = manager.services.lock().await;
            services.insert("project:web".to_string(), service(project_path.clone()));
            services.insert("other:web".to_string(), service(other_path));
        }

        manager.stop_project(&project_path).await.unwrap();

        let services = manager.services.lock().await;
        assert_eq!(
            services.get("project:web").unwrap().health_status,
            HealthStatus::Unknown
        );
        assert_eq!(
            services.get("other:web").unwrap().health_status,
            HealthStatus::Healthy
        );
    }

    #[tokio::test]
    async fn test_project_detach_normalizes_the_path_before_stopping_services() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::with_path(
            dir.path().join("catalog.json"),
        )));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let manager = ProcessManager::new(
            notify_path,
            state_manager,
            registry,
            attachments.clone(),
            None,
        )
        .expect("create process manager");
        let project_path = dir.path().join("project");
        std::fs::create_dir(&project_path).expect("create project directory");
        let canonical = std::fs::canonicalize(&project_path).expect("canonical project path");
        let noncanonical = project_path.join("..").join("project");
        let source = AttachmentSource::Editor {
            name: "vscode".to_owned(),
            id: "window".to_owned(),
            pid: Some(std::process::id()),
        };
        attachments
            .lock()
            .await
            .attach(Attachment {
                project_path: canonical.clone(),
                source: source.clone(),
                created_at: SystemTime::now(),
            })
            .expect("attach editor");

        manager.services.lock().await.insert(
            "project:web".to_owned(),
            Service {
                config: LocaldConfig::default(),
                service_config: ServiceConfig::Legacy(ExecServiceConfig::default()),
                resolved_env: HashMap::new(),
                runtime_state: ServiceRuntime::None,
                sticky_port: None,
                path: canonical,
                health_status: HealthStatus::Healthy,
                health_source: HealthSource::None,
                warnings: Vec::new(),
            },
        );

        manager
            .project_detach(noncanonical, Some(source))
            .await
            .expect("detach last owner");

        assert!(
            attachments
                .lock()
                .await
                .attachments_for(&project_path)
                .is_empty()
        );
        assert_eq!(
            manager.services.lock().await["project:web"].health_status,
            HealthStatus::Unknown
        );
    }

    #[tokio::test]
    async fn test_catalog_persistence_failure_prevents_service_mutation() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let catalog_path = dir.path().join("catalog.json");
        std::fs::create_dir(&catalog_path).expect("create blocking catalog directory");
        let registry = Arc::new(Mutex::new(Registry::with_path(catalog_path)));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            notify_path,
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));

        let project_path = dir.path().join("project");
        std::fs::create_dir(&project_path).expect("create project directory");
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "catalog-failure"

[services.worker]
type = "worker"
command = "sleep 30"
"#,
        )
        .expect("write project config");

        manager
            .start(project_path, None, false)
            .await
            .expect_err("catalog persistence must block startup");

        assert!(manager.services.lock().await.is_empty());
        assert!(registry.lock().await.instances.is_empty());
    }

    #[tokio::test]
    async fn domain_only_reload_reuses_runtime_and_updates_exact_projections() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reload-project");
        std::fs::create_dir(&project_path).expect("create project directory");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::with_path(
            dir.path().join("catalog.json"),
        )));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        let host_sync_calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: host_sync_calls.clone(),
        }));
        let write_config = |domain: &str| {
            std::fs::write(
                project_path.join("locald.toml"),
                format!(
                    r#"
[project]
name = "reload"
domain = "{domain}"

[services.web]
type = "worker"
command = "sleep 30"
"#,
                ),
            )
            .expect("write project config");
        };

        write_config("first.localhost");
        manager
            .apply_config(project_path.clone(), None, false)
            .await
            .expect("start first domain");
        let first_controller = manager
            .get_service_controller("reload:web")
            .await
            .expect("first controller");

        write_config("second.localhost");
        manager
            .apply_config(project_path.clone(), None, false)
            .await
            .expect("reload second domain");
        let second_controller = manager
            .get_service_controller("reload:web")
            .await
            .expect("second controller");

        assert!(Arc::ptr_eq(&first_controller, &second_controller));
        assert!(
            manager
                .resolve_service_by_domain("first.localhost")
                .await
                .is_none()
        );
        assert_eq!(
            manager.list().await[0].domain.as_deref(),
            Some("second.localhost")
        );
        assert_eq!(
            manager
                .inspect("reload:web")
                .await
                .expect("inspect reloaded service")["domain"],
            "second.localhost"
        );
        assert!(
            registry
                .lock()
                .await
                .domain_index()
                .resolve("second.localhost")
                .is_some()
        );
        assert_eq!(
            host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[
                vec!["first.localhost".to_owned()],
                vec!["second.localhost".to_owned()]
            ]
        );

        manager
            .stop_project(&project_path)
            .await
            .expect("stop reloaded project");
    }

    #[tokio::test]
    async fn invalid_reload_preserves_previous_domain_and_runtime() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reload-project");
        std::fs::create_dir(&project_path).expect("create project directory");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::with_path(
            dir.path().join("catalog.json"),
        )));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        let host_sync_calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: host_sync_calls.clone(),
        }));

        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "reload"
domain = "first.localhost"

[services.web]
type = "worker"
command = "sleep 30"
"#,
        )
        .expect("write initial config");
        manager
            .apply_config(project_path.clone(), None, false)
            .await
            .expect("start initial config");
        let catalog_before =
            std::fs::read(dir.path().join("catalog.json")).expect("read initial catalog");
        let first_controller = manager
            .get_service_controller("reload:web")
            .await
            .expect("initial controller");

        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "reload"
domain = "second.localhost"

[services.web]
type = "worker"
command = "sleep 30"

[services.web.env]
BROKEN_URL = "${services.missing.url}"
"#,
        )
        .expect("write invalid reload");
        let error = manager
            .apply_config(project_path.clone(), None, false)
            .await
            .expect_err("invalid reload must fail before publication");

        assert!(format!("{error:#}").contains("unknown service `missing`"));
        assert!(Arc::ptr_eq(
            &first_controller,
            &manager
                .get_service_controller("reload:web")
                .await
                .expect("initial controller remains")
        ));
        assert!(
            manager
                .resolve_service_by_domain("first.localhost")
                .await
                .is_some()
        );
        assert!(
            manager
                .resolve_service_by_domain("second.localhost")
                .await
                .is_none()
        );
        assert!(
            registry
                .lock()
                .await
                .domain_index()
                .resolve("first.localhost")
                .is_some()
        );
        assert_eq!(
            std::fs::read(dir.path().join("catalog.json"))
                .expect("read catalog after rejected reload"),
            catalog_before
        );
        assert_eq!(
            host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[vec!["first.localhost".to_owned()]]
        );

        manager
            .stop_project(&project_path)
            .await
            .expect("stop initial project");
    }

    #[tokio::test]
    async fn removed_service_stop_failure_preserves_all_domain_claims() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reload-project");
        std::fs::create_dir(&project_path).expect("create project directory");
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "reload"
domain = "reload.localhost"
"#,
        )
        .expect("write replacement config");
        let canonical_path = std::fs::canonicalize(&project_path).expect("canonical project path");
        let catalog_path = dir.path().join("catalog.json");
        let mut catalog = Registry::with_path(catalog_path.clone());
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project(discovery, Some("reload".to_owned()))
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [
                    DomainClaim::service(
                        "a.reload.localhost".parse().expect("valid domain"),
                        instance_id,
                        "reload:a".to_owned(),
                    ),
                    DomainClaim::service(
                        "z.reload.localhost".parse().expect("valid domain"),
                        instance_id,
                        "reload:z".to_owned(),
                    ),
                ],
            )
            .expect("record existing claims");
        catalog.save().await.expect("persist existing catalog");
        let catalog_before = std::fs::read(&catalog_path).expect("read existing catalog");

        let registry = Arc::new(Mutex::new(catalog));
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        let running_state = RuntimeState {
            pid: Some(42),
            port: Some(3000),
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        let successful_controller = Arc::new(Mutex::new(TestController::new(
            "reload:a",
            running_state.clone(),
        )));
        let failing_controller: Arc<Mutex<dyn ServiceController>> = Arc::new(Mutex::new(
            TestController::failing_stop("reload:z", running_state),
        ));
        let service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        let previous_config = test_config_with_domain("reload", "reload.localhost");
        {
            let mut services = manager.services.lock().await;
            services.insert(
                "reload:a".to_owned(),
                test_service(
                    previous_config.clone(),
                    service_config.clone(),
                    ServiceRuntime::Controller(successful_controller),
                    canonical_path.clone(),
                ),
            );
            services.insert(
                "reload:z".to_owned(),
                test_service(
                    previous_config,
                    service_config,
                    ServiceRuntime::Controller(failing_controller.clone()),
                    canonical_path,
                ),
            );
        }
        let host_sync_calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: host_sync_calls.clone(),
        }));

        let error = manager
            .apply_config(project_path, None, false)
            .await
            .expect_err("a removed service stop failure must block publication");

        assert!(format!("{error:#}").contains("injected stop failure"));
        assert_eq!(
            std::fs::read(&catalog_path).expect("read catalog after failed reload"),
            catalog_before
        );
        for domain in ["a.reload.localhost", "z.reload.localhost"] {
            assert!(
                registry
                    .lock()
                    .await
                    .domain_index()
                    .resolve(domain)
                    .is_some()
            );
            assert!(manager.domain_index().snapshot().resolve(domain).is_some());
        }
        let services = manager.services.lock().await;
        assert!(matches!(
            &services["reload:a"].runtime_state,
            ServiceRuntime::None
        ));
        let ServiceRuntime::Controller(restored_controller) = &services["reload:z"].runtime_state
        else {
            panic!("failing controller must remain installed");
        };
        assert!(Arc::ptr_eq(restored_controller, &failing_controller));
        drop(services);
        assert!(
            host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn removed_service_releases_claim_after_successful_stop() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reload-project");
        std::fs::create_dir(&project_path).expect("create project directory");
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "reload"
domain = "reload.localhost"
"#,
        )
        .expect("write replacement config");
        let canonical_path = std::fs::canonicalize(&project_path).expect("canonical project path");
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project(discovery, Some("reload".to_owned()))
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "reload.localhost".parse().expect("valid domain"),
                    instance_id,
                    "reload:web".to_owned(),
                )],
            )
            .expect("record existing claim");
        catalog.save().await.expect("persist existing catalog");

        let registry = Arc::new(Mutex::new(catalog));
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        manager.services.lock().await.insert(
            "reload:web".to_owned(),
            test_service(
                test_config_with_domain("reload", "reload.localhost"),
                ServiceConfig::Legacy(ExecServiceConfig::default()),
                ServiceRuntime::Controller(Arc::new(Mutex::new(TestController::new(
                    "reload:web",
                    RuntimeState {
                        pid: Some(42),
                        port: Some(3000),
                        status: ServiceState::Running,
                        health_status: HealthStatus::Healthy,
                    },
                )))),
                canonical_path,
            ),
        );
        let host_sync_calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: host_sync_calls.clone(),
        }));

        manager
            .apply_config(project_path, None, false)
            .await
            .expect("publish removal after stop");

        assert!(manager.services.lock().await.get("reload:web").is_none());
        assert!(
            registry
                .lock()
                .await
                .domain_index()
                .resolve("reload.localhost")
                .is_none()
        );
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("reload.localhost")
                .is_none()
        );
        assert_eq!(
            host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[Vec::<String>::new()]
        );
    }

    #[tokio::test]
    async fn removing_project_synchronizes_released_domain_claims() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("removed-project");
        std::fs::create_dir(&project_path).expect("create project");
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project(discovery, Some("removed".to_owned()))
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "removed.localhost".parse().expect("valid domain"),
                    instance_id,
                    "removed:web".to_owned(),
                )],
            )
            .expect("record claim");
        catalog.save().await.expect("persist catalog");

        let registry = Arc::new(Mutex::new(catalog));
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        let calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: calls.clone(),
        }));

        manager
            .remove_project(&project_path)
            .await
            .expect("remove project");

        assert_eq!(
            calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[Vec::<String>::new()]
        );
        assert!(registry.lock().await.get_project(&project_path).is_none());
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("removed.localhost")
                .is_none()
        );
    }

    #[tokio::test]
    async fn removing_project_preserves_claim_and_controller_when_stop_fails() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("busy-project");
        std::fs::create_dir(&project_path).expect("create project");
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project(discovery, Some("busy".to_owned()))
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "busy.localhost".parse().expect("valid domain"),
                    instance_id,
                    "busy:web".to_owned(),
                )],
            )
            .expect("record claim");
        catalog.save().await.expect("persist catalog");
        let canonical_path = std::fs::canonicalize(&project_path).expect("canonical project path");

        let registry = Arc::new(Mutex::new(catalog));
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        let controller = Arc::new(Mutex::new(TestController::failing_stop(
            "busy:web",
            RuntimeState {
                pid: Some(43),
                port: Some(3001),
                status: ServiceState::Running,
                health_status: HealthStatus::Healthy,
            },
        )));
        manager.services.lock().await.insert(
            "busy:web".to_owned(),
            test_service(
                test_config_with_domain("busy", "busy.localhost"),
                ServiceConfig::Legacy(ExecServiceConfig::default()),
                ServiceRuntime::Controller(controller),
                canonical_path,
            ),
        );

        let error = manager
            .remove_project(&project_path)
            .await
            .expect_err("stop failure must block removal");

        assert!(format!("{error:#}").contains("injected stop failure"));
        assert!(registry.lock().await.instances.contains_key(&instance_id));
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("busy.localhost")
                .is_some()
        );
        assert!(manager.get_service_controller("busy:web").await.is_some());
    }

    #[tokio::test]
    async fn conflicting_project_claim_fails_before_service_mutation() {
        let dir = tempdir().expect("create temporary directory");
        let first_path = dir.path().join("first-project");
        let second_path = dir.path().join("second-project");
        std::fs::create_dir(&first_path).expect("create first project");
        std::fs::create_dir(&second_path).expect("create second project");
        let project_config = |name: &str| {
            format!(
                r#"
[project]
name = "{name}"
domain = "shared.localhost"

[services.web]
type = "worker"
command = "sleep 30"
"#,
            )
        };
        std::fs::write(first_path.join("locald.toml"), project_config("first"))
            .expect("write first config");
        std::fs::write(second_path.join("locald.toml"), project_config("second"))
            .expect("write second config");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::with_path(
            dir.path().join("catalog.json"),
        )));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            state_manager,
            registry.clone(),
            attachments,
            None,
        )
        .expect("create process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));

        manager
            .apply_config(first_path.clone(), None, false)
            .await
            .expect("start first project");
        let first_controller = manager
            .get_service_controller("first:web")
            .await
            .expect("first controller");
        let error = manager
            .apply_config(second_path, None, false)
            .await
            .expect_err("conflicting project must fail");

        assert!(error.to_string().contains("first:web"));
        assert!(error.to_string().contains("second:web"));
        assert!(manager.services.lock().await.get("second:web").is_none());
        assert!(Arc::ptr_eq(
            &first_controller,
            &manager
                .get_service_controller("first:web")
                .await
                .expect("first controller remains")
        ));
        assert_eq!(registry.lock().await.instances.len(), 1);
        assert!(matches!(
            manager
                .resolve_service_by_domain("shared.localhost")
                .await
                .expect("first route remains"),
            locald_core::resolver::DomainResolution::Service { ref name, .. }
                if name == "first:web"
        ));

        manager
            .stop_project(&first_path)
            .await
            .expect("stop first project");
    }

    #[tokio::test]
    async fn test_attachment_persistence_failure_preserves_catalog_on_remove() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let project_path = dir.path().join("project");
        std::fs::create_dir(&project_path).expect("create project directory");

        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        catalog
            .register_project(discovery, Some("project".to_owned()))
            .expect("register project");
        let registry = Arc::new(Mutex::new(catalog));

        let attachment_path = dir.path().join("attachments.json");
        std::fs::create_dir(&attachment_path).expect("create blocking attachment directory");
        let mut attachment_store = AttachmentStore::new(attachment_path);
        attachment_store
            .attach(Attachment {
                project_path: project_path.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        let attachments = Arc::new(Mutex::new(attachment_store));

        let mut manager = ProcessManager::new(
            notify_path,
            state_manager,
            registry.clone(),
            attachments.clone(),
            None,
        )
        .expect("create process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));

        manager
            .remove_project(&project_path)
            .await
            .expect_err("attachment persistence must block catalog removal");

        assert!(registry.lock().await.get_project(&project_path).is_some());
        let stored = attachments.lock().await;
        assert_eq!(stored.attachments_for(&project_path).len(), 1);
        assert!(matches!(
            stored.attachments_for(&project_path)[0].source,
            AttachmentSource::Pin
        ));
    }

    #[tokio::test]
    async fn test_catalog_persistence_failure_restores_attachments_on_remove() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let project_path = dir.path().join("project");
        std::fs::create_dir(&project_path).expect("create project directory");

        let catalog_path = dir.path().join("catalog.json");
        std::fs::create_dir(&catalog_path).expect("create blocking catalog directory");
        let mut catalog = Registry::with_path(catalog_path);
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover project");
        catalog
            .register_project(discovery, Some("project".to_owned()))
            .expect("register project");
        let registry = Arc::new(Mutex::new(catalog));

        let attachment_path = dir.path().join("attachments.json");
        let mut attachment_store = AttachmentStore::new(attachment_path.clone());
        attachment_store
            .attach(Attachment {
                project_path: project_path.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        attachment_store.mark_stopped(&project_path);
        attachment_store
            .save()
            .await
            .expect("persist original attachments");
        let attachments = Arc::new(Mutex::new(attachment_store));

        let mut manager = ProcessManager::new(
            notify_path,
            state_manager,
            registry.clone(),
            attachments.clone(),
            None,
        )
        .expect("create process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));

        manager
            .remove_project(&project_path)
            .await
            .expect_err("catalog persistence must restore attachment state");

        assert!(registry.lock().await.get_project(&project_path).is_some());
        let stored = attachments.lock().await;
        assert_eq!(stored.attachments_for(&project_path).len(), 1);
        assert!(matches!(
            stored.attachments_for(&project_path)[0].source,
            AttachmentSource::Pin
        ));
        assert!(stored.is_stopped(&project_path));
        drop(stored);

        let mut reloaded = AttachmentStore::new(attachment_path);
        reloaded.load().await.expect("reload restored attachments");
        assert_eq!(reloaded.attachments_for(&project_path).len(), 1);
        assert!(matches!(
            reloaded.attachments_for(&project_path)[0].source,
            AttachmentSource::Pin
        ));
        assert!(reloaded.is_stopped(&project_path));
    }

    #[tokio::test]
    async fn test_project_attach_reaps_stale_editor_before_first_attach() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::with_path(
            dir.path().join("catalog.json"),
        )));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let mut manager = ProcessManager::new(
            notify_path,
            state_manager,
            registry,
            attachments.clone(),
            None,
        )
        .expect("Failed to create ProcessManager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));

        let project_path = dir.path().join("project");
        std::fs::create_dir(&project_path).unwrap();
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "attached"

[services.web]
command = "sleep 30"
"#,
        )
        .unwrap();

        {
            let mut store = attachments.lock().await;
            store
                .attach(Attachment {
                    project_path: project_path.clone(),
                    source: AttachmentSource::Editor {
                        name: "vscode".to_string(),
                        id: "stale".to_string(),
                        pid: None,
                    },
                    created_at: SystemTime::now() - std::time::Duration::from_secs(31 * 60),
                })
                .expect("attach stale editor");
        }

        manager
            .project_attach(
                project_path.clone(),
                AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "fresh".to_string(),
                    pid: Some(std::process::id()),
                },
            )
            .await
            .unwrap();

        let services = manager.services.lock().await;
        assert!(services.contains_key("attached:web"));
        drop(services);

        let store = attachments.lock().await;
        let remaining = store.attachments_for(&project_path);
        assert_eq!(remaining.len(), 1);
        assert!(matches!(
            remaining[0].source,
            AttachmentSource::Editor { ref id, .. } if id == "fresh"
        ));
        drop(store);

        manager.stop_project(&project_path).await.unwrap();
    }

    #[tokio::test]
    async fn test_concurrency_guard() {
        let guard = ConcurrencyGuard::new();
        let run_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // We use a notify to control the execution flow of the "inner" task
        let notify = Arc::new(tokio::sync::Notify::new());

        let run_count_clone = run_count.clone();
        let notify_clone = notify.clone();

        // The task function
        let task = move || {
            let run_count = run_count_clone.clone();
            let notify = notify_clone.clone();
            async move {
                run_count.fetch_add(1, Ordering::SeqCst);
                // Wait for notification to proceed
                notify.notified().await;
                Ok(())
            }
        };

        // 1. Start the first run
        let guard_clone = guard.clone();
        let task_clone = task.clone();
        let handle = tokio::spawn(async move { guard_clone.run(task_clone).await });

        // Wait for it to start (spin wait is ugly but simple for this)
        while run_count.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(run_count.load(Ordering::SeqCst), 1);

        // 2. Trigger a second run (should be queued)
        let guard_clone2 = guard.clone();
        let task_clone2 = task.clone();
        let second_handle = tokio::spawn(async move { guard_clone2.run(task_clone2).await });

        // Give it a moment to queue
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        // Should still be 1 because first run is blocked on notify
        assert_eq!(run_count.load(Ordering::SeqCst), 1);

        // 3. Unblock the first run
        notify.notify_one();

        // Wait for second run to start
        while run_count.load(Ordering::SeqCst) == 1 {
            tokio::task::yield_now().await;
        }
        assert_eq!(run_count.load(Ordering::SeqCst), 2);

        // 4. Unblock the second run
        notify.notify_one();

        // Wait for handle to finish
        handle.await.unwrap().unwrap();
        second_handle.await.unwrap().unwrap();
    }

    fn test_config_with_domain(name: &str, domain: &str) -> LocaldConfig {
        LocaldConfig {
            project: ProjectConfig {
                name: name.to_string(),
                domain: Some(domain.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn test_service(
        config: LocaldConfig,
        service_config: ServiceConfig,
        runtime_state: ServiceRuntime,
        path: PathBuf,
    ) -> Service {
        Service {
            config,
            service_config,
            resolved_env: HashMap::new(),
            runtime_state,
            sticky_port: None,
            path,
            health_status: HealthStatus::Healthy,
            health_source: HealthSource::None,
            warnings: Vec::new(),
        }
    }

    #[tokio::test]
    async fn exact_index_drives_status_inspection_routing_and_hosts() {
        let dir = tempdir().expect("create temporary directory");
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let mut manager =
            ProcessManager::new(notify_path, state_manager, registry, attachments, None)
                .expect("create process manager");
        let calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: calls.clone(),
        }));

        manager.services.lock().await.insert(
            "app:web".to_owned(),
            test_service(
                test_config_with_domain("app", "stale.localhost"),
                ServiceConfig::Legacy(ExecServiceConfig::default()),
                ServiceRuntime::None,
                dir.path().join("app"),
            ),
        );
        install_test_claim(&manager, "current.localhost", "app:web");

        let statuses = manager.list().await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].domain.as_deref(), Some("current.localhost"));
        let inspection = manager.inspect("app:web").await.expect("inspect service");
        assert_eq!(inspection["domain"], "current.localhost");
        let resolution = manager
            .resolve_service_by_domain("CURRENT.LOCALHOST.")
            .await
            .expect("resolve normalized claim");
        assert!(matches!(
            resolution,
            locald_core::resolver::DomainResolution::Service {
                ref name,
                status: ServiceState::Stopped,
                ..
            } if name == "app:web"
        ));

        manager.sync_hosts().await.expect("synchronize hosts");
        assert_eq!(
            calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[vec!["current.localhost".to_owned()]]
        );
    }

    #[tokio::test]
    async fn test_resolve_service_by_domain_uses_the_exact_index_owner() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("Failed to create ProcessManager");

        let service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        let domain = "shared.localhost";

        let stopped = test_service(
            test_config_with_domain("stale", domain),
            service_config.clone(),
            ServiceRuntime::None,
            dir.path().join("stale"),
        );

        let running_state = RuntimeState {
            pid: Some(123),
            port: Some(3000),
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        let running_controller =
            Arc::new(Mutex::new(TestController::new("active:web", running_state)));
        let running = test_service(
            test_config_with_domain("active", domain),
            service_config.clone(),
            ServiceRuntime::Controller(running_controller),
            dir.path().join("active"),
        );

        {
            let mut services = manager.services.lock().await;
            services.insert("stale:web".to_string(), stopped);
            services.insert("active:web".to_string(), running);
        }
        install_test_claim(&manager, domain, "active:web");

        let resolution = manager
            .resolve_service_by_domain(domain)
            .await
            .expect("Expected resolution");

        assert!(matches!(
            resolution,
            locald_core::resolver::DomainResolution::Service {
                ref name,
                port: Some(3000),
                status: ServiceState::Running,
            } if name == "active:web"
        ));
    }

    #[tokio::test]
    async fn test_resolve_service_by_domain_uses_only_a_running_owner_port() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("Failed to create ProcessManager");

        let service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        let domain = "shared.localhost";

        let stopped_state = RuntimeState {
            pid: None,
            port: Some(3000),
            status: ServiceState::Stopped,
            health_status: HealthStatus::Unknown,
        };
        let stopped_controller =
            Arc::new(Mutex::new(TestController::new("stale:web", stopped_state)));
        let stopped = test_service(
            test_config_with_domain("stale", domain),
            service_config.clone(),
            ServiceRuntime::Controller(stopped_controller),
            dir.path().join("stale"),
        );

        let running_state = RuntimeState {
            pid: Some(123),
            port: Some(4000),
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        let running_controller =
            Arc::new(Mutex::new(TestController::new("active:web", running_state)));
        let running = test_service(
            test_config_with_domain("active", domain),
            service_config.clone(),
            ServiceRuntime::Controller(running_controller),
            dir.path().join("active"),
        );

        {
            let mut services = manager.services.lock().await;
            services.insert("stale:web".to_string(), stopped);
            services.insert("active:web".to_string(), running);
        }
        install_test_claim(&manager, domain, "active:web");

        let resolution = manager
            .resolve_service_by_domain(domain)
            .await
            .expect("Expected resolution");

        assert!(matches!(
            resolution,
            locald_core::resolver::DomainResolution::Service {
                ref name,
                port: Some(4000),
                status: ServiceState::Running,
            } if name == "active:web"
        ));
    }

    #[tokio::test]
    async fn test_resolve_service_by_domain_preserves_building_state() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("Failed to create ProcessManager");

        let service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        let domain = "shared.localhost";

        let stopped = test_service(
            test_config_with_domain("stale", domain),
            service_config.clone(),
            ServiceRuntime::None,
            dir.path().join("stale"),
        );

        let building_state = RuntimeState {
            pid: None,
            port: None,
            status: ServiceState::Building,
            health_status: HealthStatus::Starting,
        };
        let building_controller = Arc::new(Mutex::new(TestController::new(
            "builder:web",
            building_state,
        )));
        let building = test_service(
            test_config_with_domain("builder", domain),
            service_config.clone(),
            ServiceRuntime::Controller(building_controller),
            dir.path().join("builder"),
        );

        {
            let mut services = manager.services.lock().await;
            services.insert("stale:web".to_string(), stopped);
            services.insert("builder:web".to_string(), building);
        }
        install_test_claim(&manager, domain, "builder:web");

        let resolution = manager
            .resolve_service_by_domain(domain)
            .await
            .expect("Expected resolution");

        assert!(matches!(
            resolution,
            locald_core::resolver::DomainResolution::Service {
                ref name,
                port: None,
                status: ServiceState::Building,
            } if name == "builder:web"
        ));
    }

    #[tokio::test]
    async fn test_resolve_service_by_domain_returns_stopped() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("Failed to create ProcessManager");

        let service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        let domain = "shared.localhost";

        let stopped = test_service(
            test_config_with_domain("stale", domain),
            service_config,
            ServiceRuntime::None,
            dir.path().join("stale"),
        );

        {
            let mut services = manager.services.lock().await;
            services.insert("stale:web".to_string(), stopped);
        }
        install_test_claim(&manager, domain, "stale:web");

        let resolution = manager
            .resolve_service_by_domain(domain)
            .await
            .expect("Expected resolution");

        assert!(matches!(
            resolution,
            locald_core::resolver::DomainResolution::Service {
                ref name,
                port: None,
                status: ServiceState::Stopped,
            } if name == "stale:web"
        ));
    }

    #[tokio::test]
    async fn test_resolve_service_by_domain_preserves_legacy_ownership() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("create process manager");
        install_legacy_test_claim(&manager, "legacy.localhost");

        let resolution = manager
            .resolve_service_by_domain("LEGACY.LOCALHOST.")
            .await
            .expect("resolve persisted legacy ownership");

        assert!(matches!(
            resolution,
            locald_core::resolver::DomainResolution::OwnershipOnly
        ));
    }

    #[tokio::test]
    async fn test_resolve_service_by_domain_requires_loaded_service_context() {
        let dir = tempdir().unwrap();
        let notify_path = dir.path().join("notify.sock");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));

        let manager = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
            .expect("create process manager");
        install_test_claim(&manager, "restored.localhost", "restored:web");

        let resolution = manager
            .resolve_service_by_domain("restored.localhost")
            .await
            .expect("persisted claim remains owned");

        assert!(matches!(
            resolution,
            locald_core::resolver::DomainResolution::OwnershipOnly
        ));
    }
}
