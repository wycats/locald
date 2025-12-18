#![allow(clippy::collapsible_if)]
#![allow(clippy::option_if_let_else)]
use crate::config_loader::ConfigLoader;
use crate::health::HealthMonitor;
use crate::runtime::Runtime;
use crate::state::StateManager;
use anyhow::{Context, Result};
use bollard::Docker;
use futures_util::StreamExt;
use locald_core::config::{LocaldConfig, ServiceConfig, TypedServiceConfig};
use locald_core::ipc::{BootEvent, Event, LogEntry, ServiceStatus};
use locald_core::registry::Registry;
use locald_core::resolver::ServiceResolver;
use locald_core::service::{ServiceContext, ServiceController, ServiceFactory};
use locald_core::state::{
    HealthSource, HealthStatus, PersistedServiceState, ServerState, ServiceState,
};
use nix::sys::signal::Signal;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
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
                warn!(
                    "Skipping hosts auto-sync: locald-shim is not installed or not setuid root. Run sudo locald admin setup to configure it."
                );
                return Ok(());
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

struct RunGuard(Arc<AtomicBool>);
impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug)]
struct ConcurrencyGuard {
    running: Arc<AtomicBool>,
    pending: Arc<AtomicBool>,
}

impl ConcurrencyGuard {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn run<F, Fut>(&self, f: F) -> Result<()>
    where
        F: Fn() -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        // Try to acquire the running lock
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Already running, mark as pending
            self.pending.store(true, Ordering::SeqCst);
            info!("Host sync already in progress, queued for next run");
            return Ok(());
        }

        // We are the runner.
        // We need to ensure we clear the running flag when we are done.
        let _guard = RunGuard(self.running.clone());

        loop {
            // Clear pending flag *before* running.
            // If a new request comes in *while* we are running, it will set pending=true again.
            self.pending.store(false, Ordering::SeqCst);

            // Run the task
            if let Err(e) = f().await {
                error!("Host sync failed: {}", e);
            }

            // Check if we need to run again
            if !self.pending.load(Ordering::SeqCst) {
                break;
            }
            info!("Pending host sync detected, re-running...");
            // Throttle to prevent busy loops
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        Ok(())
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
    pub runtime_state: ServiceRuntime,
    pub sticky_port: Option<u16>,
    pub path: PathBuf,
    pub health_status: HealthStatus,
    pub health_source: HealthSource,
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
    health_monitor: HealthMonitor,
    factories: Vec<Arc<dyn ServiceFactory>>,
    hosts_sync_guard: ConcurrencyGuard,
    host_syncer: Arc<dyn HostSyncer>,
}

