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
    HealthSource, HealthStatus, PersistedProcessIdentity, PersistedServiceState, ServerState,
    ServiceState,
};
use locald_core::{
    AvailabilityError, AvailabilityStore, CatalogError, CatalogPresence, ConvergenceDecision,
    DemandKey, DomainClaim, DomainName, DomainTarget, EnsureDemandResult, ProjectInstanceId,
    SharedDomainIndex, availability_path, sanitize_project_name_for_dns,
    sanitize_service_name_for_dns,
};
use nix::sys::signal::Signal;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::SystemTime;
use tokio::sync::{Mutex, OwnedMutexGuard, broadcast};
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

#[derive(Debug)]
struct InstanceLogBuffer {
    instance_id: ProjectInstanceId,
    logs: LogBuffer,
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("Service not found")]
pub struct ServiceNotFoundError;

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("project instance {instance_id} startup was superseded by {decision:?}")]
struct AvailabilityStartSuperseded {
    instance_id: ProjectInstanceId,
    decision: ConvergenceDecision,
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("locald is shutting down")]
struct DaemonShuttingDown;

#[derive(Clone, Debug, thiserror::Error)]
#[error(
    "service `{name}` belongs to project instance {instance_id}, whose legacy runtime restore is pending; wait for restoration and retry"
)]
struct ServiceRestorePending {
    name: String,
    instance_id: ProjectInstanceId,
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
    pub instance_id: ProjectInstanceId,
    pub controller_generation: u64,
    pub projection_generation: u64,
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

#[derive(Debug)]
struct ConfigTransitionPlan {
    removed_service_names: Vec<String>,
    restart_service_names: Vec<String>,
    reusable_service_envs: HashMap<String, HashMap<String, String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ServiceProjectionToken {
    instance_id: ProjectInstanceId,
    controller_generation: u64,
    projection_generation: u64,
    has_controller: bool,
}

#[derive(Debug)]
struct AvailabilityCoordinator {
    runtime: Mutex<()>,
}

impl AvailabilityCoordinator {
    fn new() -> Self {
        Self {
            runtime: Mutex::new(()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AvailabilityConvergenceOptions {
    verbose: bool,
    apply_config_when_up: bool,
    defer_config_reload_during_cooldown: bool,
}

#[derive(Debug, Default)]
pub(crate) struct RuntimeRestorePlan {
    legacy_instances: HashSet<ProjectInstanceId>,
}

impl ServiceProjectionToken {
    fn matches(self, service: &Service) -> bool {
        self.instance_id == service.instance_id
            && self.controller_generation == service.controller_generation
            && self.projection_generation == service.projection_generation
            && self.has_controller == matches!(service.runtime_state, ServiceRuntime::Controller(_))
    }
}

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
    log_buffers: Arc<StdMutex<HashMap<String, InstanceLogBuffer>>>,
    state_manager: Arc<StateManager>,
    runtime: Arc<Runtime>,
    proxy_ports: Arc<Mutex<(Option<u16>, Option<u16>)>>, // (http, https)
    watchers: Arc<Mutex<HashMap<PathBuf, RecommendedWatcher>>>,
    registry: Arc<Mutex<Registry>>,
    domain_index: SharedDomainIndex,
    attachments: Arc<Mutex<AttachmentStore>>,
    attachment_transition_lock: Arc<Mutex<()>>,
    health_monitor: HealthMonitor,
    factories: Vec<Arc<dyn ServiceFactory>>,
    hosts_sync_guard: ConcurrencyGuard,
    host_syncer: Arc<dyn HostSyncer>,
    port_allocator: PortAllocator,
    config_transition_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    availability_coordinators: Arc<Mutex<HashMap<ProjectInstanceId, Arc<AvailabilityCoordinator>>>>,
    availability_publication_lock: Arc<Mutex<()>>,
    pending_config_reloads: Arc<Mutex<HashSet<ProjectInstanceId>>>,
    forgotten_reload_paths: Arc<Mutex<HashSet<PathBuf>>>,
    legacy_restore_evidence: Arc<Mutex<HashMap<ProjectInstanceId, Vec<PersistedServiceState>>>>,
    availability_data_dir: PathBuf,
    runtime_projection_lock: Arc<Mutex<()>>,
    state_persistence_lock: Arc<Mutex<()>>,
    next_controller_generation: Arc<AtomicU64>,
    shutting_down: Arc<AtomicBool>,
}

impl ProcessManager {
    fn postgres_data_dir(name: &str) -> PathBuf {
        ProjectDirs::from("com", "locald", "locald")
            .map(|d| d.data_dir().join("postgres").join(name))
            .unwrap_or_else(|| PathBuf::from(".locald/postgres").join(name))
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(AtomicOrdering::Acquire)
    }

    fn ensure_accepting_lifecycle_requests(&self) -> Result<()> {
        if self.is_shutting_down() {
            Err(DaemonShuttingDown.into())
        } else {
            Ok(())
        }
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
        Self::new_with_availability_data_dir(
            notify_socket_path,
            state_manager,
            registry,
            attachments,
            external_log_sender,
            locald_core::storage::data_dir(),
        )
    }

    fn new_with_availability_data_dir(
        notify_socket_path: PathBuf,
        state_manager: Arc<StateManager>,
        registry: Arc<Mutex<Registry>>,
        attachments: Arc<Mutex<AttachmentStore>>,
        external_log_sender: Option<broadcast::Sender<LogEntry>>,
        availability_data_dir: PathBuf,
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
            attachment_transition_lock: Arc::new(Mutex::new(())),
            health_monitor,
            factories,
            hosts_sync_guard: ConcurrencyGuard::new(),
            host_syncer: Arc::new(DefaultHostSyncer),
            port_allocator: PortAllocator::new(),
            config_transition_locks: Arc::new(Mutex::new(HashMap::new())),
            availability_coordinators: Arc::new(Mutex::new(HashMap::new())),
            availability_publication_lock: Arc::new(Mutex::new(())),
            pending_config_reloads: Arc::new(Mutex::new(HashSet::new())),
            forgotten_reload_paths: Arc::new(Mutex::new(HashSet::new())),
            legacy_restore_evidence: Arc::new(Mutex::new(HashMap::new())),
            availability_data_dir,
            runtime_projection_lock: Arc::new(Mutex::new(())),
            state_persistence_lock: Arc::new(Mutex::new(())),
            next_controller_generation: Arc::new(AtomicU64::new(1)),
            shutting_down: Arc::new(AtomicBool::new(false)),
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

    /// Return the authoritative exact hostnames that require hosts-file mappings.
    #[must_use]
    pub fn hosts_domains(&self) -> Vec<String> {
        self.domain_index.snapshot().hosts_domains()
    }

    /// Return validated exact names for a privileged hosts-file writer.
    #[must_use]
    pub fn hosts_domain_names(&self) -> Vec<DomainName> {
        self.domain_index.snapshot().hosts_domain_names()
    }

    fn build_domain_claims(
        instance_id: ProjectInstanceId,
        config: &LocaldConfig,
        project_path: &Path,
    ) -> Result<Vec<DomainClaim>> {
        let base_domain = config.project.domain.clone().unwrap_or_else(|| {
            format!(
                "{}.localhost",
                sanitize_project_name_for_dns(&config.project.name)
            )
        });
        let base_domain = resolve_worktree_domain(
            &base_domain,
            &config.project.name,
            config.worktrees.as_ref(),
            project_path,
        );
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
                let label = sanitize_service_name_for_dns(&service_name);
                base_domain
                    .with_prefix(&label)
                    .with_context(|| format!("service `{service_name}` has an invalid domain"))?
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

    fn requires_durable_process_ownership(service_config: &ServiceConfig) -> bool {
        matches!(
            service_config,
            ServiceConfig::Legacy(_)
                | ServiceConfig::Typed(
                    TypedServiceConfig::Exec(_)
                        | TypedServiceConfig::Worker(_)
                        | TypedServiceConfig::Container(_)
                )
        )
    }

    pub(crate) fn advance_service_projection(service: &mut Service) -> u64 {
        service.projection_generation = service.projection_generation.wrapping_add(1);
        service.projection_generation
    }

    async fn prepublication_stop_plan(
        &self,
        instance_id: ProjectInstanceId,
        config: &LocaldConfig,
        dot_env_vars: &HashMap<String, String>,
        sorted_services: &[String],
        desired_service_names: &HashSet<String>,
    ) -> Result<ConfigTransitionPlan> {
        let mut removed_service_names = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter(|(name, service)| {
                    service.instance_id == instance_id
                        && !desired_service_names.contains(name.as_str())
                })
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
        };
        removed_service_names.sort();

        let mut changed_services = HashSet::new();
        let mut restart_service_names = Vec::new();
        let mut reusable_service_envs = HashMap::new();

        for service_name in sorted_services {
            let service_config = &config.services[service_name];
            let dependency_will_change = service_config
                .depends_on()
                .iter()
                .any(|dependency| changed_services.contains(dependency));
            let (combined_env, _) =
                Self::effective_service_env(config, dot_env_vars, service_config);
            let manager = self.clone();
            let expected_instance = instance_id;
            let resolved_env =
                ConfigLoader::resolve_env(&combined_env, config, move |name, field| {
                    let manager = manager.clone();
                    async move {
                        manager
                            .get_service_field(&name, &field, expected_instance)
                            .await
                    }
                })
                .await
                .ok();
            let full_name = format!("{}:{service_name}", config.project.name);
            let service_snapshot = {
                let services = self.services.lock().await;
                services.get(&full_name).map(|service| {
                    let controller = match &service.runtime_state {
                        ServiceRuntime::Controller(controller) => Some(controller.clone()),
                        ServiceRuntime::None => None,
                    };
                    (
                        service.instance_id,
                        service.path.clone(),
                        service.service_config.clone(),
                        service.resolved_env.clone(),
                        controller,
                    )
                })
            };

            let (has_controller, is_up_to_date) = match service_snapshot {
                Some((loaded_instance, loaded_path, _, _, Some(_)))
                    if loaded_instance != instance_id =>
                {
                    anyhow::bail!(
                        "service `{full_name}` is still loaded by project instance {loaded_instance} at {}; stop that project before starting instance {instance_id}",
                        loaded_path.display()
                    );
                }
                Some((_, _, current_config, current_env, Some(controller))) => {
                    let controller = controller.lock().await;
                    let is_running = controller.read_state().await.status
                        == locald_core::state::ServiceState::Running;
                    let has_durable_process_ownership =
                        !Self::requires_durable_process_ownership(service_config)
                            || (controller.owned_process_id().is_some()
                                && controller.process_identity().is_some());
                    let environment_matches = resolved_env
                        .as_ref()
                        .is_some_and(|resolved_env| current_env == *resolved_env);
                    (
                        true,
                        !dependency_will_change
                            && is_running
                            && has_durable_process_ownership
                            && current_config == *service_config
                            && environment_matches,
                    )
                }
                Some((_, _, _, _, None)) | None => (false, false),
            };

            if !is_up_to_date {
                changed_services.insert(service_name.clone());
                if has_controller {
                    restart_service_names.push(full_name);
                }
            } else if let Some(resolved_env) = resolved_env {
                reusable_service_envs.insert(full_name, resolved_env);
            }
        }

        // Dependents are later in startup order, so reverse it for shutdown.
        restart_service_names.reverse();
        Ok(ConfigTransitionPlan {
            removed_service_names,
            restart_service_names,
            reusable_service_envs,
        })
    }

    fn domain_for_service(&self, instance_id: ProjectInstanceId, name: &str) -> Option<String> {
        self.domain_index
            .snapshot()
            .domain_for_service(instance_id, name)
            .map(ToString::to_string)
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn get_service_status(
        &self,
        name: &str,
    ) -> Option<(ServiceStatus, ServiceProjectionToken)> {
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
            projection,
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
                self.domain_for_service(service.instance_id, name),
                Some(service.path.clone()),
                service.health_status,
                service.health_source,
                snapshot,
                service.service_config.clone(),
                service.config.project.workspace.clone(),
                service.config.project.constellation.clone(),
                service.warnings.clone(),
                ServiceProjectionToken {
                    instance_id: service.instance_id,
                    controller_generation: service.controller_generation,
                    projection_generation: service.projection_generation,
                    has_controller: matches!(service.runtime_state, ServiceRuntime::Controller(_)),
                },
            )
        };

        Some((
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
            projection,
        ))
    }

    async fn broadcast_service_update(&self, name: &str) {
        if let Some((status, projection)) = self.get_service_status(name).await {
            let services = self.services.lock().await;
            if services
                .get(name)
                .is_some_and(|service| projection.matches(service))
            {
                let _ = self.event_sender.send(Event::ServiceUpdate(status));
            }
        }
    }

    fn clear_foreign_log_buffer(&self, name: &str, instance_id: ProjectInstanceId) {
        #[allow(clippy::expect_used)]
        self.log_buffers
            .lock()
            .expect("log buffer mutex poisoned")
            .retain(|service_name, buffer| {
                service_name != name || buffer.instance_id == instance_id
            });
    }

    async fn broadcast_log(
        &self,
        instance_id: ProjectInstanceId,
        controller_generation: u64,
        entry: LogEntry,
    ) {
        let services = self.services.lock().await;
        let is_current_controller = services.get(&entry.service).is_some_and(|service| {
            service.instance_id == instance_id
                && service.controller_generation == controller_generation
                && matches!(service.runtime_state, ServiceRuntime::Controller(_))
        });
        if !is_current_controller {
            return;
        }

        info!("Broadcasting log for {}: {}", entry.service, entry.message);
        // Add to buffer
        {
            #[allow(clippy::expect_used)]
            let mut buffers = self.log_buffers.lock().expect("log buffer mutex poisoned");
            let buffer = buffers
                .entry(entry.service.clone())
                .and_modify(|buffer| {
                    if buffer.instance_id != instance_id {
                        *buffer = InstanceLogBuffer {
                            instance_id,
                            logs: LogBuffer::new(LOG_BUFFER_SIZE),
                        };
                    }
                })
                .or_insert_with(|| InstanceLogBuffer {
                    instance_id,
                    logs: LogBuffer::new(LOG_BUFFER_SIZE),
                });
            buffer.logs.push(entry.clone());
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
            all_logs.extend(buffer.logs.get_all());
        }
        all_logs.sort_by_key(|e| e.timestamp);
        all_logs
    }

    async fn persist_state_checked(&self) -> Result<()> {
        let _persistence_guard = self.state_persistence_lock.lock().await;
        let mut services_data = Vec::new();
        {
            let services = self.services.lock().await;
            for (name, service) in services.iter() {
                services_data.push((
                    name.clone(),
                    service.instance_id,
                    service.config.clone(),
                    service.path.clone(),
                    service.health_status,
                    service.health_source,
                    service.runtime_state.clone(),
                ));
            }
        }

        let mut service_states = Vec::new();
        for (name, instance_id, config, path, health_status, health_source, runtime) in
            services_data
        {
            let (pid, process_identity, port, status, container_id) = match runtime {
                ServiceRuntime::Controller(c) => {
                    let guard = c.lock().await;
                    let state = guard.read_state().await;
                    let container_id = guard.get_metadata("container_id");
                    let pid = guard.owned_process_id().or_else(|| {
                        (state.status == ServiceState::Running)
                            .then_some(state.pid)
                            .flatten()
                    });
                    let process_identity = pid.and_then(|_| guard.process_identity());
                    (
                        pid,
                        process_identity,
                        state.port,
                        state.status,
                        container_id,
                    )
                }
                ServiceRuntime::None => (
                    None,
                    None,
                    None,
                    locald_core::state::ServiceState::Stopped,
                    None,
                ),
            };

            service_states.push((
                instance_id,
                PersistedServiceState {
                    name,
                    config,
                    path,
                    pid,
                    process_identity,
                    container_id,
                    port,
                    status,
                    health_status,
                    health_source,
                },
            ));
        }

        // While the compatibility restore bridge is active, every global
        // snapshot retains not-yet-restored legacy Running evidence. This
        // prevents a successful or partial start of one project from erasing
        // the retry intent for another before a.2.3 migrates it.
        let legacy_evidence = self.legacy_restore_evidence.lock().await.clone();
        for (instance_id, evidence) in legacy_evidence {
            for pending in evidence {
                match service_states
                    .iter_mut()
                    .find(|(current_instance, current)| {
                        *current_instance == instance_id && current.name == pending.name
                    }) {
                    Some((_, current))
                        if current.pid.is_some()
                            || current.container_id.is_some()
                            || current.status == ServiceState::Running => {}
                    Some((_, current)) => *current = pending,
                    None => service_states.push((instance_id, pending)),
                }
            }
        }

        let state = ServerState {
            services: service_states
                .into_iter()
                .map(|(_, service)| service)
                .collect(),
        };

        self.state_manager.save(&state).await
    }

    async fn persist_state(&self) {
        if let Err(error) = self.persist_state_checked().await {
            error!("Failed to persist state: {error}");
        }
    }

    /// Reconcile process evidence left by the previous daemon before lifecycle
    /// IPC can launch anything new. Cleanup is bounded across all recorded
    /// processes, and handles are cleared only after the OS confirms that both
    /// the recorded process and process group are gone.
    pub(crate) async fn reconcile_stale_runtime_state(&self) -> Result<RuntimeRestorePlan> {
        let _persistence_guard = self.state_persistence_lock.lock().await;
        let mut state = self.state_manager.load().await?;
        info!(
            "Reconciling stale runtime state: found {} services",
            state.services.len()
        );

        let mut recorded_processes = HashMap::<u32, Option<PersistedProcessIdentity>>::new();
        for service in &state.services {
            let Some(pid) = service.pid else {
                continue;
            };
            recorded_processes
                .entry(pid)
                .and_modify(|identity| {
                    if *identity != service.process_identity {
                        *identity = None;
                    }
                })
                .or_insert_with(|| service.process_identity.clone());
        }
        let mut pending = HashMap::new();
        let mut confirmed_gone = HashSet::new();
        let mut failures = HashMap::<u32, String>::new();

        for (pid, identity) in recorded_processes {
            let Some(identity) = identity else {
                match self.runtime.process.unverified_stale_process_exists(pid) {
                    Ok(false) => {
                        confirmed_gone.insert(pid);
                    }
                    Ok(true) => {
                        failures.insert(
                            pid,
                            "live process has no verified ownership identity; stop it manually"
                                .to_owned(),
                        );
                    }
                    Err(error) => {
                        failures
                            .insert(pid, format!("unverified liveness check failed: {error:#}"));
                    }
                }
                continue;
            };

            match self.runtime.process.verify_stale_process(pid, &identity) {
                Ok(None) => {
                    confirmed_gone.insert(pid);
                }
                Ok(Some(process)) => {
                    match self
                        .runtime
                        .process
                        .signal_verified_stale_process(&process, Signal::SIGTERM)
                    {
                        Ok(()) => {
                            pending.insert(pid, process);
                        }
                        Err(error) => {
                            failures.insert(pid, format!("verified SIGTERM failed: {error:#}"));
                        }
                    }
                }
                Err(error) => {
                    failures.insert(pid, format!("ownership verification failed: {error:#}"));
                }
            }
        }

        let term_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !pending.is_empty() && tokio::time::Instant::now() < term_deadline {
            let candidates = pending
                .iter()
                .map(|(pid, process)| (*pid, process.clone()))
                .collect::<Vec<_>>();
            for (pid, process) in candidates {
                match self.runtime.process.verified_stale_process_exists(&process) {
                    Ok(false) => {
                        pending.remove(&pid);
                        confirmed_gone.insert(pid);
                    }
                    Ok(true) => {}
                    Err(error) => {
                        pending.remove(&pid);
                        failures.insert(pid, format!("liveness check failed: {error:#}"));
                    }
                }
            }
            if !pending.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }

        let mut kill_failures = Vec::new();
        for (pid, process) in &pending {
            if let Err(error) = self
                .runtime
                .process
                .signal_verified_stale_process(process, Signal::SIGKILL)
            {
                kill_failures.push((*pid, error));
            }
        }
        for (pid, error) in kill_failures {
            pending.remove(&pid);
            failures.insert(pid, format!("SIGKILL failed: {error:#}"));
        }

        let kill_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while !pending.is_empty() && tokio::time::Instant::now() < kill_deadline {
            let candidates = pending
                .iter()
                .map(|(pid, process)| (*pid, process.clone()))
                .collect::<Vec<_>>();
            for (pid, process) in candidates {
                match self.runtime.process.verified_stale_process_exists(&process) {
                    Ok(false) => {
                        pending.remove(&pid);
                        confirmed_gone.insert(pid);
                    }
                    Ok(true) => {}
                    Err(error) => {
                        pending.remove(&pid);
                        failures.insert(pid, format!("liveness check failed: {error:#}"));
                    }
                }
            }
            if !pending.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }

        for pid in pending.into_keys() {
            failures.insert(
                pid,
                "process or process group remained live after SIGKILL".to_owned(),
            );
        }
        let mut unresolved_containers = Vec::new();
        for service in &mut state.services {
            match service.pid {
                Some(pid) if confirmed_gone.contains(&pid) => {
                    service.pid = None;
                    service.process_identity = None;
                    // The shim owns its container as a foreground child, so a
                    // confirmed-dead shim/process group also confirms cleanup.
                    service.container_id = None;
                }
                Some(_) => {}
                None if service.container_id.is_some() => {
                    service.process_identity = None;
                    unresolved_containers.push(service.name.clone());
                }
                None => {
                    service.process_identity = None;
                }
            }
        }

        // Persist partial progress before returning an error. A later daemon
        // attempt retries only the handles that could not be confirmed gone.
        self.state_manager.save(&state).await?;

        if !failures.is_empty() || !unresolved_containers.is_empty() {
            let mut details = failures
                .into_iter()
                .map(|(pid, failure)| format!("PID {pid}: {failure}"))
                .collect::<Vec<_>>();
            details.extend(unresolved_containers.into_iter().map(|service| {
                format!("service `{service}` has container evidence without a confirmable PID")
            }));
            details.sort();
            anyhow::bail!(
                "stale locald runtime cleanup is incomplete; stop the recorded processes manually and restart locald. Preserved evidence: {}",
                details.join("; ")
            );
        }

        // Availability migration lands in the next productization task. Until
        // then, preserve the old restart promise only for identities that are
        // still active in the catalog. A path/status record alone can never
        // recreate a forgotten project.
        let registry = self.registry.lock().await;
        let mut legacy_evidence = HashMap::<ProjectInstanceId, Vec<PersistedServiceState>>::new();
        for service in state
            .services
            .iter()
            .filter(|service| service.status == ServiceState::Running)
        {
            if let Some(instance_id) =
                Self::active_catalog_instance_for_path(&registry, &service.path)
            {
                legacy_evidence
                    .entry(instance_id)
                    .or_default()
                    .push(service.clone());
            }
        }
        let legacy_instances = legacy_evidence.keys().copied().collect();
        drop(registry);
        *self.legacy_restore_evidence.lock().await = legacy_evidence;
        Ok(RuntimeRestorePlan { legacy_instances })
    }

    fn active_catalog_instance_for_path(
        registry: &Registry,
        path: &Path,
    ) -> Option<ProjectInstanceId> {
        let path = Self::canonicalize_path(path);
        let mut candidates = registry
            .instances
            .iter()
            .filter_map(|(instance_id, record)| {
                (record.current_path.as_deref() == Some(path.as_path())
                    || record.last_known_path == path)
                    .then_some(*instance_id)
            })
            .collect::<HashSet<_>>();
        if let Some(instance_id) = registry.legacy_paths.get(&path).copied() {
            candidates.insert(instance_id);
        }
        let instance_id = (candidates.len() == 1)
            .then(|| candidates.into_iter().next())
            .flatten()?;
        registry.instances.get(&instance_id).and_then(|record| {
            (record.presence == CatalogPresence::Active && record.current_path.is_some())
                .then_some(instance_id)
        })
    }

    /// Restore daemon-owned availability first, then the catalog-gated legacy
    /// restart promise that remains until availability migration is complete.
    /// The whole phase runs after IPC is online.
    pub(crate) async fn restore_policy_owned_projects(&self, plan: RuntimeRestorePlan) {
        if self.is_shutting_down() {
            return;
        }
        self.converge_all_project_availability().await;

        for instance_id in plan.legacy_instances {
            if self.is_shutting_down() {
                break;
            }
            let coordinator = self.availability_coordinator(instance_id).await;
            let _runtime_guard = coordinator.runtime.lock().await;
            if self.is_shutting_down() {
                break;
            }
            if !self
                .legacy_restore_evidence
                .lock()
                .await
                .contains_key(&instance_id)
            {
                continue;
            }
            match self.availability_is_managed(instance_id).await {
                Ok(true) => {
                    self.legacy_restore_evidence
                        .lock()
                        .await
                        .remove(&instance_id);
                    continue;
                }
                Ok(false) => {}
                Err(error) => {
                    warn!(
                        "Failed to inspect legacy restoration policy for {instance_id}: {error:#}"
                    );
                    continue;
                }
            }
            let Some(path) = self.active_path_for_instance(instance_id).await else {
                continue;
            };
            info!("Restoring catalogued legacy project instance {instance_id} at {path:?}");
            if self.forgotten_reload_paths.lock().await.contains(&path) {
                continue;
            }
            if let Err(error) = self
                .apply_config_for_instance(path.clone(), None, false, Some(instance_id), true)
                .await
            {
                warn!("Failed to restore legacy project instance {instance_id}: {error:#}");
                continue;
            }
            self.watch_active_instance(instance_id).await;
            self.legacy_restore_evidence
                .lock()
                .await
                .remove(&instance_id);
        }

        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        if !self.is_shutting_down() {
            self.persist_state().await;
        }
    }

    pub async fn handle_notify(&self, pid: u32) {
        let mut services = self.services.lock().await;
        for (name, service) in services.iter_mut() {
            if let ServiceRuntime::Controller(c) = &service.runtime_state {
                let state = c.lock().await.read_state().await;
                if state.pid == Some(pid) {
                    info!("Service {} is ready (via notify)", name);
                    if service.health_status != HealthStatus::Healthy
                        || service.health_source != HealthSource::Notify
                    {
                        service.health_status = HealthStatus::Healthy;
                        service.health_source = HealthSource::Notify;
                        Self::advance_service_projection(service);
                    }
                    break;
                }
            }
        }
    }

    async fn wait_for_health(&self, name: &str, instance_id: ProjectInstanceId) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30); // TODO: Make configurable

        loop {
            self.availability_allows_inflight_transition(instance_id)
                .await?;
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
                                if service.health_status != HealthStatus::Healthy {
                                    service.health_status = HealthStatus::Healthy;
                                    Self::advance_service_projection(service);
                                }
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
    async fn get_service_field(
        &self,
        name: &str,
        field: &str,
        expected_instance: ProjectInstanceId,
    ) -> Result<String> {
        // Re-acquire lock to get port, or just get it all at once?
        // The issue is holding the lock across await points or significant drops.
        // Let's get everything we need in one go.
        let (service_config, port_result) = {
            let services = self.services.lock().await;
            let service = services
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Service {name} not found"))?;
            anyhow::ensure!(
                service.instance_id == expected_instance,
                "service `{name}` belongs to project instance {}, not requesting instance {expected_instance}",
                service.instance_id
            );

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
                    let domains = manager.hosts_domains();
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
        self.ensure_accepting_lifecycle_requests()?;
        let path = Self::canonicalize_path(&path);
        if let Some((instance_id, _)) = self.availability_instance_for_path(&path).await {
            return self
                .start_catalogued_instance(instance_id, path, event_tx, verbose)
                .await;
        }

        self.start_runtime(path, event_tx, verbose).await
    }

    async fn start_runtime(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
    ) -> Result<()> {
        let (path, transition_lock) = self.transition_lock_for_path(&path).await;
        let _transition_guard = transition_lock.lock().await;
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.forgotten_reload_paths.lock().await.remove(&path);
        // Install the watcher while the same transition still owns tombstone
        // reactivation. Events during a slow build/readiness wait are queued;
        // their reload will take this lock after the initial apply finishes.
        self.watch_config(path.clone()).await;
        self.apply_config_locked(path, event_tx, verbose, None)
            .await
    }

    async fn reload_config(&self, path: PathBuf) -> Result<()> {
        let path = Self::canonicalize_path(&path);
        if self.forgotten_reload_paths.lock().await.contains(&path) {
            return Ok(());
        }
        if let Some((instance_id, _)) = self.availability_instance_for_path(&path).await {
            return self.reload_catalogued_instance(instance_id, path).await;
        }

        self.apply_config_for_instance(path, None, false, None, true)
            .await
    }

    async fn watch_config(&self, path: PathBuf) {
        let path = Self::canonicalize_path(&path);
        if self.forgotten_reload_paths.lock().await.contains(&path) {
            return;
        }
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
                            if let Err(e) = manager.reload_config(path_clone.clone()).await {
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
                    if self.forgotten_reload_paths.lock().await.contains(&path) {
                        return;
                    }
                    let mut watchers = self.watchers.lock().await;
                    watchers.insert(path, watcher);
                }
            }
            Err(e) => error!("Failed to create watcher: {e}"),
        }
    }

    async fn retire_config_reload_paths(
        &self,
        paths: HashSet<PathBuf>,
        instance_ids: HashSet<ProjectInstanceId>,
    ) {
        let paths = paths
            .into_iter()
            .map(|path| Self::canonicalize_path(&path))
            .collect::<HashSet<_>>();
        self.forgotten_reload_paths
            .lock()
            .await
            .extend(paths.iter().cloned());
        self.pending_config_reloads
            .lock()
            .await
            .retain(|instance_id| !instance_ids.contains(instance_id));
        self.watchers
            .lock()
            .await
            .retain(|path, _| !paths.contains(path));
    }

    pub async fn apply_config(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
    ) -> Result<()> {
        self.apply_config_for_instance(path, event_tx, verbose, None, false)
            .await
    }

    async fn apply_config_for_instance(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        expected_instance: Option<ProjectInstanceId>,
        reject_forgotten: bool,
    ) -> Result<()> {
        let (path, transition_lock) = self.transition_lock_for_path(&path).await;
        let _transition_guard = transition_lock.lock().await;
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        if reject_forgotten && self.forgotten_reload_paths.lock().await.contains(&path) {
            return Ok(());
        }
        self.apply_config_locked(path, event_tx, verbose, expected_instance)
            .await
    }

    async fn apply_config_locked(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        expected_instance: Option<ProjectInstanceId>,
    ) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
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
        let (
            commit_result,
            instance_id,
            removed_service_names,
            published_domain_index,
            mut reusable_service_envs,
        ) = {
            let mut registry = self.registry.lock().await;
            let mut candidate = registry.clone();
            let instance_id =
                candidate.register_project(discovery, Some(config.project.name.clone()))?;
            if let Some(expected_instance) = expected_instance {
                anyhow::ensure!(
                    registry.instances.contains_key(&expected_instance),
                    "project instance {expected_instance} is no longer catalogued"
                );
                anyhow::ensure!(
                    instance_id == expected_instance,
                    "project identity changed while applying config: expected {expected_instance}, discovered {instance_id}"
                );
            }
            let claims = Self::build_domain_claims(instance_id, &config, &path)?;
            candidate.replace_domain_claims(instance_id, claims)?;

            // Keep the previous claim set published until every removed or
            // restart-required service has stopped. A failed stop retains both
            // ownership and the retryable service record.
            let ConfigTransitionPlan {
                removed_service_names,
                restart_service_names,
                reusable_service_envs,
            } = self
                .prepublication_stop_plan(
                    instance_id,
                    &config,
                    &dot_env_vars,
                    &sorted_services,
                    &desired_service_names,
                )
                .await?;
            if expected_instance.is_some() {
                self.availability_authorizes_start(instance_id).await?;
            }
            for name in &removed_service_names {
                info!("Service {name} removed from config, stopping before domain publication...");
                self.stop_service_instance_locked(name, instance_id).await?;
            }
            for name in &restart_service_names {
                info!("Service {name} changed, stopping before domain publication...");
                self.stop_service_instance_locked(name, instance_id).await?;
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
            (
                commit_result,
                instance_id,
                removed_service_names,
                published_domain_index,
                reusable_service_envs,
            )
        };

        // The catalog rename is the ownership commit point. Synchronize hosts
        // from that exact snapshot even when the parent-directory fsync reports
        // PublishedNotDurable, then surface the durability result. Removed
        // service records leave runtime state at the same publication point.
        if let Some(published_domain_index) = published_domain_index {
            {
                let mut services = self.services.lock().await;
                for (name, resolved_env) in &reusable_service_envs {
                    if let Some(service) = services
                        .get_mut(name)
                        .filter(|service| service.instance_id == instance_id)
                    {
                        service.config = config.clone();
                        service.path.clone_from(&path);
                        service.resolved_env.clone_from(resolved_env);
                        Self::advance_service_projection(service);
                    }
                }
                for name in &removed_service_names {
                    services.remove(name);
                }
                self.domain_index.store(published_domain_index);
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
            self.availability_allows_inflight_transition(instance_id)
                .await?;
            let service_config = &config.services[&service_name];
            info!(
                "Service {}:{} config: {:?}",
                config.project.name, service_name, service_config
            );
            let name = format!("{}:{}", config.project.name, service_name);

            // A domain-only reload updates the shared claim snapshot and the
            // service's display configuration using the authoritative
            // prepublication reuse decision.
            if reusable_service_envs.remove(&name).is_some() {
                let reused = self
                    .services
                    .lock()
                    .await
                    .get(&name)
                    .is_some_and(|service| service.instance_id == instance_id);
                if reused {
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
            }

            let (combined_env, injected_database) =
                Self::effective_service_env(&config, &dot_env_vars, service_config);
            if let Some(dependency) = injected_database {
                info!(
                    "Auto-injected DATABASE_URL for {name} from Postgres dependency {dependency}"
                );
            }

            let manager = self.clone();
            let expected_instance = instance_id;
            let lookup = move |service_name: String, field: String| {
                let manager = manager.clone();
                async move {
                    manager
                        .get_service_field(&service_name, &field, expected_instance)
                        .await
                }
            };

            let resolved_env = ConfigLoader::resolve_env(&combined_env, &config, lookup).await?;

            let has_controller = {
                let services = self.services.lock().await;
                services.get(&name).is_some_and(|service| {
                    matches!(&service.runtime_state, ServiceRuntime::Controller(_))
                })
            };
            anyhow::ensure!(
                !has_controller,
                "service `{name}` changed after prepublication transition planning"
            );

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
                    let controller_generation = self
                        .next_controller_generation
                        .fetch_add(1, AtomicOrdering::Relaxed);

                    // Hook up logs immediately so we catch build logs
                    let manager = self.clone();
                    let controller_logs = {
                        let c = controller.lock().await;
                        c.logs().await
                    };
                    // Insert into map immediately so status is visible
                    {
                        let mut services = self.services.lock().await;
                        self.clear_foreign_log_buffer(&name, instance_id);
                        services.insert(
                            name.clone(),
                            Service {
                                instance_id,
                                controller_generation,
                                projection_generation: 1,
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
                    tokio::spawn(async move {
                        let mut logs = controller_logs;
                        while let Some(entry) = logs.next().await {
                            manager
                                .broadcast_log(instance_id, controller_generation, entry)
                                .await;
                        }
                    });

                    self.broadcast_service_update(&name).await;

                    {
                        let mut c = controller.lock().await;
                        c.prepare().await.context("Failed to prepare service")?;
                        // Broadcast update after prepare (state might be Building)
                    }
                    self.broadcast_service_update(&name).await;

                    self.availability_allows_inflight_transition(instance_id)
                        .await?;

                    let start_result = {
                        let mut c = controller.lock().await;
                        c.start().await.context("Failed to start service")
                    };
                    if let Err(start_error) = start_result {
                        if let Err(persistence_error) = self.persist_state_checked().await {
                            let rollback_result = self
                                .stop_service_instance_runtime_locked(&name, instance_id)
                                .await;
                            let retry_persistence_result = self.persist_state_checked().await;
                            self.broadcast_service_update(&name).await;

                            let recovery = match (rollback_result, retry_persistence_result) {
                                (Ok(()), Ok(())) => format!(
                                    "failed to persist retained ownership after service `{name}` start failure: {persistence_error:#}; the retained controller was stopped and the cleaned state was persisted"
                                ),
                                (Ok(()), Err(retry_persistence_error)) => format!(
                                    "failed to persist retained ownership after service `{name}` start failure: {persistence_error:#}; the retained controller was stopped, but persisting the cleaned state also failed: {retry_persistence_error:#}"
                                ),
                                (Err(rollback_error), Ok(())) => format!(
                                    "failed to persist retained ownership after service `{name}` start failure: {persistence_error:#}; rollback stop failed: {rollback_error:#}; the still-retained ownership was persisted on retry"
                                ),
                                (Err(rollback_error), Err(retry_persistence_error)) => format!(
                                    "failed to persist retained ownership after service `{name}` start failure: {persistence_error:#}; rollback stop failed: {rollback_error:#}; persisting the still-retained ownership also failed: {retry_persistence_error:#}"
                                ),
                            };
                            return Err(start_error.context(recovery));
                        }
                        return Err(start_error);
                    }

                    let state = controller.lock().await.read_state().await;

                    // Update service with final state (port might have changed if dynamic?)
                    {
                        let mut services = self.services.lock().await;
                        if let Some(service) = services
                            .get_mut(&name)
                            .filter(|service| service.instance_id == instance_id)
                        {
                            service.sticky_port = state.port;
                            service.health_status = state.health_status;
                        }
                    }

                    // A successful spawn is a crash-recovery boundary. Publish
                    // its controller-owned process identity before readiness
                    // can block or a later service can start. If publication
                    // fails, synchronously stop this controller so locald never
                    // leaves behind a child that the next daemon cannot own.
                    if let Err(persistence_error) = self.persist_state_checked().await {
                        let rollback_result = self
                            .stop_service_instance_runtime_locked(&name, instance_id)
                            .await;
                        self.persist_state().await;
                        self.broadcast_service_update(&name).await;

                        if let Err(rollback_error) = rollback_result {
                            anyhow::bail!(
                                "failed to persist ownership for started service `{name}`: {persistence_error:#}; rollback stop also failed: {rollback_error:#}"
                            );
                        }
                        return Err(persistence_error).with_context(|| {
                            format!(
                                "failed to persist ownership for started service `{name}`; the service was stopped"
                            )
                        });
                    }

                    self.broadcast_service_update(&name).await;

                    self.health_monitor.spawn_check(
                        name.clone(),
                        instance_id,
                        controller_generation,
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
            if let Err(e) = self.wait_for_health(&name, instance_id).await {
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

        self.legacy_restore_evidence
            .lock()
            .await
            .remove(&instance_id);
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
        loop {
            if let Some((path, _transition_guard, _runtime_projection_guard)) =
                self.lock_service_runtime_transition(name).await
            {
                self.ensure_accepting_lifecycle_requests()?;
                return self.stop_service_locked(name, &path).await;
            }

            let pending_instance = {
                let evidence = self.legacy_restore_evidence.lock().await;
                evidence.iter().find_map(|(instance_id, services)| {
                    services
                        .iter()
                        .any(|service| service.name == name)
                        .then_some(*instance_id)
                })
            };
            let Some(instance_id) = pending_instance else {
                return Ok(());
            };

            let coordinator = self.availability_coordinator(instance_id).await;
            let _availability_guard = coordinator.runtime.lock().await;
            self.ensure_accepting_lifecycle_requests()?;
            let still_pending = self
                .legacy_restore_evidence
                .lock()
                .await
                .get(&instance_id)
                .is_some_and(|services| services.iter().any(|service| service.name == name));
            if still_pending {
                return Err(ServiceRestorePending {
                    name: name.to_owned(),
                    instance_id,
                }
                .into());
            }
        }
    }

    async fn stop_service_locked(&self, name: &str, project_path: &Path) -> Result<()> {
        let instance_id = {
            let services = self.services.lock().await;
            if let Some(service) = services.get(name) {
                anyhow::ensure!(
                    service.path == project_path,
                    "service `{name}` changed project during lifecycle transition"
                );
                service.instance_id
            } else {
                return Ok(());
            }
        };

        self.stop_service_instance_locked(name, instance_id).await
    }

    async fn stop_service_instance_locked(
        &self,
        name: &str,
        instance_id: ProjectInstanceId,
    ) -> Result<()> {
        self.stop_service_instance_runtime_locked(name, instance_id)
            .await?;
        self.persist_state().await;
        self.broadcast_service_update(name).await;
        Ok(())
    }

    async fn stop_service_instance_runtime_locked(
        &self,
        name: &str,
        instance_id: ProjectInstanceId,
    ) -> Result<()> {
        let runtime_state = {
            let services = self.services.lock().await;
            if let Some(service) = services.get(name) {
                anyhow::ensure!(
                    service.instance_id == instance_id,
                    "service `{name}` changed project instance during lifecycle transition"
                );
                service.runtime_state.clone()
            } else {
                return Ok(());
            }
        };

        match runtime_state {
            ServiceRuntime::Controller(c) => {
                let stop_result = c.lock().await.stop().await;
                if let Err(e) = stop_result {
                    warn!("Failed to stop service {name}: {e}");
                    return Err(e).with_context(|| format!("failed to stop service `{name}`"));
                }
                let mut services = self.services.lock().await;
                if let Some(service) = services.get_mut(name) {
                    anyhow::ensure!(
                        service.instance_id == instance_id,
                        "service `{name}` changed project instance during lifecycle transition"
                    );
                    let same_controller = matches!(
                        &service.runtime_state,
                        ServiceRuntime::Controller(current) if Arc::ptr_eq(current, &c)
                    );
                    anyhow::ensure!(
                        same_controller,
                        "service `{name}` changed controller during lifecycle transition"
                    );
                    service.runtime_state = ServiceRuntime::None;
                }
            }
            ServiceRuntime::None => {}
        }

        // Clear health and broadcast after stop
        {
            let mut services = self.services.lock().await;
            if let Some(service) = services
                .get_mut(name)
                .filter(|service| service.instance_id == instance_id)
            {
                // Note: We do NOT clear sticky_port here, so we can reuse it on restart.
                service.health_status = HealthStatus::Unknown;
                Self::advance_service_projection(service);
            }
        }
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        let (pending_instances, pending_paths) = {
            let evidence = self.legacy_restore_evidence.lock().await;
            (
                evidence.keys().copied().collect::<Vec<_>>(),
                evidence
                    .values()
                    .flatten()
                    .map(|service| Self::canonicalize_path(&service.path))
                    .collect::<HashSet<_>>(),
            )
        };
        for instance_id in pending_instances {
            let coordinator = self.availability_coordinator(instance_id).await;
            let _availability_guard = coordinator.runtime.lock().await;
            let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
            self.ensure_accepting_lifecycle_requests()?;
            self.legacy_restore_evidence
                .lock()
                .await
                .remove(&instance_id);
            self.persist_state().await;
        }

        let mut paths = {
            let services = self.services.lock().await;
            services
                .values()
                .map(|service| Self::canonicalize_path(&service.path))
                .collect::<HashSet<_>>()
        };
        paths.extend(pending_paths);
        let mut paths = paths.into_iter().collect::<Vec<_>>();
        paths.sort();

        for path in paths {
            if let Err(e) = self.stop_project(&path).await {
                error!("Failed to stop project at {}: {e}", path.display());
            }
        }
        Ok(())
    }

    pub async fn stop_project(&self, project_path: &Path) -> Result<()> {
        self.stop_project_unless_shutting_down(project_path).await
    }

    async fn stop_project_unless_shutting_down(&self, project_path: &Path) -> Result<()> {
        let instance_id = self
            .availability_instance_for_path(project_path)
            .await
            .map(|(instance_id, _)| instance_id);
        let coordinator = match instance_id {
            Some(instance_id) => Some(self.availability_coordinator(instance_id).await),
            None => None,
        };
        let _availability_guard = match coordinator.as_ref() {
            Some(coordinator) => Some(coordinator.runtime.lock().await),
            None => None,
        };
        let (project_path, transition_lock) = self.transition_lock_for_path(project_path).await;
        let _transition_guard = transition_lock.lock().await;
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        if self.is_shutting_down() {
            return Ok(());
        }
        self.retire_legacy_restore_for_path(&project_path).await;
        self.stop_project_locked(&project_path).await?;
        self.persist_state().await;
        Ok(())
    }

    async fn retire_legacy_restore_for_path(&self, project_path: &Path) {
        let instance_id = {
            let registry = self.registry.lock().await;
            Self::active_catalog_instance_for_path(&registry, project_path)
        };
        if let Some(instance_id) = instance_id {
            self.legacy_restore_evidence
                .lock()
                .await
                .remove(&instance_id);
        }
    }

    async fn stop_project_locked(&self, project_path: &Path) -> Result<()> {
        let service_names: Vec<String> = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter(|(_, service)| service.path == project_path)
                .map(|(name, _)| name.clone())
                .collect()
        };

        for name in service_names {
            self.stop_service_locked(&name, project_path).await?;
        }
        Ok(())
    }

    async fn stop_project_instance_locked(&self, instance_id: ProjectInstanceId) -> Result<()> {
        let service_names: Vec<String> = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter(|(_, service)| service.instance_id == instance_id)
                .map(|(name, _)| name.clone())
                .collect()
        };
        if service_names.is_empty() {
            return Ok(());
        }

        let mut stopped_service_names: Vec<String> = Vec::new();
        for name in service_names {
            if let Err(error) = self
                .stop_service_instance_runtime_locked(&name, instance_id)
                .await
            {
                self.persist_state().await;
                for stopped_name in stopped_service_names {
                    self.broadcast_service_update(&stopped_name).await;
                }
                return Err(error);
            }
            stopped_service_names.push(name);
        }
        self.persist_state().await;
        for name in stopped_service_names {
            self.broadcast_service_update(&name).await;
        }
        Ok(())
    }

    pub async fn restart(&self, name: &str) -> Result<()> {
        let Some((path, _transition_guard, _runtime_projection_guard)) =
            self.lock_service_runtime_transition(name).await
        else {
            return Err(ServiceNotFoundError.into());
        };
        self.ensure_accepting_lifecycle_requests()?;
        self.stop_service_locked(name, &path).await?;
        self.watch_config(path.clone()).await;
        self.apply_config_locked(path, None, false, None).await
    }

    async fn restart_project(&self, project_path: &Path) -> Result<()> {
        let (project_path, transition_lock) = self.transition_lock_for_path(project_path).await;
        let _transition_guard = transition_lock.lock().await;
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.stop_project_locked(&project_path).await?;
        self.watch_config(project_path.clone()).await;
        self.apply_config_locked(project_path, None, false, None)
            .await
    }

    pub async fn restart_all(&self) -> Result<()> {
        // 1. Collect unique project paths
        let mut paths: Vec<PathBuf> = {
            let services = self.services.lock().await;
            services
                .values()
                .map(|service| Self::canonicalize_path(&service.path))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        };
        paths.sort();

        // Restart one project transition at a time.
        for path in paths {
            if let Err(e) = self.restart_project(&path).await {
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

        let Some((path, _transition_guard, _runtime_projection_guard)) =
            self.lock_service_runtime_transition(name).await
        else {
            anyhow::bail!("Service {name} not found");
        };
        self.ensure_accepting_lifecycle_requests()?;

        // 1. Stop the service
        self.stop_service_locked(name, &path).await?;

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

        // 3. Restart while retaining the same project transition boundary.
        self.watch_config(path.clone()).await;
        self.apply_config_locked(path, None, false, None).await?;

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
                    self.domain_for_service(service.instance_id, name),
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
        self.shutting_down.store(true, AtomicOrdering::Release);
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        let _availability_publication_guard = self.availability_publication_lock.lock().await;
        // Availability convergence deliberately avoids the global runtime
        // projection lock so unrelated projects can stop independently. Drain
        // every coordinator that existed when shutdown began before taking the
        // global teardown lock. A coordinator created after this snapshot sees
        // `shutting_down` at the convergence boundary before any runtime action.
        let mut coordinators = {
            let coordinators = self.availability_coordinators.lock().await;
            coordinators
                .iter()
                .map(|(instance_id, coordinator)| (*instance_id, coordinator.clone()))
                .collect::<Vec<_>>()
        };
        coordinators.sort_by_key(|(instance_id, _)| *instance_id);
        let mut availability_guards = Vec::with_capacity(coordinators.len());
        for (_, coordinator) in &coordinators {
            availability_guards.push(coordinator.runtime.lock().await);
        }
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
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
        drop(availability_guards);

        Ok(())
    }

    pub async fn resolve_service_by_domain(
        &self,
        domain: &str,
    ) -> Option<locald_core::resolver::DomainResolution> {
        let (claimed_instance_id, service_name) = {
            let index = self.domain_index.snapshot();
            match index.resolve(domain) {
                Some(DomainTarget::Service {
                    project_instance_id,
                    service_name: Some(service_name),
                }) => (*project_instance_id, service_name.clone()),
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
                .filter(|service| service.instance_id == claimed_instance_id)
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
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
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
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
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
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        {
            let mut attachments = self.attachments.lock().await;
            attachments.clear_stopped(&project_path);
            let _ = attachments.save().await;
        }
        self.start(project_path, None, false).await
    }

    pub async fn project_force_stop(&self, project_path: PathBuf) -> Result<()> {
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        {
            let mut attachments = self.attachments.lock().await;
            attachments.mark_stopped(&project_path);
            let _ = attachments.save().await;
        }
        self.stop_project(&project_path).await
    }

    /// Acquire or renew semantic availability and converge the project runtime.
    ///
    /// This is the daemon-owned primitive used by compatibility and future
    /// lifecycle IPC. Public readiness semantics are layered on it separately.
    pub async fn project_ensure_availability(
        &self,
        project_path: &Path,
        demand: DemandKey,
    ) -> Result<EnsureDemandResult> {
        let (instance_id, _) = self
            .required_availability_instance_for_path(project_path)
            .await?;
        let coordinator = self.availability_coordinator(instance_id).await;
        let _runtime_guard = coordinator.runtime.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        anyhow::ensure!(
            self.active_path_for_instance(instance_id).await.is_some(),
            "project instance {instance_id} is no longer active"
        );
        self.watch_active_instance(instance_id).await;
        let mut availability =
            AvailabilityStore::load(&self.availability_data_dir, instance_id).await?;
        let (result, durability_error) =
            Self::capture_availability_publication(availability.ensure_demand(demand).await)?;
        let convergence = self
            .converge_availability_locked(
                instance_id,
                None,
                None,
                AvailabilityConvergenceOptions {
                    verbose: false,
                    apply_config_when_up: false,
                    defer_config_reload_during_cooldown: false,
                },
            )
            .await;
        Self::surface_availability_durability(convergence, durability_error)?;
        Ok(result.expect("successful availability publication returns its demand result"))
    }

    /// Enable or disable durable Always On policy and converge the runtime.
    pub async fn project_set_always_on(&self, project_path: &Path, enabled: bool) -> Result<bool> {
        let (instance_id, _) = self
            .required_availability_instance_for_path(project_path)
            .await?;
        let coordinator = self.availability_coordinator(instance_id).await;
        let _runtime_guard = coordinator.runtime.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        anyhow::ensure!(
            self.active_path_for_instance(instance_id).await.is_some(),
            "project instance {instance_id} is no longer active"
        );
        self.watch_active_instance(instance_id).await;
        let mut availability =
            AvailabilityStore::load(&self.availability_data_dir, instance_id).await?;
        let (changed, durability_error) =
            Self::capture_availability_publication(availability.set_always_on(enabled).await)?;
        let convergence = self
            .converge_availability_locked(
                instance_id,
                None,
                None,
                AvailabilityConvergenceOptions {
                    verbose: false,
                    apply_config_when_up: false,
                    defer_config_reload_during_cooldown: false,
                },
            )
            .await;
        Self::surface_availability_durability(convergence, durability_error)?;
        Ok(changed.expect("successful availability publication returns its change result"))
    }

    /// Pause a project through its current activity generation and stop it.
    pub async fn project_pause_availability(&self, project_path: &Path) -> Result<bool> {
        let (instance_id, _) = self
            .required_availability_instance_for_path(project_path)
            .await?;
        let (changed, durability_error) = {
            let _publication_guard = self.availability_publication_lock.lock().await;
            self.ensure_accepting_lifecycle_requests()?;
            let mut availability =
                AvailabilityStore::load(&self.availability_data_dir, instance_id).await?;
            Self::capture_availability_publication(availability.pause_project().await)?
        };
        let convergence = self
            .converge_managed_instance(instance_id, None, false, false)
            .await;
        Self::surface_availability_durability(convergence, durability_error)?;
        Ok(changed.expect("successful availability publication returns its change result"))
    }

    /// Re-evaluate one availability-managed project from authoritative state.
    ///
    /// Returns `None` while the project still uses the legacy lifecycle model.
    pub async fn converge_project_availability(
        &self,
        project_path: &Path,
    ) -> Result<Option<ConvergenceDecision>> {
        let Some((instance_id, _)) = self.availability_instance_for_path(project_path).await else {
            return Ok(None);
        };
        if !self.availability_is_managed(instance_id).await? {
            return Ok(None);
        }
        self.converge_managed_instance(instance_id, None, false, false)
            .await
            .map(Some)
    }

    pub async fn remove_project(&self, project_path: &Path) -> Result<()> {
        let instance_id = self
            .availability_instance_for_path(project_path)
            .await
            .map(|(instance_id, _)| instance_id);
        let coordinator = match instance_id {
            Some(instance_id) => Some(self.availability_coordinator(instance_id).await),
            None => None,
        };
        let _availability_guard = match coordinator.as_ref() {
            Some(coordinator) => Some(coordinator.runtime.lock().await),
            None => None,
        };
        let (canonical, transition_lock) = self.transition_lock_for_path(project_path).await;
        let _transition_guard = transition_lock.lock().await;
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let mut retired_paths = HashSet::from([canonical.clone()]);
        let retired_instance_ids = instance_id.into_iter().collect::<HashSet<_>>();
        if let Some(instance_id) = instance_id {
            retired_paths.extend(
                self.services
                    .lock()
                    .await
                    .values()
                    .filter(|service| service.instance_id == instance_id)
                    .map(|service| service.path.clone()),
            );
        }

        // Stop services by stable identity when one is available so a moved
        // runtime cannot outlive removal at its current catalog locator.
        if let Some(instance_id) = instance_id {
            self.stop_project_instance_locked(instance_id).await?;
        } else {
            self.stop_project_locked(&canonical).await?;
        }

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
        if let Some(instance_id) = instance_id
            && let Some(record) = registry.instances.get(&instance_id)
        {
            retired_paths.insert(record.last_known_path.clone());
            retired_paths.extend(record.current_path.iter().cloned());
            retired_paths.extend(
                registry
                    .legacy_paths
                    .iter()
                    .filter(|(_, candidate)| **candidate == instance_id)
                    .map(|(path, _)| path.clone()),
            );
        }
        catalog_candidate.unregister_project(&canonical)?;
        let surviving_paths = catalog_candidate
            .instances
            .values()
            .flat_map(|record| {
                std::iter::once(record.last_known_path.clone())
                    .chain(record.current_path.iter().cloned())
            })
            .chain(catalog_candidate.legacy_paths.keys().cloned())
            .map(|path| Self::canonicalize_path(&path))
            .collect::<HashSet<_>>();
        retired_paths.retain(|path| !surviving_paths.contains(&Self::canonicalize_path(path)));

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

        self.retire_config_reload_paths(retired_paths, retired_instance_ids)
            .await;
        self.services.lock().await.retain(|_, service| {
            instance_id.map_or(service.path != canonical, |instance_id| {
                service.instance_id != instance_id
            })
        });
        if let Some(instance_id) = instance_id {
            self.legacy_restore_evidence
                .lock()
                .await
                .remove(&instance_id);
        }
        self.persist_state().await;
        self.sync_hosts_after_catalog_publish().await;

        if let Some(error) = durability_error {
            return Err(error.into());
        }

        Ok(())
    }

    pub async fn project_status(&self, project_path: &Path) -> Result<ProjectStatusInfo> {
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
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
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
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
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
        loop {
            // Snapshot before taking project locks so every operation observes
            // the same transition -> registry lock order. Revalidate after all
            // canonical project locks are held; a concurrent catalog change
            // simply restarts the clean from a fresh snapshot.
            let (baseline, mut project_paths) = {
                let registry = self.registry.lock().await;
                let paths = registry
                    .instances
                    .values()
                    .map(|record| Self::canonicalize_path(&record.last_known_path))
                    .collect::<Vec<_>>();
                (registry.clone(), paths)
            };
            project_paths.sort();
            project_paths.dedup();

            let mut transition_guards = Vec::with_capacity(project_paths.len());
            for path in &project_paths {
                let (_, transition_lock) = self.transition_lock_for_path(path).await;
                transition_guards.push(transition_lock.lock_owned().await);
            }
            let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
            self.ensure_accepting_lifecycle_requests()?;

            let mut registry = self.registry.lock().await;
            if *registry != baseline {
                drop(registry);
                drop(transition_guards);
                continue;
            }

            let mut updated = registry.clone();
            let count = updated.prune_missing_projects()?;
            let removed_instance_ids = registry
                .instances
                .keys()
                .filter(|instance_id| !updated.instances.contains_key(instance_id))
                .copied()
                .collect::<HashSet<_>>();
            let mut removed_paths = registry
                .instances
                .iter()
                .filter(|(instance_id, _)| removed_instance_ids.contains(instance_id))
                .flat_map(|(_, record)| {
                    std::iter::once(record.last_known_path.clone())
                        .chain(record.current_path.iter().cloned())
                })
                .collect::<HashSet<_>>();
            removed_paths.extend(
                registry
                    .legacy_paths
                    .iter()
                    .filter(|(_, instance_id)| removed_instance_ids.contains(instance_id))
                    .map(|(path, _)| path.clone()),
            );
            removed_paths.extend(
                self.services
                    .lock()
                    .await
                    .values()
                    .filter(|service| removed_instance_ids.contains(&service.instance_id))
                    .map(|service| service.path.clone()),
            );
            let surviving_paths = updated
                .instances
                .values()
                .flat_map(|record| {
                    std::iter::once(record.last_known_path.clone())
                        .chain(record.current_path.iter().cloned())
                })
                .chain(updated.legacy_paths.keys().cloned())
                .map(|path| Self::canonicalize_path(&path))
                .collect::<HashSet<_>>();
            removed_paths.retain(|path| !surviving_paths.contains(&Self::canonicalize_path(path)));

            for instance_id in &removed_instance_ids {
                self.stop_project_instance_locked(*instance_id).await?;
            }

            if updated == *registry {
                return Ok(count);
            }

            let commit_result = registry.commit_candidate(updated).await;
            self.domain_index.store(registry.domain_index().clone());
            let catalog_published = commit_result.is_ok()
                || matches!(
                    &commit_result,
                    Err(CatalogError::PublishedNotDurable { .. })
                );
            if catalog_published {
                self.retire_config_reload_paths(
                    removed_paths.clone(),
                    removed_instance_ids.clone(),
                )
                .await;
                self.services
                    .lock()
                    .await
                    .retain(|_, service| !removed_instance_ids.contains(&service.instance_id));
                self.legacy_restore_evidence
                    .lock()
                    .await
                    .retain(|instance_id, _| !removed_instance_ids.contains(instance_id));
                self.persist_state().await;
                self.sync_hosts_after_catalog_publish().await;
            }
            commit_result?;
            return Ok(count);
        }
    }

    pub async fn reap_and_stop_orphans(&self) {
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
        if self.is_shutting_down() {
            return;
        }
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
            if let Err(e) = self.stop_project_unless_shutting_down(&path).await {
                warn!("Failed to stop orphaned project: {e}");
            }
        }
    }

    /// Re-evaluate every project that has entered the availability lifecycle.
    pub async fn converge_all_project_availability(&self) {
        if self.is_shutting_down() {
            return;
        }
        let instance_ids = {
            let registry = self.registry.lock().await;
            registry.instances.keys().copied().collect::<Vec<_>>()
        };

        for instance_id in instance_ids {
            if self.is_shutting_down() {
                break;
            }
            match self.availability_is_managed(instance_id).await {
                Ok(true) => {
                    if let Err(error) = self
                        .converge_managed_instance(instance_id, None, false, false)
                        .await
                    {
                        warn!("Failed to converge availability for {instance_id}: {error:#}");
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    warn!("Failed to inspect availability for {instance_id}: {error:#}");
                }
            }
        }
    }

    async fn availability_instance_for_path(
        &self,
        path: &Path,
    ) -> Option<(ProjectInstanceId, PathBuf)> {
        let canonical = Self::canonicalize_path(path);
        if let Ok(discovery) = Registry::discover(canonical.clone()).await {
            let registry = self.registry.lock().await;
            let mut candidate = registry.clone();
            let instance_id = candidate.register_project(discovery, None).ok()?;
            return registry
                .instances
                .contains_key(&instance_id)
                .then_some((instance_id, canonical));
        }

        let registry = self.registry.lock().await;
        registry.instances.iter().find_map(|(instance_id, record)| {
            (record.presence == CatalogPresence::Active
                && record.current_path.as_deref() == Some(canonical.as_path()))
            .then_some((*instance_id, canonical.clone()))
        })
    }

    async fn required_availability_instance_for_path(
        &self,
        path: &Path,
    ) -> Result<(ProjectInstanceId, PathBuf)> {
        self.availability_instance_for_path(path)
            .await
            .with_context(|| {
                format!(
                    "project `{}` is not registered in the identity catalog",
                    path.display()
                )
            })
    }

    async fn availability_is_managed(&self, instance_id: ProjectInstanceId) -> Result<bool> {
        let path = availability_path(&self.availability_data_dir, instance_id);
        tokio::fs::try_exists(&path)
            .await
            .with_context(|| format!("failed to inspect availability state `{}`", path.display()))
    }

    async fn sweep_availability(
        availability: &mut AvailabilityStore,
    ) -> Result<(ConvergenceDecision, Option<anyhow::Error>)> {
        match availability.sweep_and_decide().await {
            Ok(decision) => Ok((decision, None)),
            Err(error @ AvailabilityError::PublishedNotDurable { .. }) => {
                let decision = availability.sweep_and_decide().await.with_context(|| {
                    format!(
                        "availability was published with incomplete durability ({error}); failed to read the published decision"
                    )
                })?;
                Ok((decision, Some(error.into())))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn capture_availability_publication<Output>(
        result: std::result::Result<Output, AvailabilityError>,
    ) -> Result<(Option<Output>, Option<anyhow::Error>)> {
        match result {
            Ok(output) => Ok((Some(output), None)),
            Err(error @ AvailabilityError::PublishedNotDurable { .. }) => {
                Ok((None, Some(error.into())))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn merge_availability_durability(
        current: &mut Option<anyhow::Error>,
        next: Option<anyhow::Error>,
    ) {
        let Some(next) = next else {
            return;
        };
        *current = Some(match current.take() {
            Some(previous) => previous.context(format!(
                "an additional availability publication had incomplete durability: {next:#}"
            )),
            None => next,
        });
    }

    fn surface_availability_durability<Output>(
        result: Result<Output>,
        durability_error: Option<anyhow::Error>,
    ) -> Result<Output> {
        match (result, durability_error) {
            (Ok(_), Some(error)) => Err(error),
            (Err(action_error), Some(error)) => Err(action_error.context(format!(
                "availability state was published with incomplete durability: {error:#}"
            ))),
            (result, None) => result,
        }
    }

    async fn availability_authorizes_start(&self, instance_id: ProjectInstanceId) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        if !self.availability_is_managed(instance_id).await? {
            return Ok(());
        }
        let mut availability =
            AvailabilityStore::load(&self.availability_data_dir, instance_id).await?;
        let decision = availability.sweep_and_decide().await?;
        if matches!(decision, ConvergenceDecision::EnsureUp) {
            Ok(())
        } else {
            Err(AvailabilityStartSuperseded {
                instance_id,
                decision,
            }
            .into())
        }
    }

    async fn availability_allows_inflight_transition(
        &self,
        instance_id: ProjectInstanceId,
    ) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        if !self.availability_is_managed(instance_id).await? {
            return Ok(());
        }
        let mut availability =
            AvailabilityStore::load(&self.availability_data_dir, instance_id).await?;
        let snapshot = availability.snapshot().await?;
        if snapshot.is_paused() {
            Err(AvailabilityStartSuperseded {
                instance_id,
                decision: ConvergenceDecision::EnsureDown,
            }
            .into())
        } else {
            Ok(())
        }
    }

    async fn availability_coordinator(
        &self,
        instance_id: ProjectInstanceId,
    ) -> Arc<AvailabilityCoordinator> {
        let mut coordinators = self.availability_coordinators.lock().await;
        coordinators
            .entry(instance_id)
            .or_insert_with(|| Arc::new(AvailabilityCoordinator::new()))
            .clone()
    }

    async fn active_path_for_instance(&self, instance_id: ProjectInstanceId) -> Option<PathBuf> {
        let registry = self.registry.lock().await;
        registry.instances.get(&instance_id).and_then(|record| {
            (record.presence == CatalogPresence::Active)
                .then(|| record.current_path.clone())
                .flatten()
        })
    }

    async fn watch_active_instance(&self, instance_id: ProjectInstanceId) {
        if self.is_shutting_down() {
            return;
        }
        if let Some(path) = self.active_path_for_instance(instance_id).await {
            let (path, transition_lock) = self.transition_lock_for_path(&path).await;
            let _transition_guard = transition_lock.lock().await;
            if !self.is_shutting_down() && self.path_matches_instance(&path, instance_id).await {
                self.forgotten_reload_paths.lock().await.remove(&path);
                self.watch_config(path).await;
            }
        }
    }

    async fn path_matches_instance(&self, path: &Path, instance_id: ProjectInstanceId) -> bool {
        let canonical = Self::canonicalize_path(path);
        let Ok(discovery) = Registry::discover(canonical).await else {
            return false;
        };
        let registry = self.registry.lock().await;
        if !registry.instances.contains_key(&instance_id) {
            return false;
        }
        let mut candidate = registry.clone();
        candidate.register_project(discovery, None).ok() == Some(instance_id)
    }

    async fn project_runtime_exists(&self, instance_id: ProjectInstanceId) -> bool {
        let services = self.services.lock().await;
        services.values().any(|service| {
            service.instance_id == instance_id
                && (matches!(&service.runtime_state, ServiceRuntime::Controller(_))
                    || service.health_status != HealthStatus::Unknown)
        })
    }

    async fn project_runtime_is_ready(&self, instance_id: ProjectInstanceId) -> bool {
        let runtimes = {
            let services = self.services.lock().await;
            services
                .values()
                .filter(|service| service.instance_id == instance_id)
                .map(|service| (service.runtime_state.clone(), service.health_status))
                .collect::<Vec<_>>()
        };
        if runtimes.is_empty() {
            return false;
        }

        for (runtime, health_status) in runtimes {
            if health_status != HealthStatus::Healthy {
                return false;
            }
            match runtime {
                ServiceRuntime::None => return false,
                ServiceRuntime::Controller(controller) => {
                    if controller.lock().await.read_state().await.status != ServiceState::Running {
                        return false;
                    }
                }
            }
        }
        true
    }

    async fn stop_project_instance(&self, instance_id: ProjectInstanceId) -> Result<()> {
        let mut paths = {
            let services = self.services.lock().await;
            services
                .values()
                .filter(|service| service.instance_id == instance_id)
                .map(|service| Self::canonicalize_path(&service.path))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        };
        if let Some(record) = self.registry.lock().await.instances.get(&instance_id) {
            paths.push(Self::canonicalize_path(&record.last_known_path));
            paths.extend(
                record
                    .current_path
                    .iter()
                    .map(|path| Self::canonicalize_path(path)),
            );
        }
        paths.sort();
        paths.dedup();

        let mut transition_guards = Vec::with_capacity(paths.len());
        for path in paths {
            let (_, transition_lock) = self.transition_lock_for_path(&path).await;
            transition_guards.push(transition_lock.lock_owned().await);
        }
        let result = self.stop_project_instance_locked(instance_id).await;
        drop(transition_guards);
        result
    }

    async fn start_catalogued_instance(
        &self,
        instance_id: ProjectInstanceId,
        requested_path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
    ) -> Result<()> {
        let coordinator = self.availability_coordinator(instance_id).await;
        let _runtime_guard = coordinator.runtime.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let requested_path = if self
            .path_matches_instance(&requested_path, instance_id)
            .await
        {
            Some(Self::canonicalize_path(&requested_path))
        } else {
            None
        };

        if self.availability_is_managed(instance_id).await? {
            if let Some(path) = requested_path
                .clone()
                .or(self.active_path_for_instance(instance_id).await)
            {
                self.watch_config(path).await;
            }
            self.converge_availability_locked(
                instance_id,
                requested_path,
                event_tx,
                AvailabilityConvergenceOptions {
                    verbose,
                    apply_config_when_up: true,
                    defer_config_reload_during_cooldown: false,
                },
            )
            .await?;
            return Ok(());
        }

        let path = requested_path
            .or(self.active_path_for_instance(instance_id).await)
            .context("catalogued project instance has no active path")?;
        let action = self.start_runtime(path, event_tx, verbose).await;

        if self.availability_is_managed(instance_id).await? {
            self.converge_availability_locked(
                instance_id,
                None,
                None,
                AvailabilityConvergenceOptions {
                    verbose: false,
                    apply_config_when_up: false,
                    defer_config_reload_during_cooldown: false,
                },
            )
            .await?;
            Ok(())
        } else {
            action
        }
    }

    async fn reload_catalogued_instance(
        &self,
        instance_id: ProjectInstanceId,
        requested_path: PathBuf,
    ) -> Result<()> {
        let coordinator = self.availability_coordinator(instance_id).await;
        let _runtime_guard = coordinator.runtime.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let requested_path = if self
            .path_matches_instance(&requested_path, instance_id)
            .await
        {
            Some(Self::canonicalize_path(&requested_path))
        } else {
            None
        };

        if self.availability_is_managed(instance_id).await? {
            self.converge_availability_locked(
                instance_id,
                requested_path,
                None,
                AvailabilityConvergenceOptions {
                    verbose: false,
                    apply_config_when_up: true,
                    defer_config_reload_during_cooldown: true,
                },
            )
            .await?;
            return Ok(());
        }

        let path = requested_path
            .or(self.active_path_for_instance(instance_id).await)
            .context("catalogued project instance has no active path")?;
        let action = self
            .apply_config_for_instance(path, None, false, None, true)
            .await;

        if self.availability_is_managed(instance_id).await? {
            self.converge_availability_locked(
                instance_id,
                None,
                None,
                AvailabilityConvergenceOptions {
                    verbose: false,
                    apply_config_when_up: true,
                    defer_config_reload_during_cooldown: true,
                },
            )
            .await?;
            Ok(())
        } else {
            action
        }
    }

    async fn converge_managed_instance(
        &self,
        instance_id: ProjectInstanceId,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        apply_config_when_up: bool,
    ) -> Result<ConvergenceDecision> {
        let coordinator = self.availability_coordinator(instance_id).await;
        let _runtime_guard = coordinator.runtime.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.watch_active_instance(instance_id).await;
        let decision = self
            .converge_availability_locked(
                instance_id,
                None,
                event_tx,
                AvailabilityConvergenceOptions {
                    verbose,
                    apply_config_when_up,
                    defer_config_reload_during_cooldown: false,
                },
            )
            .await?;
        Ok(decision)
    }

    async fn converge_availability_locked(
        &self,
        instance_id: ProjectInstanceId,
        mut requested_path: Option<PathBuf>,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        options: AvailabilityConvergenceOptions,
    ) -> Result<ConvergenceDecision> {
        let mut durability_error = None;
        loop {
            self.ensure_accepting_lifecycle_requests()?;
            let mut availability =
                match AvailabilityStore::load(&self.availability_data_dir, instance_id).await {
                    Ok(availability) => availability,
                    Err(error) => {
                        return Self::surface_availability_durability(
                            Err(error.into()),
                            durability_error,
                        );
                    }
                };
            let (decision, sweep_durability_error) =
                match Self::sweep_availability(&mut availability).await {
                    Ok(result) => result,
                    Err(error) => {
                        return Self::surface_availability_durability(Err(error), durability_error);
                    }
                };
            Self::merge_availability_durability(&mut durability_error, sweep_durability_error);
            let snapshot = match availability.snapshot().await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return Self::surface_availability_durability(
                        Err(error.into()),
                        durability_error,
                    );
                }
            };
            let project_path = requested_path
                .take()
                .or(self.active_path_for_instance(instance_id).await);

            let Some(project_path) = project_path else {
                let action = self.stop_project_instance(instance_id).await;
                if matches!(decision, ConvergenceDecision::EnsureUp) && action.is_ok() {
                    let message =
                        format!("project instance {instance_id} is missing from the filesystem");
                    let result = match availability
                        .record_convergence_error(message.clone())
                        .await
                    {
                        Ok(_) => Err(anyhow::anyhow!(message)),
                        Err(error @ AvailabilityError::PublishedNotDurable { .. }) => {
                            Err(anyhow::anyhow!(message).context(format!(
                                "the missing-project convergence error was published with incomplete durability: {error}"
                            )))
                        }
                        Err(error) => Err(anyhow::anyhow!(message).context(format!(
                            "failed to record the missing-project convergence error: {error}"
                        ))),
                    };
                    return Self::surface_availability_durability(result, durability_error);
                }
                let result = self
                    .finish_availability_action(
                        &mut availability,
                        decision,
                        action,
                        matches!(decision, ConvergenceDecision::EnsureDown),
                    )
                    .await;
                return Self::surface_availability_durability(result, durability_error);
            };

            let (action, clear_on_success, applied_config) = match decision {
                ConvergenceDecision::EnsureUp => {
                    let has_pending_reload = self
                        .pending_config_reloads
                        .lock()
                        .await
                        .contains(&instance_id);
                    let should_apply = options.apply_config_when_up
                        || has_pending_reload
                        || snapshot.last_convergence_error().is_some()
                        || !self.project_runtime_is_ready(instance_id).await;
                    if should_apply {
                        (
                            self.apply_config_for_instance(
                                project_path,
                                event_tx.clone(),
                                options.verbose,
                                Some(instance_id),
                                false,
                            )
                            .await,
                            true,
                            true,
                        )
                    } else {
                        (Ok(()), true, false)
                    }
                }
                ConvergenceDecision::PreserveRuntimeUntil { .. } => {
                    if options.defer_config_reload_during_cooldown {
                        self.pending_config_reloads.lock().await.insert(instance_id);
                    }
                    (Ok(()), false, false)
                }
                ConvergenceDecision::EnsureDown => {
                    if self.project_runtime_exists(instance_id).await {
                        (self.stop_project_instance(instance_id).await, true, false)
                    } else {
                        (Ok(()), true, false)
                    }
                }
            };

            if action.is_ok() && applied_config {
                self.pending_config_reloads
                    .lock()
                    .await
                    .remove(&instance_id);
            }

            let (latest_decision, latest_durability_error) =
                match Self::sweep_availability(&mut availability).await {
                    Ok(result) => result,
                    Err(error) => {
                        return Self::surface_availability_durability(Err(error), durability_error);
                    }
                };
            Self::merge_availability_durability(&mut durability_error, latest_durability_error);
            if action
                .as_ref()
                .is_err_and(|error| error.downcast_ref::<DaemonShuttingDown>().is_some())
            {
                let Err(error) = action else {
                    unreachable!("checked error action")
                };
                return Self::surface_availability_durability(Err(error), durability_error);
            }
            if latest_decision != decision {
                match &action {
                    Ok(()) if clear_on_success => {
                        let (_, clear_durability_error) =
                            match Self::capture_availability_publication(
                                availability.clear_convergence_error().await,
                            ) {
                                Ok(result) => result,
                                Err(error) => {
                                    return Self::surface_availability_durability(
                                        Err(error),
                                        durability_error,
                                    );
                                }
                            };
                        Self::merge_availability_durability(
                            &mut durability_error,
                            clear_durability_error,
                        );
                    }
                    Err(error)
                        if error
                            .downcast_ref::<AvailabilityStartSuperseded>()
                            .is_none() =>
                    {
                        let (_, record_durability_error) =
                            match Self::capture_availability_publication(
                                availability
                                    .record_convergence_error(format!("{error:#}"))
                                    .await,
                            ) {
                                Ok(result) => result,
                                Err(record_error) => {
                                    return Self::surface_availability_durability(
                                        Err(record_error.context(format!(
                                            "failed to record availability convergence error after: {error:#}"
                                        ))),
                                        durability_error,
                                    );
                                }
                            };
                        Self::merge_availability_durability(
                            &mut durability_error,
                            record_durability_error,
                        );
                    }
                    Ok(()) | Err(_) => {}
                }
                continue;
            }

            if action.as_ref().is_err_and(|error| {
                error
                    .downcast_ref::<AvailabilityStartSuperseded>()
                    .is_some()
            }) {
                continue;
            }

            let result = self
                .finish_availability_action(&mut availability, decision, action, clear_on_success)
                .await;
            return Self::surface_availability_durability(result, durability_error);
        }
    }

    async fn finish_availability_action(
        &self,
        availability: &mut AvailabilityStore,
        decision: ConvergenceDecision,
        action: Result<()>,
        clear_on_success: bool,
    ) -> Result<ConvergenceDecision> {
        match action {
            Ok(()) => {
                if clear_on_success {
                    availability.clear_convergence_error().await?;
                }
                Ok(decision)
            }
            Err(error) => {
                let message = format!("{error:#}");
                if let Err(record_error) = availability.record_convergence_error(message).await {
                    return Err(error.context(format!(
                        "failed to record availability convergence error: {record_error}"
                    )));
                }
                Err(error)
            }
        }
    }

    fn canonicalize_path(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    async fn transition_lock_for_path(&self, path: &Path) -> (PathBuf, Arc<Mutex<()>>) {
        let canonical = Self::canonicalize_path(path);
        let transition_lock = {
            let mut locks = self.config_transition_locks.lock().await;
            locks
                .entry(canonical.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        (canonical, transition_lock)
    }

    async fn lock_service_runtime_transition(
        &self,
        name: &str,
    ) -> Option<(PathBuf, OwnedMutexGuard<()>, OwnedMutexGuard<()>)> {
        loop {
            let path = self.get_service_path(name).await?;
            let (canonical, transition_lock) = self.transition_lock_for_path(&path).await;
            let transition_guard = transition_lock.lock_owned().await;
            let runtime_projection_guard = self.runtime_projection_lock.clone().lock_owned().await;
            match self.get_service_path(name).await {
                Some(current_path) if Self::canonicalize_path(&current_path) == canonical => {
                    return Some((canonical, transition_guard, runtime_projection_guard));
                }
                Some(_) => {
                    drop(runtime_projection_guard);
                    drop(transition_guard);
                }
                None => return None,
            }
        }
    }

    pub async fn get_service_path(&self, name: &str) -> Option<PathBuf> {
        let services = self.services.lock().await;
        services.get(name).map(|s| s.path.clone())
    }

    pub async fn get_service_env(&self, name: &str) -> Result<HashMap<String, String>> {
        let Some((_path, _transition_guard, _runtime_projection_guard)) =
            self.lock_service_runtime_transition(name).await
        else {
            anyhow::bail!("Service {name} not found");
        };

        let (instance_id, config, service_config, path, port_result, sticky_port) = {
            let services = self.services.lock().await;
            let service = services
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Service {name} not found"))?;

            let port_result = match &service.runtime_state {
                ServiceRuntime::Controller(c) => Err(c.clone()),
                ServiceRuntime::None => Ok(None),
            };

            (
                service.instance_id,
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
            async move {
                manager
                    .get_service_field(&service_name, &field, instance_id)
                    .await
            }
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
                self.domain_for_service(service.instance_id, name),
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
                .iter()
                .map(|(name, service)| {
                    (
                        name.clone(),
                        service.instance_id,
                        service.controller_generation,
                        service.runtime_state.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        for (name, instance_id, controller_generation, runtime) in services {
            if let ServiceRuntime::Controller(c) = runtime {
                let metrics = {
                    let c = c.lock().await;
                    c.metrics().await
                };

                if let Ok(Some(m)) = metrics {
                    let services = self.services.lock().await;
                    let is_current_controller = services.get(&name).is_some_and(|service| {
                        service.instance_id == instance_id
                            && service.controller_generation == controller_generation
                            && matches!(service.runtime_state, ServiceRuntime::Controller(_))
                    });
                    if is_current_controller {
                        let _ = self.event_sender.send(Event::Metrics(m));
                    }
                }
            }
        }
    }
}

/// Resolve the domain for a project, applying worktree branch qualification
/// when the project is in a linked Git worktree with a domain template.
fn resolve_worktree_domain(
    base_domain: &str,
    project_name: &str,
    worktrees_config: Option<&locald_core::config::WorktreesConfig>,
    project_path: &Path,
) -> String {
    let Some(git_context) = locald_core::worktree::detect(project_path) else {
        return base_domain.to_owned();
    };

    if !git_context.is_worktree || git_context.is_default_branch {
        return base_domain.to_owned();
    }

    let Some(template) = worktrees_config.and_then(|config| config.domain.as_ref()) else {
        return base_domain.to_owned();
    };
    let Some(branch) = git_context.branch.as_ref() else {
        return base_domain.to_owned();
    };

    locald_core::worktree::resolve_domain_template(template, project_name, branch, base_domain)
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
    use crate::service::exec::ExecController;
    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use futures_util::{StreamExt, stream};
    use locald_core::config::{ExecServiceConfig, LocaldConfig, ProjectConfig, ServiceConfig};
    use locald_core::registry::Registry;
    use locald_core::service::{RuntimeState, ServiceCommand};
    use locald_core::state::PersistedProcessBirth;
    use std::collections::HashMap;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    struct ProcessGroupCleanup {
        group: i32,
        armed: bool,
    }

    impl ProcessGroupCleanup {
        fn new(pid: u32) -> Self {
            Self {
                group: i32::try_from(pid).expect("test process-group ID fits i32"),
                armed: true,
            }
        }

        fn group(&self) -> i32 {
            self.group
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for ProcessGroupCleanup {
        fn drop(&mut self) {
            if self.armed {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(-self.group),
                    Signal::SIGKILL,
                );
            }
        }
    }

    fn test_process_identity(
        start_ticks: u64,
        process_group_id: i32,
        executable: impl Into<PathBuf>,
    ) -> PersistedProcessIdentity {
        PersistedProcessIdentity {
            birth: Some(PersistedProcessBirth::Linux {
                boot_id: "test-boot".to_owned(),
                start_ticks,
            }),
            process_group_id,
            executable: Some(executable.into()),
        }
    }

    fn different_process_birth(birth: &PersistedProcessBirth) -> PersistedProcessBirth {
        match birth {
            PersistedProcessBirth::Macos {
                start_seconds,
                start_microseconds,
            } => PersistedProcessBirth::Macos {
                start_seconds: *start_seconds,
                start_microseconds: start_microseconds.saturating_add(1),
            },
            PersistedProcessBirth::Linux {
                boot_id,
                start_ticks,
            } => PersistedProcessBirth::Linux {
                boot_id: boot_id.clone(),
                start_ticks: start_ticks.saturating_add(1),
            },
        }
    }

    #[derive(Debug)]
    struct TestController {
        id: String,
        state: RuntimeState,
        fail_start: bool,
        fail_stop: bool,
        owned_process_id: Option<u32>,
        process_identity: Option<PersistedProcessIdentity>,
        container_id: Option<String>,
    }

    impl TestController {
        fn new(id: impl Into<String>, state: RuntimeState) -> Self {
            Self {
                id: id.into(),
                state,
                fail_start: false,
                fail_stop: false,
                owned_process_id: None,
                process_identity: None,
                container_id: None,
            }
        }

        fn failing_stop(id: impl Into<String>, state: RuntimeState) -> Self {
            Self {
                id: id.into(),
                state,
                fail_start: false,
                fail_stop: true,
                owned_process_id: None,
                process_identity: None,
                container_id: None,
            }
        }

        fn retained_start_failure(id: impl Into<String>, pid: u32) -> Self {
            Self {
                id: id.into(),
                state: RuntimeState {
                    pid: None,
                    port: None,
                    status: ServiceState::Stopped,
                    health_status: HealthStatus::Unknown,
                },
                fail_start: true,
                fail_stop: false,
                owned_process_id: Some(pid),
                process_identity: None,
                container_id: None,
            }
        }

        fn retained_pidless_start_failure(
            id: impl Into<String>,
            container_id: impl Into<String>,
        ) -> Self {
            Self {
                id: id.into(),
                state: RuntimeState {
                    pid: None,
                    port: None,
                    status: ServiceState::Stopped,
                    health_status: HealthStatus::Unknown,
                },
                fail_start: true,
                fail_stop: false,
                owned_process_id: None,
                process_identity: None,
                container_id: Some(container_id.into()),
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
            if self.fail_start {
                anyhow::bail!("injected retained start failure");
            }
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            if self.fail_stop {
                anyhow::bail!("injected stop failure");
            }
            self.owned_process_id = None;
            Ok(())
        }

        async fn read_state(&self) -> RuntimeState {
            self.state
        }

        fn owned_process_id(&self) -> Option<u32> {
            self.owned_process_id
        }

        fn process_identity(&self) -> Option<PersistedProcessIdentity> {
            self.process_identity.clone()
        }

        async fn logs(&self) -> futures::stream::BoxStream<'static, LogEntry> {
            stream::empty().boxed()
        }

        fn get_metadata(&self, key: &str) -> Option<String> {
            (key == "container_id")
                .then(|| self.container_id.clone())
                .flatten()
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

    #[derive(Debug)]
    struct ScriptedController {
        id: String,
        state: RuntimeState,
        process_identity: Option<PersistedProcessIdentity>,
        start_entered: Option<Arc<tokio::sync::Notify>>,
        fail_prepare: bool,
        stop_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ServiceController for ScriptedController {
        fn id(&self) -> &str {
            &self.id
        }

        async fn prepare(&mut self) -> Result<()> {
            if self.fail_prepare {
                anyhow::bail!("injected prepare failure");
            }
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            if let Some(entered) = &self.start_entered {
                entered.notify_one();
                self.state.status = ServiceState::Running;
                return Ok(());
            }
            self.state.status = ServiceState::Running;
            self.state.health_status = HealthStatus::Healthy;
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.stop_count.fetch_add(1, Ordering::SeqCst);
            self.state.status = ServiceState::Stopped;
            self.state.health_status = HealthStatus::Unknown;
            Ok(())
        }

        async fn read_state(&self) -> RuntimeState {
            self.state.clone()
        }

        fn owned_process_id(&self) -> Option<u32> {
            self.process_identity.as_ref().and(self.state.pid)
        }

        fn process_identity(&self) -> Option<PersistedProcessIdentity> {
            self.process_identity.clone()
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

    #[derive(Debug)]
    struct UnreadyStartFactory {
        entered: Arc<tokio::sync::Notify>,
        stop_count: Arc<AtomicUsize>,
    }

    impl ServiceFactory for UnreadyStartFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            _ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            Arc::new(Mutex::new(ScriptedController {
                id: name,
                state: RuntimeState {
                    pid: Some(41),
                    port: None,
                    status: ServiceState::Building,
                    health_status: HealthStatus::Unknown,
                },
                process_identity: Some(test_process_identity(1_234, 41, "/test/unready-worker")),
                start_entered: Some(self.entered.clone()),
                fail_prepare: false,
                stop_count: self.stop_count.clone(),
            }))
        }
    }

    #[derive(Debug)]
    struct RetryPrepareFactory {
        failure_consumed: Arc<AtomicBool>,
        stop_count: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct RetainedStartFailureFactory {
        pid: u32,
    }

    #[derive(Debug)]
    struct RetainedPidlessStartFailureFactory {
        container_id: String,
    }

    #[derive(Debug)]
    struct RetainedStartRetryFactory {
        create_count: Arc<AtomicUsize>,
        start_count: Arc<AtomicUsize>,
        stop_count: Arc<AtomicUsize>,
        state_path_failure_fixture: Option<PathBuf>,
    }

    #[derive(Debug)]
    struct RetainedStartRetryController {
        id: String,
        state: RuntimeState,
        fail_start: bool,
        owned_process_id: Option<u32>,
        process_identity: Option<PersistedProcessIdentity>,
        start_count: Arc<AtomicUsize>,
        stop_count: Arc<AtomicUsize>,
        state_path_failure_fixture: Option<PathBuf>,
    }

    #[async_trait]
    impl ServiceController for RetainedStartRetryController {
        fn id(&self) -> &str {
            &self.id
        }

        async fn prepare(&mut self) -> Result<()> {
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            self.start_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_start {
                if let Some(state_path) = &self.state_path_failure_fixture {
                    std::fs::remove_file(state_path).with_context(|| {
                        format!(
                            "remove runtime state before injecting persistence failure at {}",
                            state_path.display()
                        )
                    })?;
                    std::fs::create_dir(state_path).with_context(|| {
                        format!(
                            "inject runtime state persistence blocker at {}",
                            state_path.display()
                        )
                    })?;
                }
                anyhow::bail!("injected retained start failure");
            }
            self.state.status = ServiceState::Running;
            self.state.health_status = HealthStatus::Healthy;
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.stop_count.fetch_add(1, Ordering::SeqCst);
            if let Some(state_path) = self.state_path_failure_fixture.take() {
                std::fs::remove_dir_all(&state_path).with_context(|| {
                    format!(
                        "remove injected state persistence blocker at {}",
                        state_path.display()
                    )
                })?;
            }
            self.state.pid = None;
            self.state.status = ServiceState::Stopped;
            self.state.health_status = HealthStatus::Unknown;
            self.owned_process_id = None;
            self.process_identity = None;
            Ok(())
        }

        async fn read_state(&self) -> RuntimeState {
            self.state.clone()
        }

        fn owned_process_id(&self) -> Option<u32> {
            self.owned_process_id
        }

        fn process_identity(&self) -> Option<PersistedProcessIdentity> {
            self.process_identity.clone()
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

    impl ServiceFactory for RetainedStartFailureFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            _ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            Arc::new(Mutex::new(TestController::retained_start_failure(
                name, self.pid,
            )))
        }
    }

    impl ServiceFactory for RetainedPidlessStartFailureFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            _ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            Arc::new(Mutex::new(TestController::retained_pidless_start_failure(
                name,
                self.container_id.clone(),
            )))
        }
    }

    impl ServiceFactory for RetainedStartRetryFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            _ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            let creation = self.create_count.fetch_add(1, Ordering::SeqCst);
            let pid = u32::try_from(42 + creation).expect("test creation count fits in a PID");
            let fail_start = creation == 0;
            Arc::new(Mutex::new(RetainedStartRetryController {
                id: name,
                state: RuntimeState {
                    pid: Some(pid),
                    port: None,
                    status: if fail_start {
                        ServiceState::Running
                    } else {
                        ServiceState::Stopped
                    },
                    health_status: HealthStatus::Unknown,
                },
                fail_start,
                owned_process_id: Some(pid),
                process_identity: (!fail_start).then(|| {
                    test_process_identity(
                        1_234 + u64::try_from(creation).expect("test creation count fits in u64"),
                        i32::try_from(pid).expect("test PID fits in i32"),
                        "/test/retry-controller",
                    )
                }),
                start_count: self.start_count.clone(),
                stop_count: self.stop_count.clone(),
                state_path_failure_fixture: self.state_path_failure_fixture.clone(),
            }))
        }
    }

    impl ServiceFactory for RetryPrepareFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            _ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            let fail_prepare =
                name.ends_with(":second") && !self.failure_consumed.swap(true, Ordering::SeqCst);
            Arc::new(Mutex::new(ScriptedController {
                id: name,
                state: RuntimeState {
                    pid: Some(42),
                    port: None,
                    status: ServiceState::Building,
                    health_status: HealthStatus::Unknown,
                },
                process_identity: Some(test_process_identity(
                    1_234,
                    42,
                    "/test/retry-prepare-worker",
                )),
                start_entered: None,
                fail_prepare,
                stop_count: self.stop_count.clone(),
            }))
        }
    }

    #[derive(Debug)]
    struct RetryPrepareWithoutPidFactory {
        failure_consumed: Arc<AtomicBool>,
        stop_count: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct CountingStartFactory {
        creates: Arc<AtomicUsize>,
    }

    impl ServiceFactory for CountingStartFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            _ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            Arc::new(Mutex::new(ScriptedController {
                id: name,
                state: RuntimeState {
                    pid: None,
                    port: None,
                    status: ServiceState::Building,
                    health_status: HealthStatus::Unknown,
                },
                process_identity: None,
                start_entered: None,
                fail_prepare: false,
                stop_count: Arc::new(AtomicUsize::new(0)),
            }))
        }
    }

    impl ServiceFactory for RetryPrepareWithoutPidFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(config, ServiceConfig::Typed(TypedServiceConfig::Worker(_)))
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            _ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            let fail_prepare =
                name.ends_with(":second") && !self.failure_consumed.swap(true, Ordering::SeqCst);
            Arc::new(Mutex::new(ScriptedController {
                id: name,
                state: RuntimeState {
                    pid: None,
                    port: None,
                    status: ServiceState::Building,
                    health_status: HealthStatus::Unknown,
                },
                process_identity: None,
                start_entered: None,
                fail_prepare,
                stop_count: self.stop_count.clone(),
            }))
        }
    }

    #[derive(Debug)]
    struct BlockingFailStopController {
        id: String,
        state: RuntimeState,
        stop_attempts: Arc<std::sync::atomic::AtomicUsize>,
        first_stop_entered: Arc<tokio::sync::Notify>,
        release_first_stop: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl ServiceController for BlockingFailStopController {
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
            let attempt = self.stop_attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                self.first_stop_entered.notify_one();
                self.release_first_stop.notified().await;
            }
            anyhow::bail!("injected stop failure")
        }

        async fn read_state(&self) -> RuntimeState {
            self.state.clone()
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

    #[derive(Debug)]
    struct BlockingMetricsController {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        state: RuntimeState,
    }

    #[async_trait]
    impl ServiceController for BlockingMetricsController {
        fn id(&self) -> &str {
            "blocking-metrics"
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
            self.state.clone()
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
            self.entered.notify_one();
            self.release.notified().await;
            Ok(Some(locald_core::ipc::ServiceMetrics {
                name: "app:web".to_owned(),
                cpu_percent: 1.0,
                memory_bytes: 1,
                timestamp: 1,
            }))
        }
    }

    #[derive(Debug)]
    struct BlockingStatusController {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        state: RuntimeState,
    }

    #[async_trait]
    impl ServiceController for BlockingStatusController {
        fn id(&self) -> &str {
            "blocking-status"
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
            self.entered.notify_one();
            self.release.notified().await;
            self.state.clone()
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

    fn expected_hosts(service_domains: &[&str]) -> Vec<String> {
        let mut domains = vec![
            "dev.docs.local".to_owned(),
            "dev.locald.local".to_owned(),
            "docs.local".to_owned(),
            "locald.local".to_owned(),
        ];
        domains.extend(service_domains.iter().map(|domain| (*domain).to_owned()));
        domains.sort();
        domains.dedup();
        domains
    }

    fn test_instance_id() -> ProjectInstanceId {
        "00000000-0000-4000-8000-000000000001"
            .parse()
            .expect("valid project instance ID")
    }

    fn alternate_test_instance_id() -> ProjectInstanceId {
        "00000000-0000-4000-8000-000000000002"
            .parse()
            .expect("valid alternate project instance ID")
    }

    async fn availability_manager(
        root: &Path,
        project_path: &Path,
        project_name: &str,
    ) -> (ProcessManager, ProjectInstanceId, PathBuf) {
        std::fs::create_dir_all(project_path).expect("create availability project");
        let canonical = std::fs::canonicalize(project_path).expect("canonical availability path");
        let mut catalog = Registry::with_path(root.join("catalog.json"));
        let discovery = Registry::discover(canonical.clone())
            .await
            .expect("discover availability project");
        let instance_id = catalog
            .register_project(discovery, Some(project_name.to_owned()))
            .expect("register availability project");
        catalog.save().await.expect("save availability catalog");

        let availability_data_dir = root.join("availability-data");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            root.join("notify.sock"),
            Arc::new(StateManager::with_path(root.join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                root.join("attachments.json"),
            ))),
            None,
            availability_data_dir.clone(),
        )
        .expect("create availability process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        (manager, instance_id, availability_data_dir)
    }

    fn availability_test_service(
        instance_id: ProjectInstanceId,
        project_name: &str,
        project_path: &Path,
        fail_stop: bool,
    ) -> Service {
        let service_name = format!("{project_name}:web");
        let state = RuntimeState {
            pid: Some(42),
            port: Some(3000),
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        let controller: Arc<Mutex<dyn ServiceController>> = if fail_stop {
            Arc::new(Mutex::new(TestController::failing_stop(
                service_name,
                state,
            )))
        } else {
            Arc::new(Mutex::new(TestController::new(service_name, state)))
        };
        let mut service = test_service(
            test_config_with_domain(project_name, &format!("{project_name}.localhost")),
            ServiceConfig::Legacy(ExecServiceConfig::default()),
            ServiceRuntime::Controller(controller),
            std::fs::canonicalize(project_path).expect("canonical availability service path"),
        );
        service.instance_id = instance_id;
        service
    }

    fn write_availability_worker_config(
        project_path: &Path,
        project_name: &str,
        domain: &str,
        service_names: &[&str],
    ) {
        let mut config = format!("[project]\nname = \"{project_name}\"\ndomain = \"{domain}\"\n");
        for service_name in service_names {
            config.push_str(&format!(
                "\n[services.{service_name}]\ntype = \"worker\"\ncommand = \"unused-by-test-factory\"\n"
            ));
        }
        std::fs::write(project_path.join("locald.toml"), config)
            .expect("write availability worker config");
    }

    #[tokio::test]
    async fn availability_convergence_cooldown_preserves_stopped_runtime() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("cooldown-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "cooldown").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load project availability");
        let manual = DemandKey::manual_cli();
        availability
            .ensure_demand(manual.clone())
            .await
            .expect("acquire manual demand");
        availability
            .release_demand(&manual)
            .await
            .expect("release manual demand");

        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("converge cooldown project"),
            Some(ConvergenceDecision::PreserveRuntimeUntil {
                deadline: availability
                    .snapshot()
                    .await
                    .expect("reload cooldown")
                    .shutdown_cooldown_until()
                    .expect("cooldown deadline"),
            })
        );
        assert!(manager.services.lock().await.is_empty());
        assert!(
            manager
                .watchers
                .lock()
                .await
                .contains_key(&ProcessManager::canonicalize_path(&project_path))
        );
    }

    #[tokio::test]
    async fn availability_convergence_pause_stops_and_preserves_always_on() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("paused-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "paused").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load project availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable Always On");
        manager.services.lock().await.insert(
            "paused:web".to_owned(),
            availability_test_service(instance_id, "paused", &project_path, false),
        );

        assert!(
            manager
                .project_pause_availability(&project_path)
                .await
                .expect("pause project")
        );
        assert!(manager.get_service_controller("paused:web").await.is_none());
        let snapshot = availability.snapshot().await.expect("reload paused policy");
        assert!(snapshot.always_on());
        assert!(snapshot.is_paused());
        assert_eq!(snapshot.last_convergence_error(), None);
    }

    #[tokio::test]
    async fn availability_convergence_failure_preserves_intent_until_retry() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("retry-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "retry").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load project availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable Always On");
        let canonical = std::fs::canonicalize(&project_path).expect("canonical retry path");
        manager.services.lock().await.insert(
            "retry:web".to_owned(),
            availability_test_service(instance_id, "retry", &project_path, true),
        );

        let error = manager
            .project_pause_availability(&project_path)
            .await
            .expect_err("injected stop failure must surface");
        assert!(format!("{error:#}").contains("injected stop failure"));
        let failed = availability
            .snapshot()
            .await
            .expect("reload failed convergence");
        assert!(failed.is_paused());
        assert!(failed.always_on());
        assert!(
            failed
                .last_convergence_error()
                .is_some_and(|message| message.contains("injected stop failure"))
        );

        let replacement = Arc::new(Mutex::new(TestController::new(
            "retry:web",
            RuntimeState {
                pid: Some(43),
                port: Some(3000),
                status: ServiceState::Running,
                health_status: HealthStatus::Healthy,
            },
        )));
        manager
            .services
            .lock()
            .await
            .get_mut("retry:web")
            .expect("retry service remains registered")
            .runtime_state = ServiceRuntime::Controller(replacement);

        assert_eq!(
            manager
                .converge_project_availability(&canonical)
                .await
                .expect("retry convergence"),
            Some(ConvergenceDecision::EnsureDown)
        );
        assert_eq!(
            availability
                .snapshot()
                .await
                .expect("reload successful retry")
                .last_convergence_error(),
            None
        );
        assert!(manager.get_service_controller("retry:web").await.is_none());
    }

    #[tokio::test]
    async fn availability_convergence_renewed_always_on_crosses_pause() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("resume-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "resume").await;
        write_availability_worker_config(&project_path, "resume", "resume.localhost", &["web"]);
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load project availability");
        assert!(
            manager
                .project_set_always_on(&project_path, true)
                .await
                .expect("enable Always On")
        );
        let first_controller = manager
            .get_service_controller("resume:web")
            .await
            .expect("initial Always On runtime");
        assert!(
            manager
                .project_pause_availability(&project_path)
                .await
                .expect("pause project")
        );
        assert!(manager.get_service_controller("resume:web").await.is_none());
        let generation = availability
            .snapshot()
            .await
            .expect("load paused generation")
            .activity_generation();

        assert!(
            manager
                .project_set_always_on(&project_path, true)
                .await
                .expect("renew Always On")
        );
        let snapshot = availability
            .snapshot()
            .await
            .expect("reload resumed policy");
        assert_eq!(snapshot.activity_generation(), generation + 1);
        assert!(!snapshot.is_paused());
        assert!(snapshot.always_on());
        let resumed_controller = manager
            .get_service_controller("resume:web")
            .await
            .expect("resumed Always On runtime");
        assert!(!Arc::ptr_eq(&first_controller, &resumed_controller));

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up resumed runtime");
    }

    #[tokio::test]
    async fn availability_convergence_managed_start_cannot_bypass_pause() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("start-paused-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "start-paused").await;
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "start-paused"
domain = "start-paused.localhost"

[services.web]
type = "worker"
command = "sleep 30"
"#,
        )
        .expect("write paused project config");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load project availability");
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("acquire manual demand");
        availability.pause_project().await.expect("pause project");

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("paused start converges down");

        assert!(manager.services.lock().await.is_empty());
        assert!(
            manager
                .watchers
                .lock()
                .await
                .contains_key(&ProcessManager::canonicalize_path(&project_path))
        );
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("start-paused.localhost")
                .is_none()
        );
    }

    #[tokio::test]
    async fn availability_convergence_first_pause_cancels_in_flight_legacy_start() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("pause-race-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "pause-race").await;
        write_availability_worker_config(
            &project_path,
            "pause-race",
            "pause-race.localhost",
            &["web"],
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(UnreadyStartFactory {
                entered: entered.clone(),
                stop_count: stop_count.clone(),
            }),
        );

        let start_manager = manager.clone();
        let start_path = project_path.clone();
        let start = tokio::spawn(async move { start_manager.start(start_path, None, false).await });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("startup reaches blocking controller");

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            manager.project_pause_availability(&project_path),
        )
        .await
        .expect("pause cancels startup promptly")
        .expect("pause convergence succeeds");
        tokio::time::timeout(std::time::Duration::from_secs(1), start)
            .await
            .expect("cancelled start returns promptly")
            .expect("start task joins")
            .expect("superseded start converges to pause");

        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("pause-race:web")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn started_service_ownership_is_durable_while_readiness_is_blocked() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("durable-start-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "durable-start").await;
        write_availability_worker_config(
            &project_path,
            "durable-start",
            "durable-start.localhost",
            &["web"],
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(UnreadyStartFactory {
                entered: entered.clone(),
                stop_count: stop_count.clone(),
            }),
        );

        let start = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.start(project_path, None, false).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("service starts and enters its readiness wait");

        let expected_identity = test_process_identity(1_234, 41, "/test/unready-worker");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let persisted = manager
                    .state_manager
                    .load()
                    .await
                    .expect("load runtime state during readiness");
                if persisted.services.iter().any(|service| {
                    service.name == "durable-start:web"
                        && service.status == ServiceState::Running
                        && service.pid == Some(41)
                        && service.process_identity.as_ref() == Some(&expected_identity)
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("spawn ownership becomes durable before readiness completes");
        assert!(
            !start.is_finished(),
            "startup remains blocked on readiness after ownership publication"
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("pause cancels the blocked startup");
        start
            .await
            .expect("join cancelled startup")
            .expect("blocked startup converges to pause");
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_spawn_ownership_publication_stops_the_started_service() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("failed-publication-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "failed-publication").await;
        write_availability_worker_config(
            &project_path,
            "failed-publication",
            "failed-publication.localhost",
            &["web"],
        );
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: stop_count.clone(),
            }),
        );
        std::fs::create_dir(dir.path().join("state.json"))
            .expect("make the runtime-state destination unwritable as a file");

        let error = manager
            .start(project_path, None, false)
            .await
            .expect_err("ownership publication failure must fail startup");
        assert!(format!("{error:#}").contains("failed to persist ownership"));
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("failed-publication:web")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn availability_convergence_pause_is_not_blocked_by_unrelated_startup() {
        let dir = tempdir().expect("create temporary directory");
        let first_path = dir.path().join("pause-now-project");
        let second_path = dir.path().join("slow-start-project");
        let (mut manager, first_id, availability_data_dir) =
            availability_manager(dir.path(), &first_path, "pause-now").await;
        std::fs::create_dir_all(&second_path).expect("create slow-start project");
        write_availability_worker_config(
            &second_path,
            "slow-start",
            "slow-start.localhost",
            &["web"],
        );
        let second_discovery = Registry::discover(
            std::fs::canonicalize(&second_path).expect("canonical slow-start path"),
        )
        .await
        .expect("discover slow-start project");
        let second_id = manager
            .registry
            .lock()
            .await
            .register_project(second_discovery, Some("slow-start".to_owned()))
            .expect("register slow-start project");

        let mut first_availability = AvailabilityStore::load(&availability_data_dir, first_id)
            .await
            .expect("load first availability");
        first_availability
            .set_always_on(true)
            .await
            .expect("enable first Always On");
        let mut second_availability = AvailabilityStore::load(&availability_data_dir, second_id)
            .await
            .expect("load second availability");
        second_availability
            .set_always_on(true)
            .await
            .expect("enable second Always On");
        manager.services.lock().await.insert(
            "pause-now:web".to_owned(),
            availability_test_service(first_id, "pause-now", &first_path, false),
        );

        let start_entered = Arc::new(tokio::sync::Notify::new());
        manager.factories.insert(
            0,
            Arc::new(UnreadyStartFactory {
                entered: start_entered.clone(),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let slow_start = tokio::spawn({
            let manager = manager.clone();
            let second_path = second_path.clone();
            async move { manager.converge_project_availability(&second_path).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), start_entered.notified())
            .await
            .expect("unrelated startup reaches readiness wait");

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            manager.project_pause_availability(&first_path),
        )
        .await
        .expect("pause is independent of unrelated readiness")
        .expect("pause first project");
        assert!(
            manager
                .get_service_controller("pause-now:web")
                .await
                .is_none()
        );

        second_availability
            .pause_project()
            .await
            .expect("cancel slow startup");
        tokio::time::timeout(std::time::Duration::from_secs(2), slow_start)
            .await
            .expect("slow startup observes pause")
            .expect("join slow startup")
            .expect("converge slow startup after pause");
    }

    #[tokio::test]
    async fn availability_convergence_project_stop_batches_before_global_persistence() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("batched-stop-project");
        let other_path = dir.path().join("unrelated-project");
        let (manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "batched-stop").await;
        std::fs::create_dir_all(&other_path).expect("create unrelated project");

        let mut web = availability_test_service(instance_id, "batched-stop", &project_path, false);
        let mut api = availability_test_service(instance_id, "batched-stop", &project_path, false);
        api.service_config = ServiceConfig::Legacy(ExecServiceConfig {
            command: Some("api".to_owned()),
            ..Default::default()
        });
        let unrelated = availability_test_service(
            alternate_test_instance_id(),
            "unrelated",
            &other_path,
            false,
        );
        web.projection_generation = 1;
        api.projection_generation = 1;
        let unrelated_controller = match &unrelated.runtime_state {
            ServiceRuntime::Controller(controller) => controller.clone(),
            ServiceRuntime::None => unreachable!("test service has a controller"),
        };
        {
            let mut services = manager.services.lock().await;
            services.insert("batched-stop:web".to_owned(), web);
            services.insert("batched-stop:api".to_owned(), api);
            services.insert("unrelated:web".to_owned(), unrelated);
        }

        let unrelated_guard = unrelated_controller.lock().await;
        let pause_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_pause_availability(&project_path).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let both_stopped = {
                    let services = manager.services.lock().await;
                    ["batched-stop:web", "batched-stop:api"]
                        .into_iter()
                        .all(|name| {
                            services.get(name).is_some_and(|service| {
                                matches!(service.runtime_state, ServiceRuntime::None)
                            })
                        })
                };
                if both_stopped {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all selected controllers stop before persistence can read unrelated state");
        assert!(!pause_task.is_finished());

        drop(unrelated_guard);
        pause_task
            .await
            .expect("join batched pause")
            .expect("finish batched pause");
    }

    #[tokio::test]
    async fn availability_convergence_move_stops_stale_path_by_instance() {
        let dir = tempdir().expect("create temporary directory");
        let old_path = dir.path().join("old-location");
        let new_path = dir.path().join("new-location");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &old_path, "moved").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load moved availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable moved Always On");
        manager.services.lock().await.insert(
            "moved:web".to_owned(),
            availability_test_service(instance_id, "moved", &old_path, false),
        );

        std::fs::rename(&old_path, &new_path).expect("move project directory");
        let canonical_new = std::fs::canonicalize(&new_path).expect("canonical moved path");
        {
            let mut registry = manager.registry.lock().await;
            let record = registry
                .instances
                .get_mut(&instance_id)
                .expect("moved instance record");
            record.current_path = Some(canonical_new.clone());
            record.last_known_path = canonical_new.clone();
            record.presence = CatalogPresence::Active;
            registry
                .legacy_paths
                .retain(|_, candidate| *candidate != instance_id);
            registry.legacy_paths.insert(canonical_new, instance_id);
        }

        manager
            .project_pause_availability(&new_path)
            .await
            .expect("pause moved project");
        assert!(manager.get_service_controller("moved:web").await.is_none());
        assert!(
            availability
                .snapshot()
                .await
                .expect("reload moved availability")
                .is_paused()
        );
    }

    #[tokio::test]
    async fn moved_instance_config_reload_restarts_stale_path_runtime_by_identity() {
        let dir = tempdir().expect("create temporary directory");
        let old_path = dir.path().join("reload-old-location");
        let new_path = dir.path().join("reload-new-location");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &old_path, "move-reload").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager.services.lock().await.insert(
            "move-reload:web".to_owned(),
            availability_test_service(instance_id, "move-reload", &old_path, false),
        );

        std::fs::rename(&old_path, &new_path).expect("move reload project directory");
        write_availability_worker_config(
            &new_path,
            "move-reload",
            "move-reload.localhost",
            &["web"],
        );
        let canonical_new = std::fs::canonicalize(&new_path).expect("canonical moved reload path");
        {
            let mut registry = manager.registry.lock().await;
            let record = registry
                .instances
                .get_mut(&instance_id)
                .expect("moved reload instance record");
            record.current_path = Some(canonical_new.clone());
            record.last_known_path = canonical_new.clone();
            record.presence = CatalogPresence::Active;
            registry
                .legacy_paths
                .insert(canonical_new.clone(), instance_id);
        }

        manager
            .apply_config_for_instance(canonical_new.clone(), None, false, Some(instance_id), false)
            .await
            .expect("reload moved project with a restart-required change");

        let services = manager.services.lock().await;
        let service = services
            .get("move-reload:web")
            .expect("restarted moved service");
        assert_eq!(service.instance_id, instance_id);
        assert_eq!(service.path, canonical_new);
        assert!(matches!(
            service.runtime_state,
            ServiceRuntime::Controller(_)
        ));
    }

    #[tokio::test]
    async fn availability_convergence_remove_moved_instance_stops_stale_path_runtime() {
        let dir = tempdir().expect("create temporary directory");
        let old_path = dir.path().join("remove-old-location");
        let new_path = dir.path().join("remove-new-location");
        let (manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &old_path, "remove-moved").await;
        manager.services.lock().await.insert(
            "remove-moved:web".to_owned(),
            availability_test_service(instance_id, "remove-moved", &old_path, false),
        );
        manager.watch_config(old_path.clone()).await;
        let canonical_old = ProcessManager::canonicalize_path(&old_path);
        assert!(manager.watchers.lock().await.contains_key(&canonical_old));

        std::fs::rename(&old_path, &new_path).expect("move removable project directory");
        let canonical_new =
            std::fs::canonicalize(&new_path).expect("canonical removable moved path");
        {
            let mut registry = manager.registry.lock().await;
            let record = registry
                .instances
                .get_mut(&instance_id)
                .expect("removable moved instance record");
            record.current_path = Some(canonical_new.clone());
            record.last_known_path = canonical_new.clone();
            registry.legacy_paths.insert(canonical_new, instance_id);
        }

        manager
            .remove_project(&new_path)
            .await
            .expect("remove moved project");

        assert!(
            manager
                .get_service_controller("remove-moved:web")
                .await
                .is_none()
        );
        assert!(
            !manager
                .registry
                .lock()
                .await
                .instances
                .contains_key(&instance_id)
        );
        assert!(!manager.watchers.lock().await.contains_key(&canonical_old));
        assert!(
            manager
                .forgotten_reload_paths
                .lock()
                .await
                .contains(&canonical_old)
        );
    }

    #[tokio::test]
    async fn availability_convergence_remove_moved_instance_preserves_reused_path_watcher() {
        let dir = tempdir().expect("create temporary directory");
        let reused_path = dir.path().join("shared-location");
        let moved_path = dir.path().join("moved-location");
        let (mut manager, moved_id, _availability_data_dir) =
            availability_manager(dir.path(), &reused_path, "moved-owner").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager.services.lock().await.insert(
            "moved-owner:web".to_owned(),
            availability_test_service(moved_id, "moved-owner", &reused_path, false),
        );

        std::fs::rename(&reused_path, &moved_path).expect("move first project");
        let canonical_moved = std::fs::canonicalize(&moved_path).expect("canonical moved path");
        std::fs::create_dir_all(&reused_path).expect("recreate reused path");
        write_availability_worker_config(
            &reused_path,
            "replacement",
            "replacement.localhost",
            &["web"],
        );
        let replacement_discovery = Registry::discover(
            std::fs::canonicalize(&reused_path).expect("canonical replacement path"),
        )
        .await
        .expect("discover replacement project");
        let replacement_id = {
            let mut registry = manager.registry.lock().await;
            let moved = registry
                .instances
                .get_mut(&moved_id)
                .expect("moved owner record");
            moved.current_path = Some(canonical_moved.clone());
            moved.last_known_path = canonical_moved.clone();
            moved.presence = CatalogPresence::Active;
            registry
                .legacy_paths
                .retain(|_, candidate| *candidate != moved_id);
            registry
                .legacy_paths
                .insert(canonical_moved.clone(), moved_id);
            registry
                .register_project(replacement_discovery, Some("replacement".to_owned()))
                .expect("register replacement project")
        };
        assert_ne!(replacement_id, moved_id);
        manager.watch_config(reused_path.clone()).await;
        let canonical_reused = ProcessManager::canonicalize_path(&reused_path);
        assert!(
            manager
                .watchers
                .lock()
                .await
                .contains_key(&canonical_reused)
        );

        manager
            .remove_project(&moved_path)
            .await
            .expect("remove moved owner");
        assert!(
            manager
                .watchers
                .lock()
                .await
                .contains_key(&canonical_reused)
        );
        assert!(
            !manager
                .forgotten_reload_paths
                .lock()
                .await
                .contains(&canonical_reused)
        );

        manager
            .reload_config(reused_path.clone())
            .await
            .expect("reload replacement project");
        assert!(
            manager
                .get_service_controller("replacement:web")
                .await
                .is_some()
        );
        assert!(
            manager
                .registry
                .lock()
                .await
                .instances
                .contains_key(&replacement_id)
        );
    }

    #[tokio::test]
    async fn availability_convergence_stale_start_cannot_reregister_removed_instance() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("stale-start-project");
        let (manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "stale-start").await;

        manager
            .remove_project(&project_path)
            .await
            .expect("remove project before stale start");
        let error = manager
            .start_catalogued_instance(instance_id, project_path, None, false)
            .await
            .expect_err("captured start cannot restore removed identity");

        assert!(format!("{error:#}").contains("no active path"));
        assert!(
            !manager
                .registry
                .lock()
                .await
                .instances
                .contains_key(&instance_id)
        );
    }

    #[tokio::test]
    async fn availability_convergence_removed_watcher_cannot_resurrect_project() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("forgotten-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "forgotten").await;
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "forgotten"
domain = "forgotten.localhost"

[services.web]
type = "worker"
command = "ignored"
"#,
        )
        .expect("write forgotten project config");
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load forgotten availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable forgotten Always On");
        manager
            .converge_project_availability(&project_path)
            .await
            .expect("start forgotten project");
        let canonical = ProcessManager::canonicalize_path(&project_path);
        assert!(manager.watchers.lock().await.contains_key(&canonical));

        manager
            .remove_project(&project_path)
            .await
            .expect("remove forgotten project");
        assert!(!manager.watchers.lock().await.contains_key(&canonical));

        manager
            .reload_config(project_path.clone())
            .await
            .expect("ignore queued reload after removal");
        assert!(manager.services.lock().await.is_empty());
        assert!(manager.registry.lock().await.instances.is_empty());
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("forgotten.localhost")
                .is_none()
        );

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("explicit start re-registers forgotten path");
        let (replacement_id, _) = manager
            .availability_instance_for_path(&project_path)
            .await
            .expect("resolve replacement instance");
        assert_ne!(replacement_id, instance_id);
        assert!(manager.watchers.lock().await.contains_key(&canonical));
        assert!(
            manager
                .get_service_controller("forgotten:web")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn availability_convergence_missing_instance_cannot_stop_reused_path() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reused-location");
        let (manager, missing_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "missing").await;
        let replacement_id = alternate_test_instance_id();
        let mut availability = AvailabilityStore::load(&availability_data_dir, missing_id)
            .await
            .expect("load missing availability");
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("seed missing demand");
        availability
            .pause_project()
            .await
            .expect("pause missing instance");

        {
            let mut registry = manager.registry.lock().await;
            let original = registry
                .instances
                .get(&missing_id)
                .expect("missing instance record")
                .clone();
            let missing = registry
                .instances
                .get_mut(&missing_id)
                .expect("mutable missing instance record");
            missing.current_path = None;
            missing.presence = CatalogPresence::Missing;

            let mut replacement = original;
            replacement.id = replacement_id;
            replacement.current_path = Some(replacement.last_known_path.clone());
            replacement.presence = CatalogPresence::Active;
            registry.instances.insert(replacement_id, replacement);
            registry
                .legacy_paths
                .insert(project_path.clone(), replacement_id);
        }
        manager.services.lock().await.insert(
            "replacement:web".to_owned(),
            availability_test_service(replacement_id, "replacement", &project_path, false),
        );
        write_availability_worker_config(
            &project_path,
            "replacement",
            "replacement.localhost",
            &["web"],
        );

        let error = manager
            .apply_config_for_instance(project_path.clone(), None, false, Some(missing_id), false)
            .await
            .expect_err("stale convergence cannot apply config to replacement identity");
        assert!(format!("{error:#}").contains("project identity changed"));

        manager.converge_all_project_availability().await;

        assert!(
            manager
                .get_service_controller("replacement:web")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn availability_convergence_reload_defers_until_cooldown_resume() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("cooldown-reload-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "cooldown-reload").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "cooldown-reload",
            "first.localhost",
            &["web"],
        );
        manager
            .project_set_always_on(&project_path, true)
            .await
            .expect("start cooldown reload project");
        let controller = manager
            .get_service_controller("cooldown-reload:web")
            .await
            .expect("initial cooldown runtime");
        manager
            .project_set_always_on(&project_path, false)
            .await
            .expect("enter cooldown");

        write_availability_worker_config(
            &project_path,
            "cooldown-reload",
            "second.localhost",
            &["web"],
        );
        manager
            .reload_config(project_path.clone())
            .await
            .expect("record running cooldown reload");

        assert!(Arc::ptr_eq(
            &controller,
            &manager
                .get_service_controller("cooldown-reload:web")
                .await
                .expect("cooldown keeps the original runtime")
        ));
        assert!(
            manager
                .resolve_service_by_domain("first.localhost")
                .await
                .is_some()
        );

        manager
            .project_ensure_availability(&project_path, DemandKey::manual_cli())
            .await
            .expect("resume and apply pending reload");

        let reloaded = manager
            .get_service_controller("cooldown-reload:web")
            .await
            .expect("reloaded cooldown runtime");
        assert!(Arc::ptr_eq(&controller, &reloaded));
        assert!(
            manager
                .resolve_service_by_domain("first.localhost")
                .await
                .is_none()
        );
        assert!(
            manager
                .resolve_service_by_domain("second.localhost")
                .await
                .is_some()
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up cooldown runtime");
    }

    #[tokio::test]
    async fn availability_convergence_reload_rechecks_authority_before_stopping_runtime() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reload-authority-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "reload-authority").await;
        write_availability_worker_config(
            &project_path,
            "reload-authority",
            "reload-authority.localhost",
            &["web"],
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load reload authority state");
        availability
            .set_always_on(true)
            .await
            .expect("enable reload authority Always On");

        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingStatusController {
                entered: entered.clone(),
                release: release.clone(),
                state: RuntimeState {
                    pid: Some(77),
                    port: None,
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            }));
        let mut service =
            availability_test_service(instance_id, "reload-authority", &project_path, false);
        service.runtime_state = ServiceRuntime::Controller(controller.clone());
        manager
            .services
            .lock()
            .await
            .insert("reload-authority:web".to_owned(), service);

        let reload_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.reload_config(project_path).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("reload observes existing runtime");
        availability
            .set_always_on(false)
            .await
            .expect("disable Always On before destructive reload boundary");
        release.notify_one();
        reload_task
            .await
            .expect("join reload task")
            .expect("defer reload during cooldown");

        let retained = manager
            .get_service_controller("reload-authority:web")
            .await
            .expect("retain original controller");
        assert!(Arc::ptr_eq(&retained, &controller));
        assert!(
            manager
                .pending_config_reloads
                .lock()
                .await
                .contains(&instance_id)
        );
    }

    #[tokio::test]
    async fn availability_convergence_cooldown_reload_never_starts_stopped_runtime() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("stopped-cooldown-reload-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "stopped-cooldown-reload").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "stopped-cooldown-reload",
            "stopped-new.localhost",
            &["web"],
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load stopped cooldown availability");
        let manual = DemandKey::manual_cli();
        availability
            .ensure_demand(manual.clone())
            .await
            .expect("seed stopped cooldown demand");
        availability
            .release_demand(&manual)
            .await
            .expect("enter stopped cooldown");
        let stopped_controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::new(
                "stopped-cooldown-reload:web",
                RuntimeState {
                    pid: None,
                    port: None,
                    status: ServiceState::Stopped,
                    health_status: HealthStatus::Unknown,
                },
            )));
        let mut service = test_service(
            test_config_with_domain("stopped-cooldown-reload", "stopped-old.localhost"),
            ServiceConfig::Legacy(ExecServiceConfig::default()),
            ServiceRuntime::Controller(stopped_controller.clone()),
            std::fs::canonicalize(&project_path).expect("canonical stopped cooldown path"),
        );
        service.instance_id = instance_id;
        manager
            .services
            .lock()
            .await
            .insert("stopped-cooldown-reload:web".to_owned(), service);

        manager
            .reload_config(project_path.clone())
            .await
            .expect("defer stopped cooldown reload");

        let retained = manager
            .get_service_controller("stopped-cooldown-reload:web")
            .await
            .expect("retain stopped controller projection");
        assert!(Arc::ptr_eq(&stopped_controller, &retained));
        assert!(
            manager
                .resolve_service_by_domain("stopped-new.localhost")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn availability_convergence_retries_partial_start_before_clearing_error() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("partial-retry-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "partial-retry").await;
        let failure_consumed = Arc::new(AtomicBool::new(false));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: failure_consumed.clone(),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "partial-retry",
            "partial-retry.localhost",
            &["first", "second"],
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load partial retry availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable retry Always On");

        let error = manager
            .converge_project_availability(&project_path)
            .await
            .expect_err("first partial start fails");
        assert!(format!("{error:#}").contains("injected prepare failure"));
        assert!(failure_consumed.load(Ordering::SeqCst));
        assert!(
            availability
                .snapshot()
                .await
                .expect("reload partial failure")
                .last_convergence_error()
                .is_some()
        );

        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("retry partial start"),
            Some(ConvergenceDecision::EnsureUp)
        );
        assert!(manager.project_runtime_is_ready(instance_id).await);
        assert_eq!(
            availability
                .snapshot()
                .await
                .expect("reload successful retry")
                .last_convergence_error(),
            None
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up retry runtimes");
    }

    #[tokio::test]
    async fn availability_convergence_restore_retires_paused_runtime_snapshot() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("restore-paused-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "restore-paused").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load restore availability");
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("seed restore demand");
        availability
            .pause_project()
            .await
            .expect("pause restore project");
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "restore-paused:web".to_owned(),
                    config: test_config_with_domain("restore-paused", "restore-paused.localhost"),
                    path: project_path.clone(),
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist stale runtime snapshot");

        let restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile paused runtime evidence");
        manager.restore_policy_owned_projects(restore_plan).await;

        let restored_evidence = manager
            .state_manager
            .load()
            .await
            .expect("reload retired runtime snapshot");
        assert!(restored_evidence.services.is_empty());
        assert!(manager.services.lock().await.is_empty());
    }

    #[tokio::test]
    async fn shutdown_gate_rejects_a_start_already_queued_on_runtime_projection() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("shutdown-gate-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "shutdown-gate").await;
        write_availability_worker_config(
            &project_path,
            "shutdown-gate",
            "shutdown-gate.localhost",
            &["web"],
        );
        let creates = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(CountingStartFactory {
                creates: creates.clone(),
            }),
        );

        let runtime_guard = manager.runtime_projection_lock.lock().await;
        let (_, transition_lock) = manager.transition_lock_for_path(&project_path).await;
        let start_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.start(project_path, None, false).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if transition_lock.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("start queues behind the runtime projection");

        let shutdown_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !manager.is_shutting_down() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown publishes its start gate");
        drop(runtime_guard);

        let error = start_task
            .await
            .expect("join queued start")
            .expect_err("queued start is rejected after shutdown begins");
        assert!(error.downcast_ref::<DaemonShuttingDown>().is_some());
        shutdown_task
            .await
            .expect("join shutdown")
            .expect("finish shutdown");
        assert_eq!(creates.load(Ordering::SeqCst), 0);
        assert!(manager.services.lock().await.is_empty());
    }

    #[tokio::test]
    async fn shutdown_gate_rejects_a_stop_already_queued_on_runtime_projection() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("shutdown-stop-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "shutdown-stop").await;
        write_availability_worker_config(
            &project_path,
            "shutdown-stop",
            "shutdown-stop.localhost",
            &["web"],
        );
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: stop_count.clone(),
            }),
        );
        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start shutdown-stop project");
        let persisted_before = manager
            .state_manager
            .load()
            .await
            .expect("load running state before shutdown");
        assert_eq!(persisted_before.services.len(), 1);
        assert_eq!(persisted_before.services[0].status, ServiceState::Running);

        let runtime_guard = manager.runtime_projection_lock.lock().await;
        let (_, transition_lock) = manager.transition_lock_for_path(&project_path).await;
        let stop_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.stop("shutdown-stop:web").await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if transition_lock.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stop queues behind the runtime projection");

        let shutdown_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !manager.is_shutting_down() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown publishes its lifecycle gate");
        drop(runtime_guard);

        let error = stop_task
            .await
            .expect("join queued stop")
            .expect_err("queued stop is rejected after shutdown begins");
        assert!(error.downcast_ref::<DaemonShuttingDown>().is_some());
        shutdown_task
            .await
            .expect("join shutdown")
            .expect("finish shutdown");

        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        let persisted_after = manager
            .state_manager
            .load()
            .await
            .expect("reload state after shutdown");
        assert_eq!(persisted_after.services.len(), 1);
        assert_eq!(persisted_after.services[0].status, ServiceState::Running);
    }

    #[tokio::test]
    async fn shutdown_drains_inflight_availability_stop_before_teardown() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("shutdown-convergence-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "shutdown-convergence").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load shutdown-convergence availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable Always On before pause");

        let stop_attempts = Arc::new(AtomicUsize::new(0));
        let first_stop_entered = Arc::new(tokio::sync::Notify::new());
        let release_first_stop = Arc::new(tokio::sync::Notify::new());
        let mut service =
            availability_test_service(instance_id, "shutdown-convergence", &project_path, false);
        service.runtime_state =
            ServiceRuntime::Controller(Arc::new(Mutex::new(BlockingFailStopController {
                id: "shutdown-convergence:web".to_owned(),
                state: RuntimeState {
                    pid: Some(44),
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
                stop_attempts: stop_attempts.clone(),
                first_stop_entered: first_stop_entered.clone(),
                release_first_stop: release_first_stop.clone(),
            })));
        manager
            .services
            .lock()
            .await
            .insert("shutdown-convergence:web".to_owned(), service);
        manager.persist_state().await;

        let pause_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_pause_availability(&project_path).await }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            first_stop_entered.notified(),
        )
        .await
        .expect("availability stop enters its controller");

        let shutdown_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !manager.is_shutting_down() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown publishes its lifecycle gate");
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(!shutdown_task.is_finished());
        assert!(
            manager
                .get_service_controller("shutdown-convergence:web")
                .await
                .is_some()
        );

        release_first_stop.notify_one();
        let pause_error = pause_task
            .await
            .expect("join availability pause")
            .expect_err("injected availability stop failure surfaces");
        assert!(format!("{pause_error:#}").contains("injected stop failure"));
        shutdown_task
            .await
            .expect("join shutdown")
            .expect("finish shutdown after convergence drains");

        assert_eq!(stop_attempts.load(Ordering::SeqCst), 2);
        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("reload convergence state after shutdown");
        assert_eq!(persisted.services.len(), 1);
        assert_eq!(persisted.services[0].status, ServiceState::Running);
    }

    #[tokio::test]
    async fn shutdown_gate_rejects_an_attachment_write_queued_before_teardown() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("shutdown-attachment-project");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "shutdown-attachment").await;
        let transition_guard = manager.attachment_transition_lock.lock().await;
        let attach_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move {
                manager
                    .project_attach(
                        project_path,
                        AttachmentSource::Editor {
                            name: "vscode".to_owned(),
                            id: "shutdown-window".to_owned(),
                            pid: Some(std::process::id()),
                        },
                    )
                    .await
            }
        });
        tokio::task::yield_now().await;

        let shutdown_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !manager.is_shutting_down() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown publishes its attachment gate");
        drop(transition_guard);

        let error = attach_task
            .await
            .expect("join queued attachment")
            .expect_err("queued attachment is rejected after shutdown begins");
        assert!(error.downcast_ref::<DaemonShuttingDown>().is_some());
        shutdown_task
            .await
            .expect("join shutdown")
            .expect("finish shutdown");
        assert!(manager.attachments.lock().await.all_projects().is_empty());
    }

    #[tokio::test]
    async fn shutdown_gate_rejects_a_pause_queued_before_availability_publication() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("shutdown-pause-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "shutdown-pause").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load shutdown-pause availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable Always On before queued pause");

        let publication_guard = manager.availability_publication_lock.lock().await;
        let pause_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_pause_availability(&project_path).await }
        });
        tokio::task::yield_now().await;
        let shutdown_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !manager.is_shutting_down() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown publishes its availability-publication gate");
        drop(publication_guard);

        let error = pause_task
            .await
            .expect("join queued pause")
            .expect_err("queued pause is rejected after shutdown begins");
        assert!(error.downcast_ref::<DaemonShuttingDown>().is_some());
        shutdown_task
            .await
            .expect("join shutdown")
            .expect("finish shutdown");
        let snapshot = availability
            .snapshot()
            .await
            .expect("reload availability after shutdown");
        assert!(snapshot.always_on());
        assert!(!snapshot.is_paused());
    }

    #[tokio::test]
    async fn start_failure_persists_retained_unverified_process_evidence() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("retained-start-failure-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "retained-start-failure").await;
        let retained_pid = 42;
        manager.factories.insert(
            0,
            Arc::new(RetainedStartFailureFactory { pid: retained_pid }),
        );
        write_availability_worker_config(
            &project_path,
            "retained-start-failure",
            "retained-start-failure.localhost",
            &["web"],
        );

        let error = manager
            .apply_config(project_path, None, false)
            .await
            .expect_err("injected retained start failure must surface");
        assert!(format!("{error:#}").contains("injected retained start failure"));

        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("load retained start-failure evidence");
        let retained = persisted
            .services
            .iter()
            .find(|service| service.name == "retained-start-failure:web")
            .expect("persist retained start-failure service evidence");
        assert_eq!(retained.pid, Some(retained_pid));
        assert_eq!(retained.process_identity, None);
        assert_eq!(retained.status, ServiceState::Stopped);
    }

    #[tokio::test]
    async fn retained_start_failure_rolls_back_when_its_first_persistence_attempt_fails() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir
            .path()
            .join("retained-start-persistence-failure-project");
        let state_path = dir.path().join("state.json");
        let (mut manager, _instance_id, _availability_data_dir) = availability_manager(
            dir.path(),
            &project_path,
            "retained-start-persistence-failure",
        )
        .await;
        let create_count = Arc::new(AtomicUsize::new(0));
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetainedStartRetryFactory {
                create_count: create_count.clone(),
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
                state_path_failure_fixture: Some(state_path.clone()),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "retained-start-persistence-failure",
            "retained-start-persistence-failure.localhost",
            &["web"],
        );

        let error = manager
            .apply_config(project_path, None, false)
            .await
            .expect_err("retained start failure with failed persistence must surface");
        let error = format!("{error:#}");
        assert!(
            error.contains("injected retained start failure"),
            "unexpected start error: {error}"
        );
        assert!(
            error.contains("failed to persist retained ownership"),
            "missing first persistence error: {error}"
        );
        assert!(
            error.contains(
                "the retained controller was stopped and the cleaned state was persisted"
            ),
            "missing rollback outcome: {error}"
        );
        assert_eq!(create_count.load(Ordering::SeqCst), 1);
        assert_eq!(start_count.load(Ordering::SeqCst), 1);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("retained-start-persistence-failure:web")
                .await
                .is_none(),
            "successful rollback must release the retained controller"
        );

        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("load cleaned state after rollback");
        let cleaned = persisted
            .services
            .iter()
            .find(|service| service.name == "retained-start-persistence-failure:web")
            .expect("persist cleaned service state");
        assert_eq!(cleaned.pid, None);
        assert_eq!(cleaned.process_identity, None);
        assert_eq!(cleaned.container_id, None);
        assert_eq!(cleaned.status, ServiceState::Stopped);
        assert!(state_path.is_file());
    }

    #[tokio::test]
    async fn retained_start_failure_is_stopped_before_a_retry_starts_a_replacement() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("retained-start-retry-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "retained-start-retry").await;
        let create_count = Arc::new(AtomicUsize::new(0));
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetainedStartRetryFactory {
                create_count: create_count.clone(),
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
                state_path_failure_fixture: None,
            }),
        );
        write_availability_worker_config(
            &project_path,
            "retained-start-retry",
            "retained-start-retry.localhost",
            &["web"],
        );

        let first_error = manager
            .apply_config(project_path.clone(), None, false)
            .await
            .expect_err("first controller retains incomplete ownership");
        assert!(format!("{first_error:#}").contains("injected retained start failure"));
        assert_eq!(create_count.load(Ordering::SeqCst), 1);
        assert_eq!(start_count.load(Ordering::SeqCst), 1);
        assert_eq!(stop_count.load(Ordering::SeqCst), 0);

        manager
            .apply_config(project_path, None, false)
            .await
            .expect("retry stops the incomplete controller and starts a replacement");

        assert_eq!(create_count.load(Ordering::SeqCst), 2);
        assert_eq!(start_count.load(Ordering::SeqCst), 2);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        let replacement = manager
            .get_service_controller("retained-start-retry:web")
            .await
            .expect("replacement controller is installed");
        let replacement = replacement.lock().await;
        assert!(replacement.owned_process_id().is_some());
        assert!(replacement.process_identity().is_some());
        assert_eq!(replacement.read_state().await.status, ServiceState::Running);
    }

    #[tokio::test]
    async fn pidless_start_failure_marker_survives_for_fresh_reconciliation() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("pidless-start-failure-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "pidless-start-failure").await;
        let retained_marker = "host-pidless-start-failure";
        manager.factories.insert(
            0,
            Arc::new(RetainedPidlessStartFailureFactory {
                container_id: retained_marker.to_owned(),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "pidless-start-failure",
            "pidless-start-failure.localhost",
            &["web"],
        );

        let error = manager
            .apply_config(project_path.clone(), None, false)
            .await
            .expect_err("pidless retained start failure must surface");
        assert!(format!("{error:#}").contains("injected retained start failure"));
        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("load pidless retained start-failure evidence");
        assert_eq!(persisted.services.len(), 1);
        assert_eq!(persisted.services[0].pid, None);
        assert_eq!(
            persisted.services[0].container_id.as_deref(),
            Some(retained_marker)
        );
        drop(manager);

        let (fresh_manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "pidless-start-failure").await;
        let reconcile_error = fresh_manager
            .reconcile_stale_runtime_state()
            .await
            .expect_err("fresh daemon must preserve a pidless retained marker");
        let reconcile_error = format!("{reconcile_error:#}");
        assert!(
            reconcile_error.contains(
                "service `pidless-start-failure:web` has container evidence without a confirmable PID"
            ),
            "unexpected reconciliation error: {reconcile_error}"
        );
        let preserved = fresh_manager
            .state_manager
            .load()
            .await
            .expect("reload preserved pidless evidence");
        assert_eq!(preserved.services[0].pid, None);
        assert_eq!(
            preserved.services[0].container_id.as_deref(),
            Some(retained_marker)
        );
    }

    #[tokio::test]
    async fn runtime_persistence_ignores_status_pids_without_a_cleanup_handle() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("process-identity-project");
        let (manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "process-identity").await;
        let identity = test_process_identity(1_234, 42, "/bin/controller-owned");
        let controller = Arc::new(Mutex::new(TestController {
            id: "process-identity:web".to_owned(),
            state: RuntimeState {
                pid: Some(42),
                port: Some(3000),
                status: ServiceState::Running,
                health_status: HealthStatus::Healthy,
            },
            fail_start: false,
            fail_stop: false,
            owned_process_id: None,
            process_identity: Some(identity.clone()),
            container_id: None,
        }));
        let runtime_controller: Arc<Mutex<dyn ServiceController>> = controller.clone();
        let mut service =
            availability_test_service(instance_id, "process-identity", &project_path, false);
        service.runtime_state = ServiceRuntime::Controller(runtime_controller);
        manager
            .services
            .lock()
            .await
            .insert("process-identity:web".to_owned(), service);

        manager.persist_state().await;
        let running = manager
            .state_manager
            .load()
            .await
            .expect("load running controller identity");
        assert_eq!(running.services[0].pid, Some(42));
        assert_eq!(running.services[0].process_identity, Some(identity));

        controller.lock().await.state.status = ServiceState::Stopped;
        manager.persist_state().await;
        let stopped = manager
            .state_manager
            .load()
            .await
            .expect("load stopped controller identity");
        assert_eq!(stopped.services[0].pid, None);
        assert_eq!(stopped.services[0].process_identity, None);
    }

    #[tokio::test]
    async fn runtime_persistence_retains_a_stopped_leaders_live_group_ownership() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("leaderless-group-project");
        let (manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "leaderless-group").await;
        let runtime = crate::runtime::process::ProcessRuntime::new(dir.path().join("notify.sock"));
        let service_config = ServiceConfig::Legacy(ExecServiceConfig {
            command: Some("trap '' HUP; set +m; sleep 30 & exec sleep 0.2".to_owned()),
            ..Default::default()
        });
        let controller = Arc::new(Mutex::new(ExecController::new(
            "leaderless-group:web".to_owned(),
            runtime.clone(),
            service_config.clone(),
            project_path.clone(),
            None,
            HashMap::new(),
        )));
        controller
            .lock()
            .await
            .start()
            .await
            .expect("start process-group leader");
        let spawn_pid = {
            let controller = controller.lock().await;
            controller
                .owned_process_id()
                .expect("capture process-group leader PID")
        };
        let mut cleanup = ProcessGroupCleanup::new(spawn_pid);
        let spawn_identity = {
            let controller = controller.lock().await;
            controller
                .process_identity()
                .expect("capture process-group identity")
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if controller.lock().await.read_state().await.status == ServiceState::Stopped {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "process-group leader did not exit before the deadline"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            runtime
                .owned_process_or_group_exists(spawn_pid, &spawn_identity)
                .expect("inspect leaderless process group"),
            "the leader exits while its descendant group remains live (PID {spawn_pid}, PGID {})",
            spawn_identity.process_group_id
        );

        let runtime_controller: Arc<Mutex<dyn ServiceController>> = controller;
        let mut service =
            availability_test_service(instance_id, "leaderless-group", &project_path, false);
        service.service_config = service_config;
        service.runtime_state = ServiceRuntime::Controller(runtime_controller);
        manager
            .services
            .lock()
            .await
            .insert("leaderless-group:web".to_owned(), service);

        manager
            .persist_state_checked()
            .await
            .expect("persist stopped leader ownership");
        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("reload stopped leader ownership");
        let persisted = persisted
            .services
            .iter()
            .find(|service| service.name == "leaderless-group:web")
            .expect("persist leaderless service");
        assert_eq!(persisted.status, ServiceState::Stopped);
        assert_eq!(persisted.pid, Some(spawn_pid));
        assert_eq!(persisted.process_identity.as_ref(), Some(&spawn_identity));

        let error = manager
            .stop("leaderless-group:web")
            .await
            .expect_err("leaderless process group must fail closed");
        assert!(format!("{error:#}").contains("ownership cannot be revalidated"));
        let retained = manager
            .state_manager
            .load()
            .await
            .expect("reload retained leaderless ownership");
        let retained = retained
            .services
            .iter()
            .find(|service| service.name == "leaderless-group:web")
            .expect("retain leaderless service evidence");
        assert_eq!(retained.pid, Some(spawn_pid));
        assert_eq!(retained.process_identity.as_ref(), Some(&spawn_identity));

        let group = cleanup.group();
        assert!(matches!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(-group), None),
            Ok(()) | Err(nix::errno::Errno::EPERM)
        ));
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(-group), Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => panic!("terminate leaderless test process group: {error}"),
        }
        for _ in 0..100 {
            if matches!(
                nix::sys::signal::kill(nix::unistd::Pid::from_raw(-group), None),
                Err(nix::errno::Errno::ESRCH)
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        manager
            .stop("leaderless-group:web")
            .await
            .expect("clear ownership after the leaderless group is gone");
        cleanup.disarm();
        let stopped = manager
            .state_manager
            .load()
            .await
            .expect("reload state after confirmed cleanup");
        let stopped = stopped
            .services
            .iter()
            .find(|service| service.name == "leaderless-group:web")
            .expect("retain stopped service record");
        assert_eq!(stopped.pid, None);
        assert_eq!(stopped.process_identity, None);
    }

    #[test]
    fn stale_runtime_cleanup_rejects_reserved_and_current_group_identifiers() {
        let runtime = crate::runtime::process::ProcessRuntime::new(PathBuf::from("notify.sock"));
        for operation in [
            runtime.capture_process_identity(1).map(|_| ()),
            runtime.unverified_stale_process_exists(1).map(|_| ()),
        ] {
            assert!(
                format!("{}", operation.expect_err("PID 1 must be rejected"))
                    .contains("reserved process ID 1")
            );
        }

        let process_group = u32::try_from(nix::unistd::getpgrp().as_raw())
            .expect("current process-group ID is positive");
        if process_group != std::process::id() {
            for operation in [
                runtime.capture_process_identity(process_group).map(|_| ()),
                runtime
                    .unverified_stale_process_exists(process_group)
                    .map(|_| ()),
            ] {
                assert!(
                    format!(
                        "{}",
                        operation.expect_err("the current process group must be rejected")
                    )
                    .contains("current locald process group")
                );
            }
        }
    }

    #[test]
    fn stale_runtime_cleanup_rejects_a_mismatched_process_identity() {
        let runtime = crate::runtime::process::ProcessRuntime::new(PathBuf::from("notify.sock"));
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn identity test process");
        let pid = child.id();
        let mut identity = runtime
            .capture_process_identity(pid)
            .expect("inspect identity test process")
            .expect("identity test process is live");
        identity.birth = Some(different_process_birth(
            identity
                .birth
                .as_ref()
                .expect("captured identity has process birth authority"),
        ));

        let error = runtime
            .verify_stale_process(pid, &identity)
            .expect_err("mismatched process identity must fail closed");
        assert!(format!("{error:#}").contains("was reused"));
        assert!(
            child
                .try_wait()
                .expect("inspect identity test process")
                .is_none()
        );
        child.kill().expect("terminate identity test process");
        child.wait().expect("reap identity test process");
    }

    #[test]
    fn stale_runtime_identity_survives_exec_with_the_same_process_birth() {
        let runtime = crate::runtime::process::ProcessRuntime::new(PathBuf::from("notify.sock"));
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 0.5; exec sleep 2")
            .spawn()
            .expect("spawn exec identity test process");
        let pid = child.id();
        let identity = runtime
            .capture_process_identity(pid)
            .expect("inspect shell identity")
            .expect("shell process is live");
        let initial_executable = identity
            .executable
            .clone()
            .expect("shell executable is observable");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let observed_after_exec = loop {
            let observed = runtime
                .capture_process_identity(pid)
                .expect("inspect process across exec")
                .expect("exec process remains live");
            if observed.executable.as_ref() != Some(&initial_executable) {
                break observed;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "test process did not exec before the deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        assert_eq!(observed_after_exec.birth, identity.birth);
        assert_eq!(
            observed_after_exec.process_group_id,
            identity.process_group_id
        );
        assert!(
            runtime
                .verify_stale_process(pid, &identity)
                .expect("verify process after exec")
                .is_some()
        );

        child.kill().expect("terminate exec identity test process");
        child.wait().expect("reap exec identity test process");
    }

    #[test]
    fn stale_runtime_cleanup_preserves_a_verified_group_after_its_leader_exits() {
        let runtime = crate::runtime::process::ProcessRuntime::new(PathBuf::from("notify.sock"));
        let mut leader = Command::new("sh");
        leader
            .arg("-c")
            .arg("sleep 30 & exec sleep 0.2")
            .process_group(0);
        let mut leader = leader.spawn().expect("spawn stale process-group leader");
        let pid = leader.id();
        let identity = runtime
            .capture_process_identity(pid)
            .expect("inspect stale process-group leader")
            .expect("stale process-group leader is live");
        assert_eq!(
            identity.process_group_id,
            i32::try_from(pid).expect("test process ID fits i32")
        );
        leader.wait().expect("reap stale process-group leader");

        let error = runtime
            .verify_stale_process(pid, &identity)
            .expect_err("live orphaned group must preserve cleanup evidence");
        assert!(format!("{error:#}").contains("verified process group"));

        let group = i32::try_from(pid).expect("test process-group ID fits i32");
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(-group), Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => panic!("terminate stale test process group: {error}"),
        }
    }

    #[tokio::test]
    async fn stale_runtime_reconciliation_restores_catalogued_legacy_project() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-restore-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "legacy-restore").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "legacy-restore",
            "legacy-restore.localhost",
            &["web"],
        );
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "legacy-restore:web".to_owned(),
                    config: test_config_with_domain("legacy-restore", "legacy-restore.localhost"),
                    path: project_path,
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist legacy Running evidence");

        let restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile legacy runtime evidence");
        manager.restore_policy_owned_projects(restore_plan).await;

        assert!(
            manager
                .get_service_controller("legacy-restore:web")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn fresh_daemon_restores_an_availability_managed_always_on_project() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("always-on-restore-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "always-on-restore").await;
        write_availability_worker_config(
            &project_path,
            "always-on-restore",
            "always-on-restore.localhost",
            &["web"],
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load Always On restore policy");
        availability
            .set_always_on(true)
            .await
            .expect("persist Always On restore policy");
        manager
            .state_manager
            .save(&ServerState::default())
            .await
            .expect("persist empty runtime snapshot");
        drop(manager);

        let catalog =
            Registry::load_from_paths(locald_core::CatalogPaths::for_data_dir(dir.path()))
                .await
                .expect("reload Always On catalog");
        let mut fresh = ProcessManager::new_with_availability_data_dir(
            dir.path().join("fresh-notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
            availability_data_dir,
        )
        .expect("create fresh Always On manager");
        fresh.set_host_syncer(Arc::new(NoopHostSyncer));
        fresh.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let restore_plan = fresh
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile empty runtime snapshot");
        fresh.restore_policy_owned_projects(restore_plan).await;

        assert!(
            fresh
                .get_service_controller("always-on-restore:web")
                .await
                .is_some()
        );
        assert_eq!(
            fresh
                .services
                .lock()
                .await
                .get("always-on-restore:web")
                .expect("restored Always On service")
                .instance_id,
            instance_id
        );
        let snapshot = availability
            .snapshot()
            .await
            .expect("reload Always On policy after restore");
        assert!(snapshot.always_on());
        assert!(!snapshot.is_paused());
        assert_eq!(snapshot.last_convergence_error(), None);
    }

    #[tokio::test]
    async fn failed_legacy_restore_evidence_survives_for_fresh_retry() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-retry-project");
        let (mut manager, _instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "legacy-retry").await;
        write_availability_worker_config(
            &project_path,
            "legacy-retry",
            "legacy-retry.localhost",
            &["first", "second"],
        );
        let failure_consumed = Arc::new(AtomicBool::new(false));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareWithoutPidFactory {
                failure_consumed: failure_consumed.clone(),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "legacy-retry:intent".to_owned(),
                    config: test_config_with_domain("legacy-retry", "legacy-retry.localhost"),
                    path: project_path.clone(),
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: None,
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist retryable legacy intent");

        let restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile retryable legacy intent");
        manager.restore_policy_owned_projects(restore_plan).await;
        assert!(failure_consumed.load(Ordering::SeqCst));
        let failed_snapshot = manager
            .state_manager
            .load()
            .await
            .expect("reload failed legacy restore snapshot");
        assert!(failed_snapshot.services.iter().any(|service| {
            service.name == "legacy-retry:intent" && service.status == ServiceState::Running
        }));

        let catalog =
            Registry::load_from_paths(locald_core::CatalogPaths::for_data_dir(dir.path()))
                .await
                .expect("reload retry catalog");
        let mut fresh = ProcessManager::new_with_availability_data_dir(
            dir.path().join("fresh-notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
            availability_data_dir,
        )
        .expect("create fresh retry manager");
        fresh.set_host_syncer(Arc::new(NoopHostSyncer));
        fresh.factories.insert(
            0,
            Arc::new(RetryPrepareWithoutPidFactory {
                failure_consumed,
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let retry_plan = fresh
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile preserved retry intent");
        fresh.restore_policy_owned_projects(retry_plan).await;

        assert!(
            fresh
                .get_service_controller("legacy-retry:first")
                .await
                .is_some()
        );
        assert!(
            fresh
                .get_service_controller("legacy-retry:second")
                .await
                .is_some()
        );
        assert!(
            !fresh
                .state_manager
                .load()
                .await
                .expect("reload successful retry snapshot")
                .services
                .iter()
                .any(|service| service.name == "legacy-retry:intent")
        );
    }

    #[tokio::test]
    async fn service_stop_reports_legacy_restore_that_has_not_materialized() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-service-stop-project");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "legacy-service-stop").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "legacy-service-stop",
            "legacy-service-stop.localhost",
            &["web"],
        );
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "legacy-service-stop:web".to_owned(),
                    config: test_config_with_domain(
                        "legacy-service-stop",
                        "legacy-service-stop.localhost",
                    ),
                    path: project_path.clone(),
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist pending service restore");
        let restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile pending service restore");

        let error = manager
            .stop("legacy-service-stop:web")
            .await
            .expect_err("pending service stop must be explicit");
        let pending = error
            .downcast_ref::<ServiceRestorePending>()
            .expect("pending restore error type");
        assert_eq!(pending.name, "legacy-service-stop:web");
        assert_eq!(pending.instance_id, instance_id);
        assert!(manager.services.lock().await.is_empty());

        manager.restore_policy_owned_projects(restore_plan).await;
        assert!(
            manager
                .get_service_controller("legacy-service-stop:web")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn explicit_legacy_stop_retires_pending_restore_evidence() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-stop-project");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "legacy-stop").await;
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "legacy-stop:web".to_owned(),
                    config: test_config_with_domain("legacy-stop", "legacy-stop.localhost"),
                    path: project_path.clone(),
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist legacy stop intent");
        let restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile legacy stop evidence");

        manager
            .stop_project(&project_path)
            .await
            .expect("explicitly stop legacy project");
        manager.restore_policy_owned_projects(restore_plan).await;

        assert!(
            manager
                .state_manager
                .load()
                .await
                .expect("reload stopped legacy state")
                .services
                .is_empty()
        );
        assert!(manager.services.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stale_runtime_reconciliation_does_not_retarget_ambiguous_reused_path() {
        let dir = tempdir().expect("create temporary directory");
        let old_path = dir.path().join("legacy-old-path");
        let moved_path = dir.path().join("legacy-moved-path");
        let (mut manager, moved_id, _availability_data_dir) =
            availability_manager(dir.path(), &old_path, "legacy-moved").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        std::fs::rename(&old_path, &moved_path).expect("move legacy project");
        let canonical_moved =
            std::fs::canonicalize(&moved_path).expect("canonical moved legacy path");
        std::fs::create_dir_all(&old_path).expect("recreate legacy path");
        write_availability_worker_config(
            &old_path,
            "legacy-replacement",
            "legacy-replacement.localhost",
            &["web"],
        );
        let canonical_old = std::fs::canonicalize(&old_path).expect("canonical reused legacy path");
        let replacement_discovery = Registry::discover(canonical_old.clone())
            .await
            .expect("discover legacy replacement");
        let replacement_id = {
            let mut registry = manager.registry.lock().await;
            let moved = registry
                .instances
                .get_mut(&moved_id)
                .expect("moved legacy instance");
            moved.current_path = Some(canonical_moved.clone());
            // Retain the old locator as historical evidence so the path-only
            // runtime record is explicitly ambiguous.
            moved.last_known_path = canonical_old.clone();
            moved.presence = CatalogPresence::Active;
            registry.legacy_paths.insert(canonical_moved, moved_id);
            let replacement_id = registry
                .register_project(replacement_discovery, Some("legacy-replacement".to_owned()))
                .expect("register legacy replacement");
            let moved = registry
                .instances
                .get_mut(&moved_id)
                .expect("missing legacy instance");
            moved.current_path = None;
            moved.last_known_path = canonical_old.clone();
            moved.presence = CatalogPresence::Missing;
            replacement_id
        };
        assert_ne!(replacement_id, moved_id);
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "legacy-moved:web".to_owned(),
                    config: test_config_with_domain("legacy-moved", "legacy-moved.localhost"),
                    path: canonical_old,
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist ambiguous legacy Running evidence");

        let restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile ambiguous legacy runtime evidence");
        manager.restore_policy_owned_projects(restore_plan).await;

        assert!(manager.services.lock().await.is_empty());
        assert!(
            manager
                .get_service_controller("legacy-replacement:web")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn stale_runtime_reconciliation_does_not_register_unseen_replacement_worktree() {
        let dir = tempdir().expect("create temporary directory");
        let repository = dir.path().join("legacy-repository");
        std::fs::create_dir(&repository).expect("create legacy repository");
        git(&repository, &["init", "-b", "main"]);
        git(&repository, &["config", "user.name", "locald tests"]);
        git(
            &repository,
            &["config", "user.email", "locald@example.test"],
        );
        std::fs::write(repository.join("README.md"), "fixture\n").expect("write fixture");
        std::fs::write(
            repository.join("locald.toml"),
            r#"[project]
name = "legacy-recreated"
domain = "legacy-recreated.localhost"

[services.web]
type = "worker"
command = "unused-by-test-factory"
"#,
        )
        .expect("write legacy config");
        git(&repository, &["add", "README.md", "locald.toml"]);
        git(&repository, &["commit", "-m", "initial"]);

        let worktree = dir.path().join("legacy-worktree");
        let worktree_arg = worktree.to_str().expect("UTF-8 legacy worktree path");
        git(
            &repository,
            &["worktree", "add", "-b", "first", worktree_arg],
        );
        let canonical_worktree =
            std::fs::canonicalize(&worktree).expect("canonical first legacy worktree");
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let stale_instance = catalog
            .register_project(
                Registry::discover(canonical_worktree.clone())
                    .await
                    .expect("discover first legacy worktree"),
                Some("legacy-recreated".to_owned()),
            )
            .expect("register first legacy worktree");
        catalog.save().await.expect("persist stale legacy catalog");

        git(&repository, &["worktree", "remove", worktree_arg]);
        git(&repository, &["worktree", "prune"]);
        git(
            &repository,
            &["worktree", "add", "-b", "second", worktree_arg],
        );
        let replacement_discovery = Registry::discover(
            std::fs::canonicalize(&worktree).expect("canonical replacement worktree"),
        )
        .await
        .expect("discover unseen replacement identity");
        let mut expected_catalog = catalog.clone();
        let replacement_instance = expected_catalog
            .register_project(replacement_discovery, Some("legacy-recreated".to_owned()))
            .expect("derive replacement identity");
        assert_ne!(replacement_instance, stale_instance);

        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "legacy-recreated:web".to_owned(),
                    config: test_config_with_domain(
                        "legacy-recreated",
                        "legacy-recreated.localhost",
                    ),
                    path: canonical_worktree,
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist unregistered replacement evidence");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            state_manager,
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
            dir.path().join("availability-data"),
        )
        .expect("create stale-catalog process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile stale worktree evidence");
        manager.restore_policy_owned_projects(restore_plan).await;

        assert!(manager.services.lock().await.is_empty());
        let registry = manager.registry.lock().await;
        assert!(registry.instances.contains_key(&stale_instance));
        assert!(!registry.instances.contains_key(&replacement_instance));
    }

    #[tokio::test]
    async fn stale_runtime_reconciliation_preserves_unconfirmed_process_evidence() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("unconfirmed-cleanup-project");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "unconfirmed-cleanup").await;
        let current_pid = std::process::id();
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "unconfirmed-cleanup:web".to_owned(),
                    config: test_config_with_domain(
                        "unconfirmed-cleanup",
                        "unconfirmed-cleanup.localhost",
                    ),
                    path: project_path,
                    pid: Some(current_pid),
                    process_identity: None,
                    container_id: Some("host-unconfirmed".to_owned()),
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist unconfirmed process evidence");

        let error = manager
            .reconcile_stale_runtime_state()
            .await
            .expect_err("the current daemon process cannot be treated as stale");
        assert!(format!("{error:#}").contains("current locald process"));

        let preserved = manager
            .state_manager
            .load()
            .await
            .expect("reload preserved process evidence");
        assert_eq!(preserved.services.len(), 1);
        assert_eq!(preserved.services[0].pid, Some(current_pid));
        assert_eq!(
            preserved.services[0].container_id.as_deref(),
            Some("host-unconfirmed")
        );
    }

    #[tokio::test]
    async fn stale_runtime_reconciliation_never_signals_a_live_unverified_pid() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("unverified-live-project");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "unverified-live").await;
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn unverified test process");
        let stale_pid = child.id();
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "unverified-live:web".to_owned(),
                    config: test_config_with_domain("unverified-live", "unverified-live.localhost"),
                    path: project_path,
                    pid: Some(stale_pid),
                    process_identity: None,
                    container_id: Some("host-unverified-live".to_owned()),
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist unverified live process evidence");

        let error = manager
            .reconcile_stale_runtime_state()
            .await
            .expect_err("live unverified process requires manual recovery");
        assert!(format!("{error:#}").contains("no verified ownership identity"));
        assert!(
            child
                .try_wait()
                .expect("inspect unverified test process")
                .is_none()
        );
        child.kill().expect("terminate unverified test process");
        child.wait().expect("reap unverified test process");

        let preserved = manager
            .state_manager
            .load()
            .await
            .expect("reload unverified process evidence");
        assert_eq!(preserved.services[0].pid, Some(stale_pid));
        assert!(preserved.services[0].process_identity.is_none());
    }

    #[tokio::test]
    async fn stale_runtime_reconciliation_clears_confirmed_process_evidence() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("confirmed-cleanup-project");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "confirmed-cleanup").await;
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn stale test process");
        let stale_pid = child.id();
        let process_identity = manager
            .runtime
            .process
            .capture_process_identity(stale_pid)
            .expect("inspect stale test process")
            .expect("stale test process is live");
        let waiter = std::thread::spawn(move || child.wait().expect("reap stale test process"));
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "confirmed-cleanup:web".to_owned(),
                    config: test_config_with_domain(
                        "confirmed-cleanup",
                        "confirmed-cleanup.localhost",
                    ),
                    path: project_path,
                    pid: Some(stale_pid),
                    process_identity: Some(process_identity),
                    container_id: Some("host-confirmed".to_owned()),
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist confirmed process evidence");

        let _restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile confirmed stale process");
        waiter.join().expect("join stale process reaper");

        let reconciled = manager
            .state_manager
            .load()
            .await
            .expect("reload reconciled process evidence");
        assert_eq!(reconciled.services.len(), 1);
        assert_eq!(reconciled.services[0].pid, None);
        assert_eq!(reconciled.services[0].container_id, None);
    }

    #[tokio::test]
    async fn removed_project_is_not_restored_from_stale_running_evidence() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("removed-restore-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "removed-restore").await;
        let domain: DomainName = "removed-restore.localhost"
            .parse()
            .expect("valid removed project domain");
        {
            let mut registry = manager.registry.lock().await;
            let mut candidate = registry.clone();
            candidate
                .replace_domain_claims(
                    instance_id,
                    [DomainClaim::service(
                        domain.clone(),
                        instance_id,
                        "removed-restore:web".to_owned(),
                    )],
                )
                .expect("claim removed project domain");
            registry
                .commit_candidate(candidate)
                .await
                .expect("persist removed project domain");
            manager.domain_index.store(registry.domain_index().clone());
        }
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load removed project availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable removed project Always On");
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "removed-restore:web".to_owned(),
                    config: test_config_with_domain("removed-restore", "removed-restore.localhost"),
                    path: project_path.clone(),
                    pid: None,
                    process_identity: None,
                    container_id: None,
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                    health_source: HealthSource::None,
                }],
            })
            .await
            .expect("persist stale Running evidence");

        let _restore_plan = manager
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile handle-free stale evidence");
        manager
            .remove_project(&project_path)
            .await
            .expect("remove project after reconciliation");

        let catalog =
            Registry::load_from_paths(locald_core::CatalogPaths::for_data_dir(dir.path()))
                .await
                .expect("reload catalog after removal");
        let mut fresh = ProcessManager::new_with_availability_data_dir(
            dir.path().join("fresh-notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
            availability_data_dir,
        )
        .expect("create fresh process manager");
        fresh.set_host_syncer(Arc::new(NoopHostSyncer));
        let restore_plan = fresh
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile fresh manager state");
        fresh.restore_policy_owned_projects(restore_plan).await;

        assert!(fresh.registry.lock().await.instances.is_empty());
        assert!(fresh.services.lock().await.is_empty());
        assert!(
            fresh
                .domain_index()
                .snapshot()
                .resolve(domain.as_str())
                .is_none()
        );
        assert!(
            fresh
                .state_manager
                .load()
                .await
                .expect("reload fresh runtime state")
                .services
                .is_empty()
        );
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
        ProcessManager::build_domain_claims(
            test_instance_id(),
            config,
            Path::new("/nonexistent/locald-domain-claims-test"),
        )
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

    fn git(current_dir: &Path, arguments: &[&str]) {
        let mut command = Command::new("git");
        command.args(arguments).current_dir(current_dir);
        for variable in [
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_OBJECT_DIRECTORY",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_IMPLICIT_WORK_TREE",
            "GIT_GRAFT_FILE",
            "GIT_INDEX_FILE",
            "GIT_NO_REPLACE_OBJECTS",
            "GIT_REPLACE_REF_BASE",
            "GIT_PREFIX",
            "GIT_SHALLOW_FILE",
            "GIT_COMMON_DIR",
        ] {
            command.env_remove(variable);
        }
        let output = command.output().expect("run git");
        assert!(
            output.status.success(),
            "git {} failed:\nstdout: {}\nstderr: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn install_test_claim(manager: &ProcessManager, domain: &str, service_name: &str) {
        install_test_claim_for_instance(manager, test_instance_id(), domain, service_name);
    }

    fn install_test_claim_for_instance(
        manager: &ProcessManager,
        instance_id: ProjectInstanceId,
        domain: &str,
        service_name: &str,
    ) {
        let current = manager.domain_index.snapshot();
        let replacement = current
            .replacing_instance(
                instance_id,
                [DomainClaim::service(
                    domain.parse().expect("valid test domain"),
                    instance_id,
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
    fn explicit_project_domains_remain_strict_and_service_labels_are_generated() {
        let invalid_project_domain = config_with_services(
            ProjectConfig {
                name: "project".to_owned(),
                domain: Some("My_Project.localhost".to_owned()),
                ..Default::default()
            },
            &["web"],
        );
        assert!(
            ProcessManager::build_domain_claims(
                test_instance_id(),
                &invalid_project_domain,
                Path::new("/nonexistent/locald-domain-claims-test"),
            )
            .expect_err("explicit project domains are not rewritten")
            .to_string()
            .contains("invalid exact base domain")
        );

        let generated_service_domain = config_with_services(
            ProjectConfig {
                name: "project".to_owned(),
                domain: None,
                ..Default::default()
            },
            &["web", "api_v2"],
        );
        assert_eq!(
            claim_domains(&generated_service_domain).get("project:api_v2"),
            Some(&"api-v2.project.localhost".to_owned())
        );
    }

    #[test]
    fn colliding_generated_service_labels_are_rejected_atomically() {
        let config = config_with_services(
            ProjectConfig {
                name: "project".to_owned(),
                domain: None,
                ..Default::default()
            },
            &["web", "my_api", "my-api"],
        );
        let claims = ProcessManager::build_domain_claims(
            test_instance_id(),
            &config,
            Path::new("/nonexistent/locald-domain-claims-test"),
        )
        .expect("generated labels are individually valid");

        let error = locald_core::DomainIndex::default()
            .replacing_instance(test_instance_id(), claims)
            .expect_err("colliding generated labels must fail as one claim set");

        assert!(error.to_string().contains("my-api.project.localhost"));
    }

    #[test]
    fn worktree_templates_qualify_claims_before_exact_domain_validation() {
        let temp = tempdir().expect("create temporary directory");
        let repository = temp.path().join("repository");
        std::fs::create_dir(&repository).expect("create repository directory");
        git(&repository, &["init", "-b", "main"]);
        git(&repository, &["config", "user.name", "locald tests"]);
        git(
            &repository,
            &["config", "user.email", "locald@example.test"],
        );
        std::fs::write(repository.join("README.md"), "fixture\n").expect("write fixture");
        git(&repository, &["add", "README.md"]);
        git(&repository, &["commit", "-m", "initial"]);

        let worktree = temp.path().join("feature-worktree");
        let worktree_path = worktree.to_str().expect("UTF-8 worktree path");
        git(
            &repository,
            &[
                "worktree",
                "add",
                "-b",
                "feature/JIRA-123_foo",
                worktree_path,
            ],
        );

        let mut config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "app"
domain = "app.localhost"

[worktrees]
domain = "{{branch.last}}.{{project.domain}}"

[services.web]
command = "web"

[services.api]
command = "api"
"#,
        )
        .expect("parse worktree config");

        let main_claims =
            ProcessManager::build_domain_claims(test_instance_id(), &config, &repository)
                .expect("primary checkout claims");
        let main_domains = main_claims
            .into_iter()
            .map(|claim| claim.domain.to_string())
            .collect::<HashSet<_>>();
        assert_eq!(
            main_domains,
            HashSet::from(["app.localhost".to_owned(), "api.app.localhost".to_owned(),])
        );

        let worktree_claims =
            ProcessManager::build_domain_claims(test_instance_id(), &config, &worktree)
                .expect("linked worktree claims");
        let worktree_domains = worktree_claims
            .into_iter()
            .map(|claim| claim.domain.to_string())
            .collect::<HashSet<_>>();
        assert_eq!(
            worktree_domains,
            HashSet::from([
                "jira-123-foo.app.localhost".to_owned(),
                "api.jira-123-foo.app.localhost".to_owned(),
            ])
        );

        config.worktrees.as_mut().expect("worktree config").domain =
            Some("bad_{{branch.last}}.{{project.domain}}".to_owned());
        let error = ProcessManager::build_domain_claims(test_instance_id(), &config, &worktree)
            .expect_err("invalid expanded templates remain strict");
        assert!(error.to_string().contains("invalid exact base domain"));
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
            &[expected_hosts(&[])]
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
    async fn registry_clean_preserves_watcher_for_reused_worktree_path() {
        let dir = tempdir().expect("create temporary directory");
        let repository = dir.path().join("repository");
        std::fs::create_dir(&repository).expect("create repository");
        git(&repository, &["init", "-b", "main"]);
        git(&repository, &["config", "user.name", "locald tests"]);
        git(
            &repository,
            &["config", "user.email", "locald@example.test"],
        );
        std::fs::write(repository.join("README.md"), "fixture\n").expect("write fixture");
        std::fs::write(
            repository.join("locald.toml"),
            "[project]\nname = \"reused\"\ndomain = \"reused.localhost\"\n",
        )
        .expect("write config fixture");
        git(&repository, &["add", "README.md", "locald.toml"]);
        git(&repository, &["commit", "-m", "initial"]);

        let reused_path = dir.path().join("reused-worktree");
        let reused_arg = reused_path.to_str().expect("UTF-8 worktree path");
        git(&repository, &["worktree", "add", "-b", "first", reused_arg]);
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let first_id = catalog
            .register_project(
                Registry::discover(
                    std::fs::canonicalize(&reused_path).expect("canonical first worktree"),
                )
                .await
                .expect("discover first worktree"),
                Some("first".to_owned()),
            )
            .expect("register first worktree");
        git(&repository, &["worktree", "remove", reused_arg]);
        git(&repository, &["worktree", "prune"]);
        git(
            &repository,
            &["worktree", "add", "-b", "second", reused_arg],
        );
        let second_id = catalog
            .register_project(
                Registry::discover(
                    std::fs::canonicalize(&reused_path).expect("canonical second worktree"),
                )
                .await
                .expect("discover second worktree"),
                Some("second".to_owned()),
            )
            .expect("register second worktree");
        assert_ne!(first_id, second_id);
        catalog.save().await.expect("persist reused catalog");

        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
        )
        .expect("create process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager.watch_config(reused_path.clone()).await;
        let canonical_reused = ProcessManager::canonicalize_path(&reused_path);

        assert_eq!(
            manager.registry_clean().await.expect("clean old worktree"),
            1
        );
        assert!(
            manager
                .watchers
                .lock()
                .await
                .contains_key(&canonical_reused)
        );
        assert!(
            !manager
                .forgotten_reload_paths
                .lock()
                .await
                .contains(&canonical_reused)
        );
        let registry = manager.registry.lock().await;
        assert!(!registry.instances.contains_key(&first_id));
        assert!(registry.instances.contains_key(&second_id));
    }

    #[tokio::test]
    async fn registry_clean_then_explicit_start_reactivates_replacement_watcher() {
        let dir = tempdir().expect("create temporary directory");
        let repository = dir.path().join("clean-start-repository");
        std::fs::create_dir(&repository).expect("create clean/start repository");
        git(&repository, &["init", "-b", "main"]);
        git(&repository, &["config", "user.name", "locald tests"]);
        git(
            &repository,
            &["config", "user.email", "locald@example.test"],
        );
        std::fs::write(repository.join("README.md"), "fixture\n").expect("write fixture");
        std::fs::write(
            repository.join("locald.toml"),
            r#"[project]
name = "clean-start"
domain = "clean-start.localhost"

[services.web]
type = "worker"
command = "unused-by-test-factory"
"#,
        )
        .expect("write clean/start config");
        git(&repository, &["add", "README.md", "locald.toml"]);
        git(&repository, &["commit", "-m", "initial"]);

        let reused_path = dir.path().join("clean-start-worktree");
        let reused_arg = reused_path.to_str().expect("UTF-8 reused path");
        git(&repository, &["worktree", "add", "-b", "first", reused_arg]);
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let removed_id = catalog
            .register_project(
                Registry::discover(
                    std::fs::canonicalize(&reused_path).expect("canonical first clean worktree"),
                )
                .await
                .expect("discover first clean worktree"),
                Some("clean-start".to_owned()),
            )
            .expect("register first clean worktree");
        catalog.save().await.expect("persist clean/start catalog");
        git(&repository, &["worktree", "remove", reused_arg]);
        git(&repository, &["worktree", "prune"]);
        git(
            &repository,
            &["worktree", "add", "-b", "second", reused_arg],
        );

        let mut manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
        )
        .expect("create clean/start process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let canonical_reused = ProcessManager::canonicalize_path(&reused_path);
        let (_, transition_lock) = manager.transition_lock_for_path(&reused_path).await;
        let runtime_guard = manager.runtime_projection_lock.lock().await;
        let clean_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.registry_clean().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if transition_lock.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("clean acquires the project transition before start");

        let start_task = tokio::spawn({
            let manager = manager.clone();
            let reused_path = reused_path.clone();
            async move { manager.start(reused_path, None, false).await }
        });
        tokio::task::yield_now().await;
        assert!(!start_task.is_finished());
        drop(runtime_guard);

        assert_eq!(
            clean_task
                .await
                .expect("join clean task")
                .expect("clean stale worktree"),
            1
        );
        start_task
            .await
            .expect("join explicit start")
            .expect("start replacement worktree");

        let (replacement_id, _) = manager
            .availability_instance_for_path(&reused_path)
            .await
            .expect("resolve registered replacement");
        assert_ne!(replacement_id, removed_id);
        assert!(
            manager
                .watchers
                .lock()
                .await
                .contains_key(&canonical_reused)
        );
        assert!(
            !manager
                .forgotten_reload_paths
                .lock()
                .await
                .contains(&canonical_reused)
        );
        assert!(
            manager
                .get_service_controller("clean-start:web")
                .await
                .is_some()
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
        let mut service = test_service(
            test_config_with_domain("busy-missing", "busy-missing.localhost"),
            ServiceConfig::Legacy(ExecServiceConfig::default()),
            ServiceRuntime::Controller(controller),
            canonical_path,
        );
        service.instance_id = instance_id;
        manager
            .services
            .lock()
            .await
            .insert("busy-missing:web".to_owned(), service);
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
            instance_id: test_instance_id(),
            controller_generation: 1,
            projection_generation: 1,
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
                instance_id: test_instance_id(),
                controller_generation: 1,
                projection_generation: 1,
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
                expected_hosts(&["first.localhost"]),
                expected_hosts(&["second.localhost"]),
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
            &[expected_hosts(&["first.localhost"])]
        );

        manager
            .stop_project(&project_path)
            .await
            .expect("stop initial project");
    }

    #[tokio::test]
    async fn retained_service_restart_stop_failure_preserves_previous_domain_claims() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reload-project");
        std::fs::create_dir(&project_path).expect("create project directory");
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "reload"
domain = "second.localhost"

[services.web]
type = "worker"
command = "sleep 31"
"#,
        )
        .expect("write replacement config");
        let canonical_path = std::fs::canonicalize(&project_path).expect("canonical project path");
        let previous_config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "reload"
domain = "first.localhost"

[services.web]
type = "worker"
command = "sleep 30"
"#,
        )
        .expect("parse previous config");
        let previous_service_config = previous_config.services["web"].clone();
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
                [DomainClaim::service(
                    "first.localhost".parse().expect("valid domain"),
                    instance_id,
                    "reload:web".to_owned(),
                )],
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
            port: None,
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        let stop_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let first_stop_entered = Arc::new(tokio::sync::Notify::new());
        let release_first_stop = Arc::new(tokio::sync::Notify::new());
        let failing_controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingFailStopController {
                id: "reload:web".to_owned(),
                state: running_state,
                stop_attempts: stop_attempts.clone(),
                first_stop_entered: first_stop_entered.clone(),
                release_first_stop: release_first_stop.clone(),
            }));
        let mut retained_service = test_service(
            previous_config.clone(),
            previous_service_config.clone(),
            ServiceRuntime::Controller(failing_controller.clone()),
            canonical_path,
        );
        retained_service.instance_id = instance_id;
        {
            let mut services = manager.services.lock().await;
            services.insert("reload:web".to_owned(), retained_service);
        }
        let host_sync_calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: host_sync_calls.clone(),
        }));

        let stop_manager = manager.clone();
        let stop_task = tokio::spawn(async move { stop_manager.stop("reload:web").await });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            first_stop_entered.notified(),
        )
        .await
        .expect("public stop reaches its controller");

        let apply_manager = manager.clone();
        let apply_task =
            tokio::spawn(
                async move { apply_manager.apply_config(project_path, None, false).await },
            );
        tokio::task::yield_now().await;
        assert!(
            !apply_task.is_finished(),
            "reload waits for the in-flight project transition"
        );
        assert_eq!(
            std::fs::read(&catalog_path).expect("read catalog during blocked stop"),
            catalog_before
        );
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("first.localhost")
                .is_some()
        );
        assert!(
            host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .is_empty()
        );

        release_first_stop.notify_one();
        let stop_error = tokio::time::timeout(std::time::Duration::from_secs(2), stop_task)
            .await
            .expect("public stop does not deadlock")
            .expect("public stop task joins")
            .expect_err("injected public stop failure");
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), apply_task)
            .await
            .expect("reload does not deadlock")
            .expect("reload task joins")
            .expect_err("restart stop failure must block domain publication");

        assert!(format!("{stop_error:#}").contains("injected stop failure"));
        assert!(format!("{error:#}").contains("injected stop failure"));
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            std::fs::read(&catalog_path).expect("read catalog after failed reload"),
            catalog_before
        );
        for index in [
            registry.lock().await.domain_index().clone(),
            manager.domain_index().snapshot().as_ref().clone(),
        ] {
            assert!(index.resolve("first.localhost").is_some());
            assert!(index.resolve("second.localhost").is_none());
        }
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
        let services = manager.services.lock().await;
        let service = &services["reload:web"];
        let ServiceRuntime::Controller(restored_controller) = &service.runtime_state else {
            panic!("failing controller must remain installed");
        };
        assert!(Arc::ptr_eq(restored_controller, &failing_controller));
        assert_eq!(service.config, previous_config);
        assert_eq!(service.service_config, previous_service_config);
        drop(services);
        assert!(
            host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .is_empty()
        );
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
            let mut service_a = test_service(
                previous_config.clone(),
                service_config.clone(),
                ServiceRuntime::Controller(successful_controller),
                canonical_path.clone(),
            );
            service_a.instance_id = instance_id;
            services.insert("reload:a".to_owned(), service_a);
            let mut service_z = test_service(
                previous_config,
                service_config,
                ServiceRuntime::Controller(failing_controller.clone()),
                canonical_path,
            );
            service_z.instance_id = instance_id;
            services.insert("reload:z".to_owned(), service_z);
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
        let mut service = test_service(
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
        );
        service.instance_id = instance_id;
        manager
            .services
            .lock()
            .await
            .insert("reload:web".to_owned(), service);
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
            &[expected_hosts(&[])]
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
            &[expected_hosts(&[])]
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
        let mut service = test_service(
            test_config_with_domain("busy", "busy.localhost"),
            ServiceConfig::Legacy(ExecServiceConfig::default()),
            ServiceRuntime::Controller(controller),
            canonical_path,
        );
        service.instance_id = instance_id;
        manager
            .services
            .lock()
            .await
            .insert("busy:web".to_owned(), service);

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
        let project_config = |name: &str, domain: &str, include_api: bool| {
            format!(
                r#"
[project]
name = "{name}"
domain = "{domain}"

[services.web]
type = "worker"
command = "sleep 30"
{}
"#,
                if include_api {
                    r#"
[services.api]
type = "worker"
command = "sleep 30"
"#
                } else {
                    ""
                }
            )
        };
        std::fs::write(
            first_path.join("locald.toml"),
            project_config("first", "first.localhost", false),
        )
        .expect("write first config");
        std::fs::write(
            second_path.join("locald.toml"),
            project_config("second", "second.localhost", true),
        )
        .expect("write second config");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let catalog_path = dir.path().join("catalog.json");
        let registry = Arc::new(Mutex::new(Registry::with_path(catalog_path.clone())));
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

        manager
            .apply_config(first_path.clone(), None, false)
            .await
            .expect("start first project");
        manager
            .apply_config(second_path.clone(), None, false)
            .await
            .expect("start second project");
        let first_controller = manager
            .get_service_controller("first:web")
            .await
            .expect("first controller");
        let second_web_controller = manager
            .get_service_controller("second:web")
            .await
            .expect("second web controller");
        let second_api_controller = manager
            .get_service_controller("second:api")
            .await
            .expect("second api controller");
        let catalog_before = std::fs::read(&catalog_path).expect("read existing catalog");
        let registry_before = registry.lock().await.clone();
        let domain_index_before = manager.domain_index().snapshot();
        let host_sync_calls_before = host_sync_calls
            .lock()
            .expect("recording host sync mutex poisoned")
            .clone();

        std::fs::write(
            second_path.join("locald.toml"),
            project_config("second", "first.localhost", true),
        )
        .expect("write conflicting second config");
        let error = manager
            .apply_config(second_path.clone(), None, false)
            .await
            .expect_err("conflicting project must fail");

        assert!(error.to_string().contains("first:web"));
        assert!(error.to_string().contains("second:web"));
        assert_eq!(
            std::fs::read(&catalog_path).expect("read catalog after rejected reload"),
            catalog_before
        );
        assert_eq!(*registry.lock().await, registry_before);
        assert_eq!(
            manager.domain_index().snapshot().as_ref(),
            domain_index_before.as_ref()
        );
        assert_eq!(
            *host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned"),
            host_sync_calls_before
        );
        assert!(Arc::ptr_eq(
            &first_controller,
            &manager
                .get_service_controller("first:web")
                .await
                .expect("first controller remains")
        ));
        assert!(Arc::ptr_eq(
            &second_web_controller,
            &manager
                .get_service_controller("second:web")
                .await
                .expect("second web controller remains")
        ));
        assert!(Arc::ptr_eq(
            &second_api_controller,
            &manager
                .get_service_controller("second:api")
                .await
                .expect("second api controller remains")
        ));
        for (domain, service) in [
            ("first.localhost", "first:web"),
            ("second.localhost", "second:web"),
            ("api.second.localhost", "second:api"),
        ] {
            assert!(matches!(
                manager
                    .resolve_service_by_domain(domain)
                    .await
                    .expect("published route remains"),
                locald_core::resolver::DomainResolution::Service { ref name, .. }
                    if name == service
            ));
            assert_eq!(
                crate::tls::owned_server_name(&manager.domain_index(), domain),
                Some(domain.to_owned())
            );
        }
        assert!(
            manager
                .resolve_service_by_domain("api.first.localhost")
                .await
                .is_none()
        );
        assert!(
            crate::tls::owned_server_name(&manager.domain_index(), "api.first.localhost").is_none()
        );

        manager
            .stop_project(&second_path)
            .await
            .expect("stop second project");
        for domain in ["second.localhost", "api.second.localhost"] {
            assert_eq!(
                crate::tls::owned_server_name(&manager.domain_index(), domain),
                Some(domain.to_owned()),
                "stopped project domains remain owned for the paused surface"
            );
        }
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
            instance_id: test_instance_id(),
            controller_generation: 1,
            projection_generation: 1,
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
    async fn service_restart_removed_during_transition_returns_not_found() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("project");
        std::fs::create_dir(&project_path).expect("create project directory");
        let canonical_path = ProcessManager::canonicalize_path(&project_path);

        let manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(Registry::default())),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
        )
        .expect("create process manager");

        manager.services.lock().await.insert(
            "project:web".to_owned(),
            test_service(
                LocaldConfig::default(),
                ServiceConfig::Legacy(ExecServiceConfig::default()),
                ServiceRuntime::None,
                canonical_path.clone(),
            ),
        );

        let (_, transition_lock) = manager.transition_lock_for_path(&canonical_path).await;
        let transition_guard = transition_lock.lock().await;
        let services_guard = manager.services.lock().await;

        let app = crate::api::router(manager.clone());
        let restart = tokio::spawn(async move {
            app.oneshot(
                Request::post("/services/project:web/restart")
                    .body(Body::empty())
                    .expect("build restart request"),
            )
            .await
            .expect("restart response")
        });
        tokio::task::yield_now().await;

        let services = manager.services.clone();
        let remove = tokio::spawn(async move { services.lock().await.remove("project:web") });
        tokio::task::yield_now().await;

        drop(services_guard);
        assert!(
            remove
                .await
                .expect("service removal task completes")
                .is_some()
        );
        drop(transition_guard);

        let response = restart.await.expect("restart task completes");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn prepublication_plan_covers_dot_env_changes_and_dependents() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reload-project");
        let registry = Arc::new(Mutex::new(Registry::default()));
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
        let previous_config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "reload"