impl ProcessManager {
    /// Create a new `ProcessManager`.
    ///
    /// # Arguments
    ///
    /// * `notify_socket_path` - Path to the Unix socket for `sd_notify` messages.
    /// * `docker` - Docker client instance.
    /// * `state_manager` - State persistence manager.
    /// * `registry` - Project registry.
    pub fn new(
        notify_socket_path: PathBuf,
        docker: Option<Arc<Docker>>,
        state_manager: Arc<StateManager>,
        registry: Arc<Mutex<Registry>>,
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

        let health_monitor = HealthMonitor::new(
            docker.clone(),
            services.clone(),
            event_tx.clone(),
            proxy_ports.clone(),
        );

        let runtime = Arc::new(Runtime::new(docker.clone(), notify_socket_path));

        let factories: Vec<Arc<dyn ServiceFactory>> = vec![
            Arc::new(crate::service::postgres::PostgresFactory),
            Arc::new(crate::service::site::SiteFactory),
            Arc::new(crate::service::exec::ExecFactory::new(
                runtime.process.clone(),
            )),
            #[allow(deprecated)]
            Arc::new(crate::service::docker::DockerFactory::new(
                crate::runtime::docker::DockerRuntime::new(docker),
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
            health_monitor,
            factories,
            hosts_sync_guard: ConcurrencyGuard::new(),
            host_syncer: Arc::new(DefaultHostSyncer),
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
    ) -> ServiceStatus {
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

        ServiceStatus {
            name: name.clone(),
            pid,
            port,
            status,
            url,
            health_status,
            health_source,
            path,
            domain,
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

    pub(crate) fn get_service_domain(
        name: &str,
        project_config: &locald_core::config::ProjectConfig,
    ) -> String {
        let domain = project_config
            .domain
            .clone()
            .unwrap_or_else(|| format!("{}.localhost", project_config.name));

        let short_name = name.split(':').nth(1).unwrap_or(name);

        // If the service is named "web" or matches the project name, map it to the root domain.
        if short_name == "web" || short_name == project_config.name {
            domain
        } else {
            format!("{}.{}", short_name, domain)
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn get_service_status(&self, name: &str) -> Option<ServiceStatus> {
        let proxy_ports = { *self.proxy_ports.lock().await };
        let (domain, path, health_status, health_source, snapshot, service_config) = {
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
                Some(Self::get_service_domain(name, &service.config.project)),
                Some(service.path.clone()),
                service.health_status,
                service.health_source,
                snapshot,
                service.service_config.clone(),
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
                if let Err(e) = self.runtime.docker.stop_container(container_id).await {
                    warn!("Cleanup warning (stop_container): {:#}", e);
                }
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
                let short_name = name.split(':').nth(1).unwrap_or(name);
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
                    let domains = {
                        let services = manager.services.lock().await;
                        let mut domains = HashSet::new();
                        for (name, service) in services.iter() {
                            domains.insert(Self::get_service_domain(name, &service.config.project));
                        }
                        let mut list: Vec<String> = domains.into_iter().collect();
                        list.sort();
                        list
                    };
                    syncer.sync(domains).await
                }
            })
            .await
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

        let manager = self.clone();
        let path_clone = path.clone();
        let handle = tokio::runtime::Handle::current();

        let watcher_res =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        if event.kind.is_modify() || event.kind.is_create() {
                            let relevant = event
                                .paths
                                .iter()
                                .any(|p| p.ends_with("locald.toml") || p.ends_with("Procfile"));

                            if relevant {
                                info!("Config changed: {:?}", event.paths);
                                let manager = manager.clone();
                                let path = path_clone.clone();
                                handle.spawn(async move {
                                    // Debounce?
                                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                    if let Err(e) = manager.apply_config(path, None, false).await {
                                        error!("Failed to reload config: {e}");
                                    }
                                });
                            }
                        }
                    }
                    Err(e) => error!("Watch error: {e}"),
                }
            });

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

        let (config, dot_env_vars) = ConfigLoader::load_project_config(&path).await?;

        if let Some(tx) = &event_tx {
            let _ = tx
                .send(BootEvent::StepFinished {
                    id: "config".to_string(),
                    result: Ok(()),
                })
                .await;
        }

        // Update registry
        {
            let mut registry = self.registry.lock().await;
            registry.register_project(&path, Some(config.project.name.clone()));
            if let Err(e) = registry.save().await {
                error!("Failed to save registry: {}", e);
            }
        }

        // Auto-sync hosts if needed
        // We do this in a background task to avoid blocking startup
        // and to handle the fact that we might not have permissions (though shim should handle it)
        let manager = self.clone();
        tokio::spawn(async move {
            if let Err(e) = manager.sync_hosts().await {
                warn!("Failed to auto-sync hosts: {}", e);
            }
        });

        let sorted_services = ConfigLoader::resolve_startup_order(&config)?;
        let mut active_services = HashSet::new();

        for service_name in sorted_services {
            let service_config = &config.services[&service_name];
            info!(
                "Service {}:{} config: {:?}",
                config.project.name, service_name, service_config
            );
            let name = format!("{}:{}", config.project.name, service_name);
            active_services.insert(name.clone());

            // Check if already running and config matches
            {
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
                        if &service.service_config == service_config {
                            info!("Service {name} is already running and up to date");
                            if let Some(tx) = &event_tx {
                                let _ = tx
                                    .send(BootEvent::StepStarted {
                                        id: name.clone(),
                                        description: format!("Service {} up to date", name),
                                    })
                                    .await;
                                let _ = tx
                                    .send(BootEvent::StepFinished {
                                        id: name.clone(),
                                        result: Ok(()),
                                    })
                                    .await;
                            }
                            continue;
                        }
                        info!("Service {name} config changed, restarting...");
                    }
                }
            } // Drop lock before stopping/starting

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

            let port = if !needs_port {
                None
            } else if let Some(p) = service_config.port() {
                Some(p)
            } else {
                // Check for sticky port
                let sticky = {
                    let services = self.services.lock().await;
                    services.get(&name).and_then(|s| s.sticky_port)
                };

                if let Some(p) = sticky {
                    // Try to bind to sticky port to ensure it's free
                    if std::net::TcpListener::bind(format!("127.0.0.1:{p}")).is_ok() {
                        info!("Reusing sticky port {p} for service {name}");
                        Some(p)
                    } else {
                        warn!("Sticky port {p} for service {name} is taken, assigning new port");
                        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
                        Some(listener.local_addr()?.port())
                    }
                } else {
                    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
                    Some(listener.local_addr()?.port())
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

            let mut combined_env = dot_env_vars.clone();
            for (k, v) in service_config.env() {
                combined_env.insert(k.clone(), v.clone());
            }

            let manager = self.clone();
            let lookup = move |service_name: String, field: String| {
                let manager = manager.clone();
                async move { manager.get_service_field(&service_name, &field).await }
            };

            let resolved_env = ConfigLoader::resolve_env(&combined_env, &config, lookup).await?;

            let mut handled = false;
            for factory in &self.factories {
                if factory.can_handle(service_config) {
                    info!("Using factory for service {}", name);
                    let ctx = ServiceContext {
                        project_root: path.clone(),
                        port,
                        env: resolved_env.clone(),
                    };
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
                                runtime_state: ServiceRuntime::Controller(controller.clone()),
                                sticky_port: port,
                                path: path.clone(),
                                health_status: HealthStatus::Unknown,
                                health_source: HealthSource::None,
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
                        None,
                        false,
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

        // Stop removed services
        let to_stop = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter(|(n, s)| s.path == path && !active_services.contains(n.as_str()))
                .map(|(n, _)| n.clone())
                .collect::<Vec<_>>()
        };

        for name in to_stop {
            info!("Service {name} removed from config, stopping...");
            self.stop(&name).await?;
        }

        self.persist_state().await;
        Ok(())
    }

    /// Stops a specific service by name.
    ///
    /// This method:
    /// 1. Identifies the runtime type (Process, Docker, Postgres).
    /// 2. Sends the configured stop signal (default: SIGTERM).
    /// 3. Cleans up associated resources (containers, PTYs).
    /// 4. Persists the new state.
    ///
    /// # Errors
    ///
    /// Returns an error if the service state cannot be persisted, though
    /// cleanup errors are generally logged as warnings rather than returned.
    pub async fn stop(&self, name: &str) -> Result<()> {
        let mut runtime_state = ServiceRuntime::None;

        {
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(name) {
                runtime_state = std::mem::replace(&mut service.runtime_state, ServiceRuntime::None);
                // Note: We do NOT clear sticky_port here, so we can reuse it on restart.
                service.health_status = HealthStatus::Unknown;
            }
        }

        match runtime_state {
            ServiceRuntime::Controller(c) => {
                if let Err(e) = c.lock().await.stop().await {
                    warn!("Failed to stop service {name}: {e}");
                }
            }
            ServiceRuntime::None => {}
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
            services.get(name).map(|service| {
                let short_name = name.split(':').nth(1).unwrap_or(name);
                // TODO: This path logic is duplicated. Should be centralized.
                // For now, we only support resetting Postgres services which use this path.
                // If we add other stateful services, we need a better way to know their data dir.
                // We could store `data_dir` in the Service struct?
                // But `Service` struct has `path` which is project root.
                // Let's assume Postgres for now.
                service
                    .path
                    .join(".locald/services/postgres")
                    .join(short_name)
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
                    Some(Self::get_service_domain(name, &service.config.project)),
                    Some(service.path.clone()),
                    service.health_status,
                    service.health_source,
                    snapshot,
                    service.service_config.clone(),
                ));
            }
        }

        let mut results = Vec::new();
        for (name, domain, path, health_status, health_source, snapshot, service_config) in
            snapshots
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

    pub async fn resolve_service_by_domain(&self, domain: &str) -> Option<(String, u16)> {
        let (name, controller_to_check) = {
            let services = self.services.lock().await;
            let found = services.iter().find_map(|(name, service)| {
                let d = Self::get_service_domain(name, &service.config.project);
                if d == domain {
                    match &service.runtime_state {
                        ServiceRuntime::Controller(c) => {
                            return Some((name.clone(), Err(c.clone())));
                        }
                        ServiceRuntime::None => return Some((name.clone(), Ok(None))),
                    }
                }
                None
            });
            match found {
                Some(x) => x,
                None => return None,
            }
        };

        let port = match controller_to_check {
            Ok(port) => port,
            Err(c) => c.lock().await.read_state().await.port,
        };

        port.map(|p| (name, p))
    }

    pub async fn registry_list(&self) -> Vec<locald_core::registry::ProjectEntry> {
        let registry = self.registry.lock().await;
        registry.projects.values().cloned().collect()
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_pin(&self, path: &std::path::Path) -> Result<()> {
        let mut registry = self.registry.lock().await;
        if registry.pin_project(path) {
            registry.save().await?;
            Ok(())
        } else {
            anyhow::bail!("Project not found in registry")
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_unpin(&self, path: &std::path::Path) -> Result<()> {
        let mut registry = self.registry.lock().await;
        if registry.unpin_project(path) {
            registry.save().await?;
            Ok(())
        } else {
            anyhow::bail!("Project not found in registry")
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_clean(&self) -> Result<usize> {
        let mut registry = self.registry.lock().await;

        // Identify missing projects
        let mut to_remove = Vec::new();
        for (path, entry) in &registry.projects {
            // If pinned, we might want to keep it even if missing?
            // But current implementation of prune_missing_projects didn't check pinned.
            // Let's respect pinned if the user intended it to prevent cleanup.
            // However, if the directory is gone, it's broken.
            // Let's stick to "if it's gone, it's gone" for now to match existing behavior,
            // unless we want to fix the "pinned" behavior too.
            // Given the user asked about "pruning", let's assume they want to clean up garbage.
            if !path.exists() && !entry.pinned {
                to_remove.push(path.clone());
            }
        }

        let count = to_remove.len();

        for path in to_remove {
            // Remove from registry
            registry.projects.remove(&path);

            // Clean up global state directory
            let state_dir = locald_utils::project::get_state_dir(&path);
            // The state_dir is .../projects/<name>-<hash>/.locald
            // We want to remove the parent directory .../projects/<name>-<hash>
            if let Some(project_container) = state_dir.parent() {
                if project_container.exists() {
                    info!("Removing orphaned state directory: {:?}", project_container);
                    if let Err(e) = tokio::fs::remove_dir_all(project_container).await {
                        warn!(
                            "Failed to remove orphaned state directory {:?}: {}",
                            project_container, e
                        );
                    }
                }
            }
        }

        if count > 0 {
            registry.save().await?;
        }
        Ok(count)
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
        let (service_config, path, health_status, health_source, runtime_info, domain) = {
            let services = self.services.lock().await;
            let service = services
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Service not found"))?;

            let short_name = name.split(':').nth(1).unwrap_or(name);
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
                Some(Self::get_service_domain(name, &service.config.project)),
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
            "url": url,
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
    async fn resolve_service_by_domain(&self, domain: &str) -> Option<(String, u16)> {
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
    use locald_core::config::ProjectConfig;

    #[test]
    fn test_get_service_domain_default() {
        let project_config = ProjectConfig {
            name: "myproject".to_string(),
            domain: None,
            ..Default::default()
        };

        let domain = ProcessManager::get_service_domain("myproject:web", &project_config);
        // "web" service should map to the root domain
        assert_eq!(domain, "myproject.localhost".to_string());
    }

    #[test]
    fn test_get_service_domain_explicit() {
        let project_config = ProjectConfig {
            name: "myproject".to_string(),
            domain: Some("example.com".to_string()),
            ..Default::default()
        };

        let domain = ProcessManager::get_service_domain("myproject:api", &project_config);
        assert_eq!(domain, "api.example.com".to_string());
    }

    #[test]
    fn test_get_service_domain_main_service() {
        let project_config = ProjectConfig {
            name: "shop".to_string(),
            domain: Some("shop.local".to_string()),
            ..Default::default()
        };

        // "web" service -> root domain
        let domain = ProcessManager::get_service_domain("shop:web", &project_config);
        assert_eq!(domain, "shop.local".to_string());

        // Service matching project name -> root domain
        let domain = ProcessManager::get_service_domain("shop:shop", &project_config);
        assert_eq!(domain, "shop.local".to_string());

        // Other service -> subdomain
        let domain = ProcessManager::get_service_domain("shop:api", &project_config);
        assert_eq!(domain, "api.shop.local".to_string());
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
        )
        .await;
        assert_eq!(status.url, Some("https://app.test:8443".to_string()));
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
        tokio::spawn(async move { guard_clone2.run(task_clone2).await });

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
    }
}