domain = "first.localhost"

[services.db]
type = "worker"
command = "old-db"

[services.db.env]
TOKEN = "stable"

[services.web]
type = "worker"
command = "web"
depends_on = ["db"]

[services.web.env]
TOKEN = "stable"

[services.api]
type = "worker"
command = "api"
"#,
        )
        .expect("parse previous config");
        let next_config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "reload"
domain = "second.localhost"

[services.db]
type = "worker"
command = "new-db"

[services.db.env]
TOKEN = "stable"

[services.web]
type = "worker"
command = "web"
depends_on = ["db"]

[services.web.env]
TOKEN = "stable"

[services.api]
type = "worker"
command = "api"
"#,
        )
        .expect("parse next config");
        let running_state = RuntimeState {
            pid: Some(42),
            port: None,
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        {
            let mut services = manager.services.lock().await;
            for (name, token) in [("db", "stable"), ("web", "stable"), ("api", "old")] {
                let controller: Arc<Mutex<dyn ServiceController>> = Arc::new(Mutex::new(
                    TestController::new(format!("reload:{name}"), running_state.clone()),
                ));
                let mut service = test_service(
                    previous_config.clone(),
                    previous_config.services[name].clone(),
                    ServiceRuntime::Controller(controller),
                    project_path.clone(),
                );
                service
                    .resolved_env
                    .insert("TOKEN".to_owned(), token.to_owned());
                services.insert(format!("reload:{name}"), service);
            }
        }
        let desired_service_names = next_config
            .services
            .keys()
            .map(|name| format!("reload:{name}"))
            .collect::<HashSet<_>>();
        let dot_env_vars = HashMap::from([("TOKEN".to_owned(), "new".to_owned())]);

        let plan = manager
            .prepublication_stop_plan(
                test_instance_id(),
                &next_config,
                &dot_env_vars,
                &["db".to_owned(), "web".to_owned(), "api".to_owned()],
                &desired_service_names,
            )
            .await
            .expect("build prepublication plan");

        assert!(plan.removed_service_names.is_empty());
        assert_eq!(
            plan.restart_service_names,
            ["reload:api", "reload:web", "reload:db"]
        );
        assert!(plan.reusable_service_envs.is_empty());
    }

    #[tokio::test]
    async fn prepublication_plan_reuses_non_process_services_without_process_identity() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("managed-reload-project");
        let manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(Registry::default())),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
        )
        .expect("create process manager");
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "managed-reload"

[services.db]
type = "postgres"

[services.docs]
type = "site"
path = "docs"
"#,
        )
        .expect("parse managed service config");
        let running_state = RuntimeState {
            pid: None,
            port: None,
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        {
            let mut services = manager.services.lock().await;
            for name in ["db", "docs"] {
                let controller: Arc<Mutex<dyn ServiceController>> = Arc::new(Mutex::new(
                    TestController::new(format!("managed-reload:{name}"), running_state.clone()),
                ));
                services.insert(
                    format!("managed-reload:{name}"),
                    test_service(
                        config.clone(),
                        config.services[name].clone(),
                        ServiceRuntime::Controller(controller),
                        project_path.clone(),
                    ),
                );
            }
        }
        let desired_service_names = HashSet::from([
            "managed-reload:db".to_owned(),
            "managed-reload:docs".to_owned(),
        ]);

        let plan = manager
            .prepublication_stop_plan(
                test_instance_id(),
                &config,
                &HashMap::new(),
                &["db".to_owned(), "docs".to_owned()],
                &desired_service_names,
            )
            .await
            .expect("build managed-service reuse plan");

        assert!(plan.removed_service_names.is_empty());
        assert!(plan.restart_service_names.is_empty());
        assert_eq!(plan.reusable_service_envs.len(), 2);
        assert!(plan.reusable_service_envs.contains_key("managed-reload:db"));
        assert!(
            plan.reusable_service_envs
                .contains_key("managed-reload:docs")
        );
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

        assert_eq!(
            manager.hosts_domains(),
            expected_hosts(&["current.localhost"])
        );
        manager.sync_hosts().await.expect("synchronize hosts");
        assert_eq!(
            calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[expected_hosts(&["current.localhost"])]
        );
    }

    #[tokio::test]
    async fn same_name_instances_keep_runtime_routing_instance_scoped() {
        let dir = tempdir().expect("create temporary directory");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
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
        let first_instance = test_instance_id();
        let second_instance = alternate_test_instance_id();
        let first_path = dir.path().join("first");
        let second_path = dir.path().join("second");
        let service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        let first_config = config_with_services(
            ProjectConfig {
                name: "app".to_owned(),
                domain: Some("first.app.localhost".to_owned()),
                ..Default::default()
            },
            &["web"],
        );
        let second_config = config_with_services(
            ProjectConfig {
                name: "app".to_owned(),
                domain: Some("second.app.localhost".to_owned()),
                ..Default::default()
            },
            &["web"],
        );
        let running_state = RuntimeState {
            pid: Some(42),
            port: Some(4242),
            status: ServiceState::Running,
            health_status: HealthStatus::Healthy,
        };
        let first_controller: Arc<Mutex<dyn ServiceController>> = Arc::new(Mutex::new(
            TestController::new("app:web", running_state.clone()),
        ));
        let mut first_service = test_service(
            first_config,
            service_config.clone(),
            ServiceRuntime::Controller(first_controller),
            first_path.clone(),
        );
        first_service.instance_id = first_instance;
        manager
            .services
            .lock()
            .await
            .insert("app:web".to_owned(), first_service);

        let desired_names = HashSet::from(["app:web".to_owned()]);
        let error = manager
            .prepublication_stop_plan(
                second_instance,
                &second_config,
                &HashMap::new(),
                &["web".to_owned()],
                &desired_names,
            )
            .await
            .expect_err("a live same-name instance cannot be reused or overwritten");
        assert!(
            error
                .to_string()
                .contains("still loaded by project instance")
        );

        manager
            .broadcast_log(
                first_instance,
                1,
                LogEntry {
                    timestamp: 0,
                    service: "app:web".to_owned(),
                    stream: locald_core::ipc::LogStream::Stdout,
                    message: "first-instance history".to_owned(),
                },
            )
            .await;
        assert_eq!(manager.get_recent_logs().len(), 1);
        manager.services.lock().await.remove("app:web");

        install_test_claim_for_instance(&manager, first_instance, "first.app.localhost", "app:web");
        install_test_claim_for_instance(
            &manager,
            second_instance,
            "second.app.localhost",
            "app:web",
        );
        let second_controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::new("app:web", running_state)));
        let mut second_service = test_service(
            second_config,
            service_config,
            ServiceRuntime::Controller(second_controller),
            second_path,
        );
        second_service.instance_id = second_instance;
        manager.clear_foreign_log_buffer("app:web", second_instance);
        manager
            .services
            .lock()
            .await
            .insert("app:web".to_owned(), second_service);
        assert!(manager.get_recent_logs().is_empty());

        assert!(matches!(
            manager
                .resolve_service_by_domain("first.app.localhost")
                .await,
            Some(locald_core::resolver::DomainResolution::OwnershipOnly)
        ));
        assert!(matches!(
            manager
                .resolve_service_by_domain("second.app.localhost")
                .await,
            Some(locald_core::resolver::DomainResolution::Service {
                ref name,
                port: Some(4242),
                status: ServiceState::Running,
            }) if name == "app:web"
        ));

        let statuses = manager.list().await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].domain.as_deref(), Some("second.app.localhost"));
        let inspection = manager.inspect("app:web").await.expect("inspect service");
        assert_eq!(inspection["domain"], "second.app.localhost");

        manager
            .broadcast_log(
                first_instance,
                1,
                LogEntry {
                    timestamp: 1,
                    service: "app:web".to_owned(),
                    stream: locald_core::ipc::LogStream::Stdout,
                    message: "stale first-instance log".to_owned(),
                },
            )
            .await;
        assert!(manager.get_recent_logs().is_empty());
        manager
            .broadcast_log(
                second_instance,
                0,
                LogEntry {
                    timestamp: 2,
                    service: "app:web".to_owned(),
                    stream: locald_core::ipc::LogStream::Stdout,
                    message: "stale second-instance controller log".to_owned(),
                },
            )
            .await;
        assert!(manager.get_recent_logs().is_empty());
        manager
            .broadcast_log(
                second_instance,
                1,
                LogEntry {
                    timestamp: 3,
                    service: "app:web".to_owned(),
                    stream: locald_core::ipc::LogStream::Stdout,
                    message: "current second-instance log".to_owned(),
                },
            )
            .await;
        assert_eq!(manager.get_recent_logs().len(), 1);
        manager
            .services
            .lock()
            .await
            .get_mut("app:web")
            .expect("loaded service")
            .runtime_state = ServiceRuntime::None;
        manager
            .broadcast_log(
                second_instance,
                1,
                LogEntry {
                    timestamp: 4,
                    service: "app:web".to_owned(),
                    stream: locald_core::ipc::LogStream::Stdout,
                    message: "stopped controller log".to_owned(),
                },
            )
            .await;
        assert_eq!(manager.get_recent_logs().len(), 1);
    }

    #[tokio::test]
    async fn service_env_rejects_a_foreign_instance_dependency() {
        let dir = tempdir().expect("create temporary directory");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
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
        let first_instance = test_instance_id();
        let second_instance = alternate_test_instance_id();
        let mut config = config_with_services(
            ProjectConfig {
                name: "app".to_owned(),
                ..Default::default()
            },
            &["web", "db"],
        );
        let ServiceConfig::Legacy(web_config) = config
            .services
            .get_mut("web")
            .expect("web service configuration")
        else {
            panic!("web is a legacy exec service");
        };
        web_config
            .common
            .env
            .insert("DEPENDENCY_URL".to_owned(), "${services.db.url}".to_owned());
        let web_service_config = config.services["web"].clone();
        let db_service_config = config.services["db"].clone();
        let mut web_service = test_service(
            config.clone(),
            web_service_config,
            ServiceRuntime::None,
            dir.path().join("first"),
        );
        web_service.instance_id = first_instance;
        let db_controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::new(
                "app:db",
                RuntimeState {
                    pid: Some(42),
                    port: Some(5432),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            )));
        let mut db_service = test_service(
            config,
            db_service_config,
            ServiceRuntime::Controller(db_controller),
            dir.path().join("second"),
        );
        db_service.instance_id = second_instance;
        manager.services.lock().await.extend([
            ("app:web".to_owned(), web_service),
            ("app:db".to_owned(), db_service),
        ]);

        let error = manager
            .get_service_env("app:web")
            .await
            .expect_err("foreign dependency must not enter the command environment");
        assert!(error.to_string().contains("belongs to project instance"));
        assert!(error.to_string().contains(&first_instance.to_string()));
        assert!(error.to_string().contains(&second_instance.to_string()));
    }

    #[tokio::test]
    async fn stopped_controller_drops_inflight_metrics() {
        let dir = tempdir().expect("create temporary directory");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
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
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingMetricsController {
                entered: entered.clone(),
                release: release.clone(),
                state: RuntimeState {
                    pid: Some(42),
                    port: Some(4242),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            }));
        manager.services.lock().await.insert(
            "app:web".to_owned(),
            test_service(
                config_with_services(
                    ProjectConfig {
                        name: "app".to_owned(),
                        ..Default::default()
                    },
                    &["web"],
                ),
                ServiceConfig::Legacy(ExecServiceConfig::default()),
                ServiceRuntime::Controller(controller),
                dir.path().join("app"),
            ),
        );
        let mut events = manager.event_sender.subscribe();

        let metrics_task = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager.collect_metrics().await;
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("metrics collection starts");
        manager
            .services
            .lock()
            .await
            .get_mut("app:web")
            .expect("loaded service")
            .runtime_state = ServiceRuntime::None;
        release.notify_one();
        metrics_task.await.expect("metrics task");

        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn projection_change_suppresses_inflight_manager_status_event() {
        let dir = tempdir().expect("create temporary directory");
        let state_manager = Arc::new(StateManager::with_path(dir.path().join("state.json")));
        let registry = Arc::new(Mutex::new(Registry::default()));
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
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingStatusController {
                entered: entered.clone(),
                release: release.clone(),
                state: RuntimeState {
                    pid: Some(42),
                    port: Some(4242),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            }));
        manager.services.lock().await.insert(
            "app:web".to_owned(),
            test_service(
                config_with_services(
                    ProjectConfig {
                        name: "app".to_owned(),
                        ..Default::default()
                    },
                    &["web"],
                ),
                ServiceConfig::Legacy(ExecServiceConfig::default()),
                ServiceRuntime::Controller(controller),
                dir.path().join("app"),
            ),
        );
        let mut events = manager.event_sender.subscribe();

        let status_task = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager.broadcast_service_update("app:web").await;
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("status construction starts");
        {
            let mut services = manager.services.lock().await;
            let service = services.get_mut("app:web").expect("loaded service");
            service.warnings.push("new projection".to_owned());
            ProcessManager::advance_service_projection(service);
        }
        release.notify_one();
        status_task.await.expect("status task");

        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
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
