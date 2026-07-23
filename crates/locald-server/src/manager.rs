#![allow(clippy::collapsible_if)]
#![allow(clippy::option_if_let_else)]
use crate::config_loader::ConfigLoader;
use crate::health::{HealthMonitor, ReadinessRequirement};
use crate::lifecycle_migration::{
    availability_demand_for_attachment_source, manual_cli_session_demand,
    plan_project_lifecycle_migration,
};
use crate::lifecycle_transaction::{
    AttachmentTransactionImages, CatalogTransactionImages, LegacyV1File, LifecycleJournal,
    LifecycleTransaction, LifecycleTransactionKind, LifecycleTransactionPhase,
    validate_attachment_authority,
};
use crate::plugins;
use crate::port_allocator::PortAllocator;
use crate::runtime::Runtime;
use crate::state::StateManager;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use futures_util::StreamExt;
use locald_core::attachments::{
    Attachment, AttachmentCompatibilityEvidence, AttachmentSource, AttachmentStore,
    AttachmentStoreSnapshot, ManualCliSession, ProjectFilter, ProjectListEntry, ProjectSection,
    ProjectStatusInfo,
};
use locald_core::config::{LocaldConfig, ServiceConfig, TypedServiceConfig};
use locald_core::ipc::{
    BootEvent, EnsureProjectResult, EnsureProjectState, EnsuredServiceStatus, Event, LogEntry,
    ServiceStatus,
};
use locald_core::registry::Registry;
use locald_core::resolver::ServiceResolver;
use locald_core::service::{ServiceContext, ServiceController, ServiceFactory};
use locald_core::state::{
    HealthSource, HealthStatus, PersistedProcessIdentity, PersistedServiceState, ServerState,
    ServiceState,
};
use locald_core::{
    AvailabilityBatch, AvailabilityBatchOperation, AvailabilityError, AvailabilityStore,
    CatalogError, CatalogPresence, Clock, ConvergenceDecision, DemandKey, DemandKind, DomainClaim,
    DomainName, DomainTarget, EnsureDemandResult, ProjectAvailability, ProjectDiscovery,
    ProjectInstanceId, SharedDomainIndex, SystemClock, availability_path,
    sanitize_project_name_for_dns, sanitize_service_name_for_dns,
};
use nix::sys::signal::Signal;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::SystemTime;
use tokio::sync::{Mutex, Notify, OwnedMutexGuard, RwLock, broadcast};
use tracing::{error, info, warn};

const LOG_BUFFER_SIZE: usize = 2000;
const SERVICE_READINESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const SERVICE_READINESS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

// This scope is entered only while holding a shared availability-transition
// permit. It lets a transition admitted before shutdown finish its semantic
// convergence while shutdown's exclusive permit waits to begin teardown.
tokio::task_local! {
    static AVAILABILITY_TRANSITION_ADMITTED: usize;
}

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

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("availability transitions cannot be nested")]
struct ReentrantAvailabilityTransition;

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

struct SandboxHostSyncer;

#[async_trait::async_trait]
impl HostSyncer for SandboxHostSyncer {
    async fn sync(&self, _domains: Vec<String>) -> Result<()> {
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
    stopped_service_projections: HashMap<String, (ServiceConfig, Option<HashMap<String, String>>)>,
}

#[derive(Debug)]
struct CataloguedLifecycleTarget {
    instance_id: ProjectInstanceId,
    path: PathBuf,
    catalog_base: Registry,
    catalog_target: Registry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecycleTargetIdentity {
    Catalogued(ProjectInstanceId),
    UnregisteredPhysical(ProjectInstanceId),
    UnresolvedLegacy,
    Ambiguous,
}

#[derive(Debug)]
enum LifecycleTargetResolution {
    Catalogued(Box<CataloguedLifecycleTarget>),
    UnregisteredPhysical { instance_id: ProjectInstanceId },
    UnresolvedLegacy,
    Ambiguous,
}

#[derive(Clone, Copy, Debug)]
enum ConfigIdentityExpectation {
    Existing(ProjectInstanceId),
    Initial(ConfigPhysicalIdentity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigPhysicalIdentity {
    Git(ProjectInstanceId),
    NonGit,
}

impl ConfigPhysicalIdentity {
    fn from_discovery(discovery: &ProjectDiscovery) -> Self {
        match discovery {
            ProjectDiscovery::Git { resolved, .. } => {
                Self::Git(resolved.identity.project_instance_id)
            }
            ProjectDiscovery::NonGit { .. } => Self::NonGit,
        }
    }
}

impl ConfigIdentityExpectation {
    const fn expected_instance_id(self) -> Option<ProjectInstanceId> {
        match self {
            Self::Existing(instance_id)
            | Self::Initial(ConfigPhysicalIdentity::Git(instance_id)) => Some(instance_id),
            Self::Initial(ConfigPhysicalIdentity::NonGit) => None,
        }
    }

    const fn requires_existing_catalog(self) -> bool {
        matches!(self, Self::Existing(_))
    }

    const fn is_initial(self) -> bool {
        matches!(self, Self::Initial(_))
    }

    const fn initial_physical_identity(self) -> Option<ConfigPhysicalIdentity> {
        match self {
            Self::Initial(identity) => Some(identity),
            Self::Existing(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AvailabilityManagementState {
    Managed,
    PendingInitial,
    LegacyUnmanaged,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct ConfigPublicationHook {
    reached: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}

impl LifecycleTargetResolution {
    fn identity(&self) -> LifecycleTargetIdentity {
        match self {
            Self::Catalogued(target) => LifecycleTargetIdentity::Catalogued(target.instance_id),
            Self::UnregisteredPhysical { instance_id } => {
                LifecycleTargetIdentity::UnregisteredPhysical(*instance_id)
            }
            Self::UnresolvedLegacy => LifecycleTargetIdentity::UnresolvedLegacy,
            Self::Ambiguous => LifecycleTargetIdentity::Ambiguous,
        }
    }

    fn catalogued_instance_id(&self) -> Option<ProjectInstanceId> {
        match self {
            Self::Catalogued(target) => Some(target.instance_id),
            Self::UnregisteredPhysical { .. } | Self::UnresolvedLegacy | Self::Ambiguous => None,
        }
    }

    fn into_catalogued(self) -> Option<CataloguedLifecycleTarget> {
        match self {
            Self::Catalogued(target) => Some(*target),
            Self::UnregisteredPhysical { .. } | Self::UnresolvedLegacy | Self::Ambiguous => None,
        }
    }

    fn into_mutation_target(
        self,
        project_path: &Path,
        operation: &str,
    ) -> Result<Option<CataloguedLifecycleTarget>> {
        match self {
            Self::Catalogued(target) => Ok(Some(*target)),
            Self::UnresolvedLegacy => Ok(None),
            Self::UnregisteredPhysical { instance_id } => anyhow::bail!(
                "{operation} requires the physical project at `{}` to be registered as {instance_id}; run `locald up` from that worktree first",
                project_path.display()
            ),
            Self::Ambiguous => anyhow::bail!(
                "{operation} cannot resolve `{}` because it matches multiple catalogued project instances",
                project_path.display()
            ),
        }
    }
}

struct PendingInitialAvailabilityGuard {
    instance_id: ProjectInstanceId,
    pending: Arc<StdMutex<HashSet<ProjectInstanceId>>>,
}

struct ConfigApplyOutcome {
    instance_id: ProjectInstanceId,
    pending_initial: Option<PendingInitialAvailabilityGuard>,
}

impl Drop for PendingInitialAvailabilityGuard {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.instance_id);
    }
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
    runtime: Arc<Mutex<()>>,
}

impl AvailabilityCoordinator {
    fn new() -> Self {
        Self {
            runtime: Arc::new(Mutex::new(())),
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
pub(crate) struct RuntimeRestorePlan;

#[derive(Clone)]
struct SharedAvailabilityClock(Arc<dyn Clock>);

impl SharedAvailabilityClock {
    fn system() -> Self {
        Self(Arc::new(SystemClock))
    }

    #[cfg(test)]
    fn new(clock: impl Clock + 'static) -> Self {
        Self(Arc::new(clock))
    }
}

impl fmt::Debug for SharedAvailabilityClock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedAvailabilityClock")
    }
}

impl Clock for SharedAvailabilityClock {
    fn now(&self) -> SystemTime {
        self.0.now()
    }
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
    proxy_ports_changed: Arc<Notify>,
    watchers: Arc<Mutex<HashMap<PathBuf, RecommendedWatcher>>>,
    registry: Arc<Mutex<Registry>>,
    domain_index: SharedDomainIndex,
    attachments: Arc<Mutex<AttachmentStore>>,
    attachment_transition_lock: Arc<Mutex<()>>,
    lifecycle_publication_lock: Arc<Mutex<()>>,
    lifecycle_recovery_required: Arc<AtomicBool>,
    pending_initial_availability: Arc<StdMutex<HashSet<ProjectInstanceId>>>,
    health_monitor: HealthMonitor,
    factories: Vec<Arc<dyn ServiceFactory>>,
    hosts_sync_guard: ConcurrencyGuard,
    host_syncer: Arc<dyn HostSyncer>,
    port_allocator: PortAllocator,
    config_transition_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    availability_coordinators: Arc<Mutex<HashMap<ProjectInstanceId, Arc<AvailabilityCoordinator>>>>,
    availability_transition_gate: Arc<RwLock<()>>,
    pending_config_reloads: Arc<Mutex<HashSet<ProjectInstanceId>>>,
    forgotten_reload_paths: Arc<Mutex<HashSet<PathBuf>>>,
    // A named service stop is a daemon-memory, one-off runtime override. It
    // survives automatic availability convergence without rewriting project
    // availability, and explicit lifecycle actions or daemon restart clear it.
    service_stop_suppressions: Arc<Mutex<HashSet<(ProjectInstanceId, String)>>>,
    availability_data_dir: PathBuf,
    availability_clock: SharedAvailabilityClock,
    lifecycle_journal: LifecycleJournal,
    runtime_projection_lock: Arc<Mutex<()>>,
    state_persistence_lock: Arc<Mutex<()>>,
    next_controller_generation: Arc<AtomicU64>,
    shutting_down: Arc<AtomicBool>,
    #[cfg(test)]
    config_publication_hook: Arc<StdMutex<Option<ConfigPublicationHook>>>,
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

    fn availability_now(&self) -> SystemTime {
        self.availability_clock.now()
    }

    async fn load_availability(
        &self,
        instance_id: ProjectInstanceId,
    ) -> std::result::Result<AvailabilityStore<SharedAvailabilityClock>, AvailabilityError> {
        AvailabilityStore::load_with_clock(
            &self.availability_data_dir,
            instance_id,
            self.availability_clock.clone(),
        )
        .await
    }

    fn availability_transition_key(&self) -> usize {
        Arc::as_ptr(&self.availability_transition_gate) as usize
    }

    fn ensure_accepting_new_lifecycle_request(&self) -> Result<()> {
        if self.is_shutting_down() {
            Err(DaemonShuttingDown.into())
        } else {
            Ok(())
        }
    }

    fn ensure_accepting_lifecycle_requests(&self) -> Result<()> {
        let availability_transition_admitted = AVAILABILITY_TRANSITION_ADMITTED
            .try_with(|admitted| *admitted == self.availability_transition_key())
            .unwrap_or(false);
        if self.is_shutting_down() && !availability_transition_admitted {
            Err(DaemonShuttingDown.into())
        } else {
            Ok(())
        }
    }

    fn ensure_lifecycle_publication_available(&self) -> Result<()> {
        anyhow::ensure!(
            !self
                .lifecycle_recovery_required
                .load(AtomicOrdering::Acquire),
            "lifecycle state requires daemon restart so the active transaction can be recovered before more lifecycle changes"
        );
        Ok(())
    }

    /// Admit one availability transition and drain it before teardown.
    ///
    /// The transition may call lifecycle helpers on this manager after the
    /// shutdown request is published. It must not call [`Self::shutdown`],
    /// which waits for this method's own shared permit. Nested availability
    /// transitions are rejected before they can request another read permit.
    async fn run_admitted_availability_transition<Output, MakeTransition, Transition>(
        &self,
        make_transition: MakeTransition,
    ) -> Result<Output>
    where
        MakeTransition: FnOnce() -> Transition,
        Transition: std::future::Future<Output = Result<Output>>,
    {
        let reentrant = AVAILABILITY_TRANSITION_ADMITTED
            .try_with(|admitted| *admitted == self.availability_transition_key())
            .unwrap_or(false);
        if reentrant {
            return Err(ReentrantAvailabilityTransition.into());
        }
        let transition_guard = self.availability_transition_gate.read().await;
        self.ensure_accepting_new_lifecycle_request()?;
        let result = AVAILABILITY_TRANSITION_ADMITTED
            .scope(self.availability_transition_key(), make_transition())
            .await;
        drop(transition_guard);
        result
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
        #[cfg(test)]
        let availability_data_dir = state_manager
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("locald-test-data");
        #[cfg(not(test))]
        let availability_data_dir = locald_core::storage::data_dir();
        Self::new_with_availability_data_dir(
            notify_socket_path,
            state_manager,
            registry,
            attachments,
            external_log_sender,
            availability_data_dir,
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
        Self::new_with_availability_data_dir_and_clock(
            notify_socket_path,
            state_manager,
            registry,
            attachments,
            external_log_sender,
            availability_data_dir,
            SharedAvailabilityClock::system(),
        )
    }

    fn new_with_availability_data_dir_and_clock(
        notify_socket_path: PathBuf,
        state_manager: Arc<StateManager>,
        registry: Arc<Mutex<Registry>>,
        attachments: Arc<Mutex<AttachmentStore>>,
        external_log_sender: Option<broadcast::Sender<LogEntry>>,
        availability_data_dir: PathBuf,
        availability_clock: SharedAvailabilityClock,
    ) -> Result<Self> {
        let (tx, _) = if let Some(tx) = external_log_sender {
            (tx, broadcast::channel(1).1) // Dummy receiver
        } else {
            broadcast::channel(100)
        };
        let (event_tx, _) = broadcast::channel(100);

        let services = Arc::new(Mutex::new(HashMap::new()));
        let proxy_ports = Arc::new(Mutex::new((None, None)));
        let proxy_ports_changed = Arc::new(Notify::new());

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

        let lifecycle_journal = LifecycleJournal::at(&availability_data_dir);
        Ok(Self {
            services,
            log_sender: tx,
            event_sender: event_tx,
            log_buffers: Arc::new(StdMutex::new(HashMap::new())),
            state_manager,
            runtime,
            proxy_ports,
            proxy_ports_changed,
            watchers: Arc::new(Mutex::new(HashMap::new())),
            registry,
            domain_index,
            attachments,
            attachment_transition_lock: Arc::new(Mutex::new(())),
            lifecycle_publication_lock: Arc::new(Mutex::new(())),
            lifecycle_recovery_required: Arc::new(AtomicBool::new(false)),
            pending_initial_availability: Arc::new(StdMutex::new(HashSet::new())),
            health_monitor,
            factories,
            hosts_sync_guard: ConcurrencyGuard::new(),
            host_syncer: Arc::new(DefaultHostSyncer),
            port_allocator: PortAllocator::new(),
            config_transition_locks: Arc::new(Mutex::new(HashMap::new())),
            availability_coordinators: Arc::new(Mutex::new(HashMap::new())),
            availability_transition_gate: Arc::new(RwLock::new(())),
            pending_config_reloads: Arc::new(Mutex::new(HashSet::new())),
            forgotten_reload_paths: Arc::new(Mutex::new(HashSet::new())),
            service_stop_suppressions: Arc::new(Mutex::new(HashSet::new())),
            availability_data_dir,
            availability_clock,
            lifecycle_journal,
            runtime_projection_lock: Arc::new(Mutex::new(())),
            state_persistence_lock: Arc::new(Mutex::new(())),
            next_controller_generation: Arc::new(AtomicU64::new(1)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            config_publication_hook: Arc::new(StdMutex::new(None)),
        })
    }

    #[cfg(test)]
    pub fn set_host_syncer(&mut self, syncer: Arc<dyn HostSyncer>) {
        self.host_syncer = syncer;
    }

    #[cfg(test)]
    fn set_config_publication_hook(&self, hook: ConfigPublicationHook) {
        *self
            .config_publication_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    #[cfg(test)]
    async fn wait_at_config_publication_hook(&self) {
        let hook = self
            .config_publication_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(hook) = hook {
            hook.reached.notify_one();
            hook.resume.notified().await;
        }
    }

    pub(crate) fn use_sandbox_host_syncer(&mut self) {
        self.host_syncer = Arc::new(SandboxHostSyncer);
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
        self.proxy_ports_changed.notify_waiters();
    }

    pub async fn set_https_port(&self, port: Option<u16>) {
        self.proxy_ports.lock().await.1 = port;
        self.proxy_ports_changed.notify_waiters();
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
        self.hosts_domain_names()
            .into_iter()
            .map(|domain| domain.to_string())
            .collect()
    }

    /// Return validated exact names for a privileged hosts-file writer.
    #[must_use]
    pub fn hosts_domain_names(&self) -> Vec<DomainName> {
        let index = self.domain_index.snapshot();
        #[cfg(target_os = "macos")]
        {
            index.macos_hosts_domain_names()
        }
        #[cfg(not(target_os = "macos"))]
        {
            index.hosts_domain_names()
        }
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

    fn service_activation_closure(
        config: &LocaldConfig,
        selected_service: &str,
    ) -> Result<HashSet<String>> {
        let selected = Self::configured_service_name(selected_service, config);
        anyhow::ensure!(
            config.services.contains_key(selected),
            "service `{selected_service}` is not present in the current project configuration"
        );

        let mut pending = vec![selected.to_owned()];
        let mut local_names = HashSet::new();
        while let Some(name) = pending.pop() {
            if !local_names.insert(name.clone()) {
                continue;
            }
            let service = config.services.get(&name).with_context(|| {
                format!("service `{name}` disappeared from its dependency graph")
            })?;
            pending.extend(service.depends_on().iter().cloned());
        }

        Ok(local_names
            .into_iter()
            .map(|name| format!("{}:{name}", config.project.name))
            .collect())
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
        let mut stopped_service_projections = HashMap::new();

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
                        service.health_status,
                    )
                })
            };

            let (has_controller, is_up_to_date, is_stopped_projection) = match service_snapshot {
                Some((loaded_instance, loaded_path, _, _, Some(_), _))
                    if loaded_instance != instance_id =>
                {
                    anyhow::bail!(
                        "service `{full_name}` is still loaded by project instance {loaded_instance} at {}; stop that project before starting instance {instance_id}",
                        loaded_path.display()
                    );
                }
                Some((_, _, current_config, current_env, Some(controller), health_status)) => {
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
                            && health_status == HealthStatus::Healthy
                            && has_durable_process_ownership
                            && current_config == *service_config
                            && environment_matches,
                        false,
                    )
                }
                Some((_, _, _, _, None, _)) => (false, false, true),
                None => (false, false, false),
            };

            if is_stopped_projection {
                stopped_service_projections.insert(
                    full_name.clone(),
                    (service_config.clone(), resolved_env.clone()),
                );
            }

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
            stopped_service_projections,
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

        // Runtime snapshots authorize stale-process cleanup only. Availability
        // migration establishes every future restart decision before IPC is
        // admitted, so no Running bit is carried forward as lifecycle intent.
        Ok(RuntimeRestorePlan)
    }

    /// Restore only daemon-owned availability policy.
    pub(crate) async fn restore_policy_owned_projects(&self, _plan: RuntimeRestorePlan) {
        if self.is_shutting_down() {
            return;
        }
        self.converge_all_project_availability().await;

        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        if !self.is_shutting_down() {
            self.persist_state().await;
        }
    }

    pub async fn handle_notify(&self, pid: u32) {
        let candidates = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter_map(|(name, service)| match &service.runtime_state {
                    ServiceRuntime::Controller(controller) => Some((
                        name.clone(),
                        service.instance_id,
                        service.controller_generation,
                        controller.clone(),
                    )),
                    ServiceRuntime::None => None,
                })
                .collect::<Vec<_>>()
        };

        for (name, instance_id, controller_generation, controller) in candidates {
            let state = controller.lock().await.read_state().await;
            if state.pid != Some(pid) {
                continue;
            }

            {
                let services = self.services.lock().await;
                let Some(service) = services.get(&name).filter(|service| {
                    service.instance_id == instance_id
                        && service.controller_generation == controller_generation
                        && matches!(
                            &service.runtime_state,
                            ServiceRuntime::Controller(current) if Arc::ptr_eq(current, &controller)
                        )
                }) else {
                    continue;
                };
                let Ok(requirement) =
                    ReadinessRequirement::for_service(&service.service_config, service.sticky_port)
                else {
                    warn!(
                        "Ignoring readiness notification for service {name} with an invalid readiness contract"
                    );
                    return;
                };
                // READY=1 remains observable compatibility input, but it
                // cannot replace the service's authoritative readiness
                // contract. Endpoint services prove their assigned endpoint;
                // portless workers prove owned-process liveness.
                info!(
                    "Ignoring readiness notification for service {name}; its {} remains authoritative",
                    requirement.description()
                );
            }
            return;
        }
    }

    fn readiness_is_satisfied(
        requirement: &ReadinessRequirement,
        projected_health: HealthStatus,
        runtime_status: ServiceState,
        controller_health: HealthStatus,
        owned_process_id: Option<u32>,
    ) -> bool {
        match requirement {
            ReadinessRequirement::ProcessRunning => {
                runtime_status == ServiceState::Running && owned_process_id.is_some()
            }
            ReadinessRequirement::ControllerAndAssignedPortTcp { .. } => {
                projected_health == HealthStatus::Healthy
                    && controller_health == HealthStatus::Healthy
            }
            ReadinessRequirement::ExplicitHttp { .. }
            | ReadinessRequirement::ExplicitTcp { .. }
            | ReadinessRequirement::ExplicitCommand { .. }
            | ReadinessRequirement::AssignedPortTcp { .. } => {
                projected_health == HealthStatus::Healthy
            }
        }
    }

    const fn readiness_requires_controller_health(requirement: &ReadinessRequirement) -> bool {
        matches!(
            requirement,
            ReadinessRequirement::ControllerAndAssignedPortTcp { .. }
        )
    }

    async fn wait_for_health(&self, name: &str, instance_id: ProjectInstanceId) -> Result<()> {
        let deadline = tokio::time::Instant::now() + SERVICE_READINESS_TIMEOUT;

        loop {
            self.availability_allows_inflight_transition(instance_id)
                .await?;
            let (controller, controller_generation, requirement) = {
                let services = self.services.lock().await;
                let service = services
                    .get(name)
                    .with_context(|| format!("service `{name}` disappeared during readiness"))?;
                anyhow::ensure!(
                    service.instance_id == instance_id,
                    "service `{name}` changed project instances during readiness"
                );
                let requirement =
                    ReadinessRequirement::for_service(&service.service_config, service.sticky_port)
                        .with_context(|| {
                            format!("service `{name}` has an invalid readiness contract")
                        })?;
                let controller = match &service.runtime_state {
                    ServiceRuntime::Controller(controller) => controller.clone(),
                    ServiceRuntime::None => {
                        anyhow::bail!("service `{name}` has no runtime during readiness")
                    }
                };
                (controller, service.controller_generation, requirement)
            };
            let (state, owned_process_id) = {
                let controller = controller.lock().await;
                (controller.read_state().await, controller.owned_process_id())
            };
            let observation = {
                let mut services = self.services.lock().await;
                let service = services
                    .get_mut(name)
                    .with_context(|| format!("service `{name}` disappeared during readiness"))?;
                anyhow::ensure!(
                    service.instance_id == instance_id,
                    "service `{name}` changed project instances during readiness"
                );
                anyhow::ensure!(
                    service.controller_generation == controller_generation
                        && matches!(
                            &service.runtime_state,
                            ServiceRuntime::Controller(current) if Arc::ptr_eq(current, &controller)
                        ),
                    "service `{name}` changed controllers during readiness"
                );
                let current_requirement =
                    ReadinessRequirement::for_service(&service.service_config, service.sticky_port)
                        .with_context(|| {
                            format!("service `{name}` has an invalid readiness contract")
                        })?;
                anyhow::ensure!(
                    current_requirement == requirement,
                    "service `{name}` changed its readiness contract during readiness"
                );

                if state.status == ServiceState::Stopped {
                    Some(Err(format!(
                        "runtime stopped before satisfying {}; last readiness was {} ({})",
                        requirement.description(),
                        service.health_status,
                        service.health_source
                    )))
                } else if Self::readiness_requires_controller_health(&requirement)
                    && state.health_status == HealthStatus::Unhealthy
                {
                    Some(Err(format!(
                        "controller reported unhealthy before satisfying {}",
                        requirement.description()
                    )))
                } else {
                    let controller_ready = Self::readiness_is_satisfied(
                        &requirement,
                        service.health_status,
                        state.status,
                        state.health_status,
                        owned_process_id,
                    );
                    if controller_ready {
                        if service.health_status != HealthStatus::Healthy {
                            service.health_status = HealthStatus::Healthy;
                            service.health_source = HealthSource::Explicit;
                            Self::advance_service_projection(service);
                        }
                        Some(Ok(()))
                    } else {
                        None
                    }
                }
            };

            match observation {
                Some(Ok(())) => {
                    self.broadcast_service_update(name).await;
                    return Ok(());
                }
                Some(Err(reason)) => {
                    self.mark_readiness_failed(name, instance_id).await?;
                    anyhow::bail!("service `{name}` failed readiness: {reason}");
                }
                None => {}
            }

            tokio::select! {
                () = tokio::time::sleep_until(deadline) => {
                    let (controller, controller_generation, requirement) = {
                        let services = self.services.lock().await;
                        let service = services
                            .get(name)
                            .with_context(|| format!("service `{name}` disappeared during readiness"))?;
                        anyhow::ensure!(
                            service.instance_id == instance_id,
                            "service `{name}` changed project instances during readiness"
                        );
                        let requirement = ReadinessRequirement::for_service(
                            &service.service_config,
                            service.sticky_port,
                        )
                        .with_context(|| format!("service `{name}` has an invalid readiness contract"))?;
                        let controller = match &service.runtime_state {
                            ServiceRuntime::Controller(controller) => controller.clone(),
                            ServiceRuntime::None => {
                                anyhow::bail!("service `{name}` has no runtime at its readiness deadline")
                            }
                        };
                        (controller, service.controller_generation, requirement)
                    };
                    let (state, owned_process_id) = {
                        let controller = controller.lock().await;
                        (controller.read_state().await, controller.owned_process_id())
                    };
                    let (ready, last_status, last_source, readiness_changed) = {
                        let mut services = self.services.lock().await;
                        let service = services
                            .get_mut(name)
                            .with_context(|| format!("service `{name}` disappeared during readiness"))?;
                        anyhow::ensure!(
                            service.instance_id == instance_id,
                            "service `{name}` changed project instances during readiness"
                        );
                        anyhow::ensure!(
                            service.controller_generation == controller_generation
                                && matches!(
                                    &service.runtime_state,
                                    ServiceRuntime::Controller(current) if Arc::ptr_eq(current, &controller)
                                ),
                            "service `{name}` changed controllers during readiness"
                        );
                        let current_requirement = ReadinessRequirement::for_service(
                            &service.service_config,
                            service.sticky_port,
                        )
                        .with_context(|| format!("service `{name}` has an invalid readiness contract"))?;
                        anyhow::ensure!(
                            current_requirement == requirement,
                            "service `{name}` changed its readiness contract during readiness"
                        );
                        let last_status = service.health_status;
                        let last_source = service.health_source;
                        let ready = state.status != ServiceState::Stopped
                            && Self::readiness_is_satisfied(
                            &requirement,
                            service.health_status,
                            state.status,
                            state.health_status,
                            owned_process_id,
                        );
                        if ready && service.health_status != HealthStatus::Healthy {
                            service.health_status = HealthStatus::Healthy;
                            service.health_source = HealthSource::Explicit;
                            Self::advance_service_projection(service);
                        }
                        let readiness_changed = !ready
                            && (service.health_status != HealthStatus::Unhealthy
                                || service.health_source != requirement.health_source());
                        if readiness_changed {
                            service.health_status = HealthStatus::Unhealthy;
                            service.health_source = requirement.health_source();
                            Self::advance_service_projection(service);
                        }
                        (ready, last_status, last_source, readiness_changed)
                    };
                    if ready {
                        self.broadcast_service_update(name).await;
                        return Ok(());
                    }
                    let (runtime_status, controller_health) = (state.status, state.health_status);
                    self.persist_state_checked().await?;
                    if readiness_changed {
                        self.broadcast_service_update(name).await;
                    }
                    anyhow::bail!(
                        "service `{name}` timed out after {}s waiting for {}; last runtime was {} with controller health {}, and last readiness was {} ({})",
                        SERVICE_READINESS_TIMEOUT.as_secs(),
                        requirement.description(),
                        runtime_status,
                        controller_health,
                        last_status,
                        last_source
                    );
                }
                () = tokio::time::sleep(SERVICE_READINESS_POLL_INTERVAL) => {}
            }
        }
    }

    async fn mark_readiness_failed(
        &self,
        name: &str,
        instance_id: ProjectInstanceId,
    ) -> Result<()> {
        let changed = {
            let mut services = self.services.lock().await;
            let service = services.get_mut(name).with_context(|| {
                format!("service `{name}` disappeared during readiness failure")
            })?;
            anyhow::ensure!(
                service.instance_id == instance_id,
                "service `{name}` changed project instances during readiness failure"
            );
            let requirement =
                ReadinessRequirement::for_service(&service.service_config, service.sticky_port)
                    .with_context(|| {
                        format!("service `{name}` has an invalid readiness contract")
                    })?;
            let source = requirement.health_source();
            let changed =
                service.health_status != HealthStatus::Unhealthy || service.health_source != source;
            if changed {
                service.health_status = HealthStatus::Unhealthy;
                service.health_source = source;
                Self::advance_service_projection(service);
            }
            changed
        };
        self.persist_state_checked().await?;
        if changed {
            self.broadcast_service_update(name).await;
        }
        Ok(())
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
        self.start_with_manual_cli_session(path, event_tx, verbose, None)
            .await
    }

    pub async fn start_with_manual_cli_session(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        manual_cli_session: Option<ManualCliSession>,
    ) -> Result<()> {
        self.start_with_request_provenance(path, event_tx, verbose, manual_cli_session, None)
            .await
    }

    pub(crate) async fn start_from_ipc(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        manual_cli_session: Option<ManualCliSession>,
        legacy_cli_peer_pid: Option<u32>,
    ) -> Result<()> {
        self.start_with_request_provenance(
            path,
            event_tx,
            verbose,
            manual_cli_session,
            legacy_cli_peer_pid,
        )
        .await
    }

    async fn start_with_request_provenance(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        manual_cli_session: Option<ManualCliSession>,
        legacy_cli_peer_pid: Option<u32>,
    ) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        let path = Self::canonicalize_path(&path);
        let expected_initial = match self.resolve_lifecycle_target(&path).await? {
            LifecycleTargetResolution::Catalogued(target) => {
                return self
                    .start_catalogued_instance(
                        target.instance_id,
                        path,
                        event_tx,
                        verbose,
                        manual_cli_session,
                        legacy_cli_peer_pid,
                    )
                    .await;
            }
            LifecycleTargetResolution::UnregisteredPhysical { instance_id } => {
                ConfigPhysicalIdentity::Git(instance_id)
            }
            LifecycleTargetResolution::UnresolvedLegacy => ConfigPhysicalIdentity::NonGit,
            LifecycleTargetResolution::Ambiguous => anyhow::bail!(
                "project start cannot resolve `{}` because it matches multiple catalogued project instances",
                path.display()
            ),
        };

        let (instance_id, pending) = self
            .start_runtime(path.clone(), None, false, expected_initial)
            .await?;
        let result = self
            .start_catalogued_instance(
                instance_id,
                path,
                event_tx,
                verbose,
                manual_cli_session,
                legacy_cli_peer_pid,
            )
            .await;
        drop(pending);
        result
    }

    async fn start_runtime(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        initial_identity: ConfigPhysicalIdentity,
    ) -> Result<(ProjectInstanceId, PendingInitialAvailabilityGuard)> {
        let (path, transition_lock) = self.transition_lock_for_path(&path).await;
        let _transition_guard = transition_lock.lock().await;
        self.start_runtime_locked(path, event_tx, verbose, initial_identity)
            .await
    }

    async fn start_runtime_locked(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        initial_identity: ConfigPhysicalIdentity,
    ) -> Result<(ProjectInstanceId, PendingInitialAvailabilityGuard)> {
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.forgotten_reload_paths.lock().await.remove(&path);
        // Install the watcher while the same transition still owns tombstone
        // reactivation. Events during a slow build/readiness wait are queued;
        // their reload will take this lock after the initial apply finishes.
        self.watch_config(path.clone()).await;
        let outcome = self
            .apply_config_locked(
                path,
                event_tx,
                verbose,
                Some(ConfigIdentityExpectation::Initial(initial_identity)),
                false,
                None,
            )
            .await?;
        let pending = outcome
            .pending_initial
            .context("initial config publication did not retain availability ownership")?;
        Ok((outcome.instance_id, pending))
    }

    async fn resolve_or_register_ensure_project(
        &self,
        project_path: &Path,
    ) -> Result<(ProjectInstanceId, Option<PendingInitialAvailabilityGuard>)> {
        let (canonical, transition_lock) = self.transition_lock_for_path(project_path).await;
        let _transition_guard = transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        match self.resolve_lifecycle_target(&canonical).await? {
            LifecycleTargetResolution::Catalogued(target) => Ok((target.instance_id, None)),
            LifecycleTargetResolution::UnregisteredPhysical { instance_id } => {
                let (registered, pending) = self
                    .start_runtime_locked(
                        canonical,
                        None,
                        false,
                        ConfigPhysicalIdentity::Git(instance_id),
                    )
                    .await?;
                Ok((registered, Some(pending)))
            }
            LifecycleTargetResolution::UnresolvedLegacy => {
                let (registered, pending) = self
                    .start_runtime_locked(canonical, None, false, ConfigPhysicalIdentity::NonGit)
                    .await?;
                Ok((registered, Some(pending)))
            }
            LifecycleTargetResolution::Ambiguous => anyhow::bail!(
                "project ensure cannot resolve `{}` because it matches multiple catalogued project instances",
                project_path.display()
            ),
        }
    }

    async fn reload_config(&self, path: PathBuf) -> Result<()> {
        let path = Self::canonicalize_path(&path);
        if self.forgotten_reload_paths.lock().await.contains(&path) {
            return Ok(());
        }
        match self.resolve_lifecycle_target(&path).await? {
            LifecycleTargetResolution::Catalogued(target) => {
                self.reload_catalogued_instance(target.instance_id, path)
                    .await
            }
            LifecycleTargetResolution::UnregisteredPhysical { .. }
            | LifecycleTargetResolution::UnresolvedLegacy
            | LifecycleTargetResolution::Ambiguous => Ok(()),
        }
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
        self.apply_config_locked(
            path,
            event_tx,
            verbose,
            expected_instance.map(ConfigIdentityExpectation::Existing),
            true,
            None,
        )
        .await
        .map(|_| ())
    }

    async fn apply_config_locked(
        &self,
        path: PathBuf,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        expected_instance: Option<ConfigIdentityExpectation>,
        start_services: bool,
        service_activation: Option<&str>,
    ) -> Result<ConfigApplyOutcome> {
        self.ensure_accepting_lifecycle_requests()?;
        let discovery_before_config = Registry::discover(path.clone()).await?;
        let physical_identity_before_config =
            ConfigPhysicalIdentity::from_discovery(&discovery_before_config);
        if let Some(expected_initial) =
            expected_instance.and_then(ConfigIdentityExpectation::initial_physical_identity)
        {
            anyhow::ensure!(
                physical_identity_before_config == expected_initial,
                "project identity changed before initial config loading"
            );
        }
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
        let service_activation = service_activation
            .map(|selected| Self::service_activation_closure(&config, selected))
            .transpose()?;
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
        #[cfg(test)]
        self.wait_at_config_publication_hook().await;
        let lifecycle_publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let discovery = Registry::discover(path.clone()).await?;
        let physical_identity_at_publication = ConfigPhysicalIdentity::from_discovery(&discovery);
        anyhow::ensure!(
            physical_identity_at_publication == physical_identity_before_config,
            "project identity changed while loading configuration"
        );
        let initial_registration =
            expected_instance.is_some_and(ConfigIdentityExpectation::is_initial);
        let (
            commit_result,
            instance_id,
            removed_service_names,
            published_domain_index,
            mut reusable_service_envs,
            stopped_service_projections,
            pending_initial,
        ) = {
            let mut registry = self.registry.lock().await;
            let catalog_base = registry.clone();
            let mut candidate = catalog_base.clone();
            let instance_id =
                candidate.register_project(discovery, Some(config.project.name.clone()))?;
            if let Some(expectation) = expected_instance {
                if expectation.requires_existing_catalog() {
                    let expected_instance = expectation
                        .expected_instance_id()
                        .expect("existing config expectation has an instance ID");
                    anyhow::ensure!(
                        registry.instances.contains_key(&expected_instance),
                        "project instance {expected_instance} is no longer catalogued"
                    );
                }
                if let Some(expected_instance) = expectation.expected_instance_id() {
                    anyhow::ensure!(
                        instance_id == expected_instance,
                        "project identity changed while applying config: expected {expected_instance}, discovered {instance_id}"
                    );
                }
            }
            let pending_initial = if initial_registration {
                let inserted = self
                    .pending_initial_availability
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(instance_id);
                anyhow::ensure!(
                    inserted,
                    "initial availability publication is already pending for project instance {instance_id}"
                );
                Some(PendingInitialAvailabilityGuard {
                    instance_id,
                    pending: Arc::clone(&self.pending_initial_availability),
                })
            } else {
                None
            };
            let claims = Self::build_domain_claims(instance_id, &config, &path)?;
            candidate.replace_domain_claims(instance_id, claims)?;

            // Keep the previous claim set published until every removed or
            // restart-required service has stopped. A failed stop retains both
            // ownership and the retryable service record.
            let ConfigTransitionPlan {
                removed_service_names,
                restart_service_names,
                reusable_service_envs,
                stopped_service_projections,
            } = self
                .prepublication_stop_plan(
                    instance_id,
                    &config,
                    &dot_env_vars,
                    &sorted_services,
                    &desired_service_names,
                )
                .await?;
            if expected_instance.is_some_and(ConfigIdentityExpectation::requires_existing_catalog) {
                self.availability_authorizes_start_locked(instance_id)
                    .await?;
            }
            for name in &removed_service_names {
                info!("Service {name} removed from config, stopping before domain publication...");
                self.stop_service_instance_locked(name, instance_id).await?;
            }
            for name in &restart_service_names {
                info!("Service {name} changed, stopping before domain publication...");
                self.stop_service_instance_locked(name, instance_id).await?;
            }

            let (commit_result, published_domain_index) = if initial_registration {
                // A first registration must not leave a catalogued instance
                // without availability authority if the daemon exits before
                // the caller publishes its first demand. Establish an idle
                // availability record in the same replayable transaction as
                // the catalog and compatibility projection; the immediately
                // following lifecycle operation adds the caller's demand. A
                // deferred legacy project receives its already-planned policy,
                // live demands, and pause in this same transaction.
                drop(registry);
                let attachment_base = self.attachments.lock().await.snapshot();
                let target = CataloguedLifecycleTarget {
                    instance_id,
                    path: path.clone(),
                    catalog_base,
                    catalog_target: candidate,
                };
                let transaction = self
                    .prepare_project_lifecycle_transaction(
                        target,
                        &AvailabilityBatch::new(self.availability_now()),
                        attachment_base.clone(),
                        attachment_base,
                    )
                    .await?;
                let commit_result = self
                    .create_and_apply_lifecycle_transaction_locked(&transaction)
                    .await;
                // The catalog rename is the ownership commit point even when
                // a later journal phase reports incomplete durability or
                // fails. Observe the authoritative catalog image directly so
                // hosts and runtime projections converge before the error is
                // surfaced to the caller.
                let published_domain_index = {
                    let registry = self.registry.lock().await;
                    transaction.catalog().and_then(|images| {
                        (*registry == *images.target()).then(|| registry.domain_index().clone())
                    })
                };
                (commit_result, published_domain_index)
            } else {
                // `commit_candidate` advances the in-memory catalog at the
                // atomic rename commit point, including PublishedNotDurable.
                let commit_result = registry.commit_candidate(candidate).await;
                let catalog_published = commit_result.is_ok()
                    || matches!(
                        &commit_result,
                        Err(CatalogError::PublishedNotDurable { .. })
                    );
                let published_domain_index =
                    catalog_published.then(|| registry.domain_index().clone());
                (commit_result.map_err(Into::into), published_domain_index)
            };
            (
                commit_result,
                instance_id,
                removed_service_names,
                published_domain_index,
                reusable_service_envs,
                stopped_service_projections,
                pending_initial,
            )
        };

        // The catalog rename is the ownership commit point. Synchronize hosts
        // from that exact snapshot even when the parent-directory fsync reports
        // PublishedNotDurable, then surface the durability result. Removed
        // service records leave runtime state at the same publication point.
        if let Some(published_domain_index) = published_domain_index {
            let mut published_stopped_service_names = Vec::new();
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
                for (name, (service_config, resolved_env)) in &stopped_service_projections {
                    if let Some(service) = services.get_mut(name).filter(|service| {
                        service.instance_id == instance_id
                            && matches!(&service.runtime_state, ServiceRuntime::None)
                    }) {
                        service.config = config.clone();
                        service.service_config.clone_from(service_config);
                        service.path.clone_from(&path);
                        service.resolved_env = resolved_env.clone().unwrap_or_default();
                        service.health_status = HealthStatus::Unknown;
                        service.health_source = HealthSource::None;
                        service.warnings.clear();
                        Self::advance_service_projection(service);
                        published_stopped_service_names.push(name.clone());
                    }
                }
                for name in &removed_service_names {
                    services.remove(name);
                }
                self.domain_index.store(published_domain_index);
            }
            for name in &published_stopped_service_names {
                self.broadcast_service_update(name).await;
            }
            if !removed_service_names.is_empty() {
                let removed_service_names = removed_service_names
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>();
                self.clear_service_stop_suppressions_for(instance_id, &removed_service_names)
                    .await;
            }
            drop(lifecycle_publication_guard);
            self.persist_state().await;
            self.sync_hosts_after_catalog_publish().await;
        } else {
            drop(lifecycle_publication_guard);
        }
        commit_result?;

        if let Some(service_names) = &service_activation {
            self.clear_service_stop_suppressions_for(instance_id, service_names)
                .await;
        }

        if let Some(tx) = &event_tx {
            let _ = tx
                .send(BootEvent::StepFinished {
                    id: "config".to_string(),
                    result: Ok(()),
                })
                .await;
        }

        if !start_services {
            return Ok(ConfigApplyOutcome {
                instance_id,
                pending_initial,
            });
        }

        for service_name in sorted_services {
            anyhow::ensure!(
                self.path_matches_instance(&path, instance_id).await,
                "project identity changed before service runtime creation for instance {instance_id}"
            );
            self.availability_allows_inflight_transition(instance_id)
                .await?;
            let service_config = &config.services[&service_name];
            info!(
                "Service {}:{} config: {:?}",
                config.project.name, service_name, service_config
            );
            let name = format!("{}:{}", config.project.name, service_name);
            if self
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, name.clone()))
            {
                info!("Service {name} remains stopped until an explicit lifecycle action");
                continue;
            }

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

            let needs_port = ReadinessRequirement::service_requires_port(service_config);

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
            let readiness = ReadinessRequirement::for_service(service_config, port)
                .with_context(|| format!("service `{name}` has an invalid readiness contract"))?;

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
                                health_status: HealthStatus::Starting,
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

                    let start_authorization = async {
                        self.availability_authorizes_start(instance_id).await?;
                        anyhow::ensure!(
                            self.path_matches_instance(&path, instance_id).await,
                            "project identity changed while preparing service `{name}` for instance {instance_id}"
                        );
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    if let Err(superseded) = start_authorization {
                        let cleanup = self.stop_service_instance_locked(&name, instance_id).await;
                        if let Err(cleanup_error) = cleanup {
                            return Err(superseded.context(format!(
                                "failed to stop service `{name}` after availability superseded its prepared start: {cleanup_error:#}"
                            )));
                        }
                        return Err(superseded);
                    }

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

                    // Keep the allocator-selected endpoint authoritative for
                    // readiness and sticky reuse.
                    {
                        let mut services = self.services.lock().await;
                        if let Some(service) = services
                            .get_mut(&name)
                            .filter(|service| service.instance_id == instance_id)
                        {
                            service.sticky_port = port;
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
                        readiness.clone(),
                        port,
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

        self.persist_state().await;
        Ok(ConfigApplyOutcome {
            instance_id,
            pending_initial,
        })
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
            let Some(instance_id) = self.service_instance_id(name).await else {
                return Ok(());
            };
            let coordinator = self.availability_coordinator(instance_id).await;
            let _availability_guard = coordinator.runtime.lock().await;
            let Some((path, _transition_guard, _runtime_projection_guard)) =
                self.lock_service_runtime_transition(name).await
            else {
                return Ok(());
            };
            if self.service_instance_id(name).await != Some(instance_id) {
                continue;
            }
            self.ensure_accepting_lifecycle_requests()?;
            self.stop_service_locked(name, &path).await?;
            self.service_stop_suppressions
                .lock()
                .await
                .insert((instance_id, name.to_owned()));
            return Ok(());
        }
    }

    /// Starts one explicitly selected service while preserving independent
    /// service-stop overrides in the same project.
    pub async fn start_service(&self, name: &str) -> Result<()> {
        self.run_admitted_availability_transition(|| self.start_service_admitted(name))
            .await
    }

    async fn start_service_admitted(&self, name: &str) -> Result<()> {
        loop {
            let Some(instance_id) = self.service_instance_id(name).await else {
                return Err(ServiceNotFoundError.into());
            };
            let coordinator = self.availability_coordinator(instance_id).await;
            let _availability_guard = coordinator.runtime.lock().await;
            let Some((path, _transition_guard, _runtime_projection_guard)) =
                self.lock_service_runtime_transition(name).await
            else {
                return Err(ServiceNotFoundError.into());
            };
            if self.service_instance_id(name).await != Some(instance_id) {
                continue;
            }
            self.ensure_accepting_lifecycle_requests()?;
            let durability_error = self
                .ensure_service_start_availability_locked(instance_id)
                .await?;
            self.watch_config(path.clone()).await;
            let start = self
                .apply_config_locked(
                    path,
                    None,
                    false,
                    Some(ConfigIdentityExpectation::Existing(instance_id)),
                    true,
                    Some(name),
                )
                .await
                .map(|_| ());
            return Self::surface_availability_durability(start, durability_error);
        }
    }

    async fn stop_service_locked(&self, name: &str, project_path: &Path) -> Result<()> {
        let instance_id = {
            let services = self.services.lock().await;
            if let Some(service) = services.get(name) {
                anyhow::ensure!(
                    Self::canonicalize_path(&service.path) == project_path,
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
        let catalog_instances = {
            let registry = self.registry.lock().await;
            registry
                .instances
                .iter()
                .map(|(instance_id, record)| {
                    (
                        *instance_id,
                        record
                            .current_path
                            .clone()
                            .unwrap_or_else(|| record.last_known_path.clone()),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut managed_instances = Vec::new();
        let mut managed_ids = HashSet::new();
        for (instance_id, path) in catalog_instances {
            if self.availability_is_managed(instance_id).await? {
                managed_ids.insert(instance_id);
                managed_instances.push((instance_id, path));
            }
        }
        let mut uncatalogued_paths = {
            let services = self
                .services
                .lock()
                .await
                .values()
                .map(|service| (service.instance_id, service.path.clone()))
                .collect::<Vec<_>>();
            let mut uncatalogued_paths = HashSet::new();
            for (instance_id, path) in services {
                if !managed_ids.contains(&instance_id) {
                    uncatalogued_paths.insert(Self::canonicalize_path(&path));
                }
            }
            uncatalogued_paths.into_iter().collect::<Vec<_>>()
        };
        managed_instances.sort_by_key(|(instance_id, _)| *instance_id);
        uncatalogued_paths.sort();

        for (instance_id, path) in managed_instances {
            if let Err(error) = self.project_force_stop(path).await {
                error!("Failed to stop project instance {instance_id}: {error}");
            }
        }

        for path in uncatalogued_paths {
            if let Err(error) = self.stop_project(&path).await {
                error!("Failed to stop project at {}: {error}", path.display());
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
        self.stop_project_locked(&project_path).await?;
        self.persist_state().await;
        Ok(())
    }

    async fn stop_project_locked(&self, project_path: &Path) -> Result<()> {
        let service_names: Vec<String> = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter(|(_, service)| Self::canonicalize_path(&service.path) == project_path)
                .map(|(name, _)| name.clone())
                .collect()
        };

        for name in service_names {
            self.stop_service_locked(&name, project_path).await?;
        }
        Ok(())
    }

    /// Stop an unresolved legacy project before retiring its last durable
    /// compatibility owner.
    ///
    /// The attachment projection is the retry authority for a project that
    /// does not yet have a catalog identity. Keep it published until both the
    /// runtime stop and its state snapshot are durable. If either operation
    /// fails, the next reconciliation sweep observes the same owner evidence
    /// and retries the stop. A daemon exit after the stop but before lifecycle
    /// publication is likewise harmless: the retry performs an idempotent stop
    /// before retiring the stale owner.
    async fn stop_unresolved_project_and_publish_attachment_transaction(
        &self,
        project_path: &Path,
        expected_resolution: LifecycleTargetIdentity,
        transaction: &LifecycleTransaction,
    ) -> Result<()> {
        let (project_path, transition_lock) = self.transition_lock_for_path(project_path).await;
        let _transition_guard = transition_lock.lock().await;
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        let _publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.ensure_lifecycle_publication_available()?;

        let resolution = self.resolve_lifecycle_target(&project_path).await?;
        anyhow::ensure!(
            resolution.identity() == expected_resolution
                && matches!(resolution, LifecycleTargetResolution::UnresolvedLegacy),
            "project identity changed while unresolved owner cleanup was waiting for runtime ownership; retry the cleanup"
        );

        self.stop_project_locked(&project_path).await?;
        self.persist_state_checked().await?;

        // The caller must release the publication lock before waiting for the
        // path/runtime stop boundary. An unrelated project may publish new
        // catalog and compatibility images in that interval, so reapply only
        // this project's intended compatibility projection to the complete
        // authoritative state after publication is reacquired. This keeps a
        // normal cross-project race from creating a journal whose stale base
        // can never be replayed.
        anyhow::ensure!(
            transaction.kind() == LifecycleTransactionKind::LifecycleMutation
                && transaction.availability().is_empty(),
            "unresolved owner cleanup requires an attachment-only lifecycle mutation"
        );
        let intended_project = transaction.attachments().target().project(&project_path);
        let mut intended_only = transaction.attachments().base().clone();
        intended_only.replace_project(
            &project_path,
            intended_project.attachments.clone(),
            intended_project.manually_stopped,
        );
        if let Some(instance_id) = intended_project.instance_owner {
            intended_only.set_instance_owner(&project_path, instance_id);
        } else {
            intended_only.clear_instance_owner(&project_path);
        }
        anyhow::ensure!(
            intended_only == *transaction.attachments().target(),
            "unresolved owner cleanup may mutate only its exact project compatibility projection"
        );

        let catalog = self.registry.lock().await.clone();
        let attachment_base = self.attachments.lock().await.snapshot();
        let mut attachment_target = attachment_base.clone();
        attachment_target.replace_project(
            &project_path,
            intended_project.attachments,
            intended_project.manually_stopped,
        );
        if let Some(instance_id) = intended_project.instance_owner {
            attachment_target.set_instance_owner(&project_path, instance_id);
        } else {
            attachment_target.clear_instance_owner(&project_path);
        }
        let rebased = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            transaction.effective_at(),
            Some(CatalogTransactionImages::new(catalog.clone(), catalog)?),
            Vec::new(),
            AttachmentTransactionImages::new(attachment_base, attachment_target),
        )?;
        self.create_and_apply_lifecycle_transaction_locked(&rebased)
            .await
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
        loop {
            let Some(instance_id) = self.service_instance_id(name).await else {
                return Err(ServiceNotFoundError.into());
            };
            let coordinator = self.availability_coordinator(instance_id).await;
            let _availability_guard = coordinator.runtime.lock().await;
            let Some((path, _transition_guard, _runtime_projection_guard)) =
                self.lock_service_runtime_transition(name).await
            else {
                return Err(ServiceNotFoundError.into());
            };
            if self.service_instance_id(name).await != Some(instance_id) {
                continue;
            }
            self.ensure_accepting_lifecycle_requests()?;
            self.availability_authorizes_start(instance_id).await?;
            self.stop_service_locked(name, &path).await?;
            self.clear_service_stop_suppression(instance_id, name).await;
            self.watch_config(path.clone()).await;
            return self
                .apply_config_locked(
                    path,
                    None,
                    false,
                    Some(ConfigIdentityExpectation::Existing(instance_id)),
                    true,
                    None,
                )
                .await
                .map(|_| ());
        }
    }

    async fn restart_project_instance(&self, instance_id: ProjectInstanceId) -> Result<()> {
        let coordinator = self.availability_coordinator(instance_id).await;
        let _availability_guard = coordinator.runtime.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.availability_authorizes_start(instance_id).await?;
        let project_path = self
            .active_path_for_instance(instance_id)
            .await
            .context("catalogued project instance has no active path")?;
        let (project_path, transition_lock) = self.transition_lock_for_path(&project_path).await;
        let _transition_guard = transition_lock.lock().await;
        let _runtime_projection_guard = self.runtime_projection_lock.lock().await;
        anyhow::ensure!(
            self.path_matches_instance(&project_path, instance_id).await,
            "project instance {instance_id} changed path during restart"
        );
        self.availability_authorizes_start(instance_id).await?;
        self.stop_project_instance_locked(instance_id).await?;
        self.clear_service_stop_suppressions(instance_id).await;
        self.watch_config(project_path.clone()).await;
        self.apply_config_locked(
            project_path,
            None,
            false,
            Some(ConfigIdentityExpectation::Existing(instance_id)),
            true,
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn restart_all(&self) -> Result<()> {
        let mut instance_ids = {
            let services = self.services.lock().await;
            services
                .values()
                .map(|service| service.instance_id)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        };
        instance_ids.sort();

        for instance_id in instance_ids {
            if let Err(error) = self.restart_project_instance(instance_id).await {
                error!("Failed to restart project instance {instance_id}: {error}");
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

        let Some(instance_id) = self.service_instance_id(name).await else {
            anyhow::bail!("Service {name} not found");
        };
        let coordinator = self.availability_coordinator(instance_id).await;
        let _availability_guard = coordinator.runtime.lock().await;
        let Some((path, _transition_guard, _runtime_projection_guard)) =
            self.lock_service_runtime_transition(name).await
        else {
            anyhow::bail!("Service {name} not found");
        };
        anyhow::ensure!(
            self.service_instance_id(name).await == Some(instance_id),
            "service `{name}` changed project instance during reset"
        );
        self.ensure_accepting_lifecycle_requests()?;
        self.availability_authorizes_start(instance_id).await?;

        // 1. Stop the service
        self.stop_service_locked(name, &path).await?;
        self.clear_service_stop_suppression(instance_id, name).await;

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
        self.apply_config_locked(
            path,
            None,
            false,
            Some(ConfigIdentityExpectation::Existing(instance_id)),
            true,
            None,
        )
        .await?;

        Ok(())
    }

    pub async fn list(&self) -> Vec<ServiceStatus> {
        self.list_with_instance_owners(None)
            .await
            .into_iter()
            .map(|(_, status)| status)
            .collect()
    }

    async fn list_with_instance_owners(
        &self,
        instance_filter: Option<ProjectInstanceId>,
    ) -> Vec<(ProjectInstanceId, ServiceStatus)> {
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
                if instance_filter.is_some_and(|instance_id| service.instance_id != instance_id) {
                    continue;
                }
                let snapshot = match &service.runtime_state {
                    ServiceRuntime::Controller(c) => RuntimeSnapshot::Controller(c.clone()),
                    ServiceRuntime::None => RuntimeSnapshot::Static {
                        is_running: false,
                        pid: None,
                        port: None,
                    },
                };

                snapshots.push((
                    service.instance_id,
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
            instance_id,
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
            results.push((
                instance_id,
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
            ));
        }
        results
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.shutting_down.store(true, AtomicOrdering::Release);
        // Drain transitions admitted before the shutdown request. Later
        // readers observe `shutting_down` before entering the admitted scope.
        let availability_shutdown_guard = self.availability_transition_gate.write().await;
        drop(availability_shutdown_guard);
        let _attachment_transition_guard = self.attachment_transition_lock.lock().await;
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

    pub async fn registry_list(&self) -> Result<Vec<locald_core::registry::ProjectEntry>> {
        let projects = self.registry.lock().await.project_entries();
        let mut resolved = Vec::with_capacity(projects.len());
        for project in projects {
            let canonical = Self::canonicalize_path(&project.path);
            let entry = match self.resolve_lifecycle_projection(&canonical).await {
                LifecycleTargetResolution::Catalogued(target) => {
                    let record = target
                        .catalog_target
                        .instances
                        .get(&target.instance_id)
                        .context("resolved catalog instance is missing from its target image")?;
                    locald_core::registry::ProjectEntry {
                        path: canonical,
                        name: record.display_name.clone(),
                        pinned: record.pinned,
                        last_seen: record.last_seen,
                    }
                }
                LifecycleTargetResolution::UnresolvedLegacy => project,
                LifecycleTargetResolution::UnregisteredPhysical { .. }
                | LifecycleTargetResolution::Ambiguous => locald_core::registry::ProjectEntry {
                    path: canonical,
                    name: None,
                    pinned: false,
                    last_seen: project.last_seen,
                },
            };
            resolved.push(entry);
        }
        resolved.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(resolved)
    }

    /// Resolve live identity when possible while retaining the durable catalog
    /// projection when a saved locator cannot currently be inspected.
    /// Lifecycle mutations continue to use `resolve_lifecycle_target` directly
    /// so discovery failures remain fail-closed.
    async fn resolve_lifecycle_projection(&self, project_path: &Path) -> LifecycleTargetResolution {
        match self.resolve_lifecycle_target(project_path).await {
            Ok(resolution) => resolution,
            Err(error) => {
                let path = Self::canonicalize_path(project_path);
                warn!(
                    "Failed to refresh project identity for read-only projection at {}: {error}",
                    path.display()
                );
                self.catalog_projection_for_path(path).await
            }
        }
    }

    async fn catalog_projection_for_path(&self, path: PathBuf) -> LifecycleTargetResolution {
        let catalog_base = self.registry.lock().await.clone();
        let mut candidates = catalog_base
            .instances
            .iter()
            .filter_map(|(id, record)| {
                (record.current_path.as_deref() == Some(path.as_path())
                    || Self::canonicalize_path(&record.last_known_path) == path)
                    .then_some(*id)
            })
            .collect::<BTreeSet<_>>();
        if let Some(instance_id) = catalog_base.legacy_paths.get(&path) {
            candidates.insert(*instance_id);
        }
        match candidates.len() {
            0 => LifecycleTargetResolution::UnresolvedLegacy,
            1 => LifecycleTargetResolution::Catalogued(Box::new(CataloguedLifecycleTarget {
                instance_id: candidates
                    .into_iter()
                    .next()
                    .expect("one projection candidate exists"),
                path,
                catalog_target: catalog_base.clone(),
                catalog_base,
            })),
            _ => LifecycleTargetResolution::Ambiguous,
        }
    }

    async fn resolve_lifecycle_target(
        &self,
        project_path: &Path,
    ) -> Result<LifecycleTargetResolution> {
        let path = Self::canonicalize_path(project_path);
        let discovery = match Registry::discover(path.clone()).await {
            Ok(discovery) => Some(discovery),
            Err(error) if path.exists() => return Err(error.into()),
            Err(_) => None,
        };
        let registry = self.registry.lock().await;
        let catalog_base = registry.clone();
        let mut catalog_target = catalog_base.clone();
        let instance_id = if let Some(discovery) = discovery {
            let stable_physical_identity = matches!(&discovery, ProjectDiscovery::Git { .. });
            let instance_id = catalog_target.register_project(discovery, None)?;
            if !catalog_base.instances.contains_key(&instance_id) {
                return Ok(if stable_physical_identity {
                    LifecycleTargetResolution::UnregisteredPhysical { instance_id }
                } else {
                    LifecycleTargetResolution::UnresolvedLegacy
                });
            }
            instance_id
        } else {
            let mut candidates = catalog_base
                .instances
                .iter()
                .filter_map(|(id, record)| {
                    (record.current_path.as_deref() == Some(path.as_path())
                        || Self::canonicalize_path(&record.last_known_path) == path)
                        .then_some(*id)
                })
                .collect::<BTreeSet<_>>();
            if let Some(instance_id) = catalog_base.legacy_paths.get(&path) {
                candidates.insert(*instance_id);
            }
            match candidates.len() {
                0 => return Ok(LifecycleTargetResolution::UnresolvedLegacy),
                1 => candidates
                    .into_iter()
                    .next()
                    .expect("one lifecycle candidate exists"),
                _ => return Ok(LifecycleTargetResolution::Ambiguous),
            }
        };
        Ok(LifecycleTargetResolution::Catalogued(Box::new(
            CataloguedLifecycleTarget {
                instance_id,
                path,
                catalog_base,
                catalog_target,
            },
        )))
    }

    fn catalogued_lifecycle_paths(target: &CataloguedLifecycleTarget) -> BTreeSet<PathBuf> {
        let mut paths = BTreeSet::from([Self::canonicalize_path(&target.path)]);
        for catalog in [&target.catalog_base, &target.catalog_target] {
            if let Some(record) = catalog.instances.get(&target.instance_id) {
                paths.insert(Self::canonicalize_path(&record.last_known_path));
                paths.extend(
                    record
                        .current_path
                        .iter()
                        .map(|path| Self::canonicalize_path(path)),
                );
            }
            paths.extend(
                catalog
                    .legacy_paths
                    .iter()
                    .filter(|(_, instance_id)| **instance_id == target.instance_id)
                    .map(|(path, _)| Self::canonicalize_path(path)),
            );
        }
        paths
    }

    fn catalog_paths(catalog: &Registry) -> HashSet<PathBuf> {
        catalog
            .instances
            .values()
            .flat_map(|record| {
                std::iter::once(record.last_known_path.clone())
                    .chain(record.current_path.iter().cloned())
            })
            .chain(catalog.legacy_paths.keys().cloned())
            .chain(catalog.unresolved_legacy.keys().cloned())
            .map(|path| Self::canonicalize_path(&path))
            .collect()
    }

    fn catalog_maps_legacy_path_to_instance(
        catalog: &Registry,
        project_path: &Path,
        instance_id: ProjectInstanceId,
    ) -> bool {
        let project_path = Self::canonicalize_path(project_path);
        catalog.legacy_paths.get(&project_path) == Some(&instance_id)
    }

    fn attachment_path_is_owned_by_target(
        snapshot: &AttachmentStoreSnapshot,
        target: &CataloguedLifecycleTarget,
        project_path: &Path,
    ) -> bool {
        snapshot.instance_owner(project_path) == Some(target.instance_id)
    }

    fn attachment_path_can_initialize_target(
        snapshot: &AttachmentStoreSnapshot,
        target: &CataloguedLifecycleTarget,
        project_path: &Path,
    ) -> bool {
        Self::attachment_path_is_owned_by_target(snapshot, target, project_path)
            || (snapshot.instance_owner(project_path).is_none()
                && (Self::catalog_maps_legacy_path_to_instance(
                    &target.catalog_base,
                    project_path,
                    target.instance_id,
                ) || Self::catalog_maps_legacy_path_to_instance(
                    &target.catalog_target,
                    project_path,
                    target.instance_id,
                )))
    }

    fn compatibility_evidence_for_target(
        snapshot: &AttachmentStoreSnapshot,
        target: &CataloguedLifecycleTarget,
        effective_at: SystemTime,
    ) -> AttachmentCompatibilityEvidence {
        let mut combined = AttachmentCompatibilityEvidence {
            project_path: Self::canonicalize_path(&target.path),
            attachments: Vec::new(),
            manually_stopped: false,
        };
        for path in Self::catalogued_lifecycle_paths(target) {
            if !Self::attachment_path_can_initialize_target(snapshot, target, &path) {
                continue;
            }
            let evidence =
                snapshot.compatibility_evidence_at(&path, effective_at, Self::legacy_pid_alive);
            combined.attachments.extend(evidence.attachments);
            combined.manually_stopped |= evidence.manually_stopped;
        }
        combined
    }

    fn claim_compatibility_projection(
        snapshot: &mut AttachmentStoreSnapshot,
        project_path: &Path,
        instance_id: ProjectInstanceId,
        mut attachments: Vec<Attachment>,
        manually_stopped: bool,
        claimed_at: SystemTime,
    ) {
        let claims_owner = !attachments.is_empty() || manually_stopped;
        if claims_owner {
            for attachment in &mut attachments {
                if matches!(
                    attachment.source,
                    AttachmentSource::Editor { pid: None, .. }
                ) {
                    attachment.created_at = claimed_at;
                }
            }
        }
        snapshot.replace_project(project_path, attachments, manually_stopped);
        if claims_owner {
            snapshot.set_instance_owner(project_path, instance_id);
        }
    }

    async fn availability_record_exists(&self, instance_id: ProjectInstanceId) -> Result<bool> {
        let path = availability_path(&self.availability_data_dir, instance_id);
        tokio::fs::try_exists(&path)
            .await
            .with_context(|| format!("failed to inspect availability state `{}`", path.display()))
    }

    fn normalize_runtime_evidence_for_initial_migration(
        evidence: &mut AttachmentCompatibilityEvidence,
        effective_at: SystemTime,
    ) {
        for item in &mut evidence.attachments {
            if matches!(item.attachment.source, AttachmentSource::Runtime) {
                item.attachment.created_at = effective_at;
                item.alive = true;
            }
        }
    }

    fn retain_explicit_attachment_owners_for_initial_migration(
        evidence: &mut AttachmentCompatibilityEvidence,
        availability_batch: &AvailabilityBatch,
    ) -> Result<()> {
        let ensured_demands = availability_batch
            .operations()
            .iter()
            .filter_map(|operation| match operation {
                AvailabilityBatchOperation::EnsureDemand(demand) => Some(demand),
                AvailabilityBatchOperation::Initialize
                | AvailabilityBatchOperation::RenewDemand(_)
                | AvailabilityBatchOperation::RevalidateDemand(_)
                | AvailabilityBatchOperation::ImportDemand { .. }
                | AvailabilityBatchOperation::ReleaseDemand(_)
                | AvailabilityBatchOperation::SetAlwaysOn(_)
                | AvailabilityBatchOperation::PauseProject
                | AvailabilityBatchOperation::SetTrustedLaunchPath(_)
                | AvailabilityBatchOperation::Retire => None,
            })
            .collect::<BTreeSet<_>>();
        for item in &mut evidence.attachments {
            if let Some(demand) =
                availability_demand_for_attachment_source(&item.attachment.source)?
                && ensured_demands.contains(&demand)
            {
                // This owner was admitted by the lifecycle request being
                // journaled, rather than merely discovered in legacy state.
                // Publish the owner and its demand together; the normal reaper
                // handles a process that exits at this boundary.
                item.alive = true;
            }
        }
        Ok(())
    }

    fn compatibility_projection_for_existing_availability(
        attachments: Vec<Attachment>,
        availability: &ProjectAvailability,
    ) -> Result<Vec<Attachment>> {
        let demand_keys = availability
            .demands()
            .iter()
            .map(|lease| lease.key().clone())
            .collect::<BTreeSet<_>>();
        attachments
            .into_iter()
            .filter_map(|attachment| match &attachment.source {
                AttachmentSource::Pin => availability.always_on().then_some(Ok(attachment)),
                AttachmentSource::Editor { .. }
                | AttachmentSource::CLI { .. }
                | AttachmentSource::ManualCLI(_) => {
                    match availability_demand_for_attachment_source(&attachment.source) {
                        Ok(Some(key)) if demand_keys.contains(&key) => Some(Ok(attachment)),
                        Ok(_) => None,
                        Err(error) => Some(Err(error)),
                    }
                }
                AttachmentSource::Runtime => None,
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    async fn prepare_project_lifecycle_transaction(
        &self,
        mut target: CataloguedLifecycleTarget,
        availability_batch: &AvailabilityBatch,
        attachment_base: AttachmentStoreSnapshot,
        mut attachment_target: AttachmentStoreSnapshot,
    ) -> Result<LifecycleTransaction> {
        let retires = availability_batch
            .operations()
            .iter()
            .any(|operation| matches!(operation, AvailabilityBatchOperation::Retire));
        let availability_state = if retires {
            None
        } else {
            Some(
                self.availability_management_state(target.instance_id)
                    .await?,
            )
        };
        let effective_batch = if retires
            || matches!(
                availability_state,
                Some(AvailabilityManagementState::Managed)
            ) {
            availability_batch.clone()
        } else {
            debug_assert!(matches!(
                availability_state,
                Some(
                    AvailabilityManagementState::PendingInitial
                        | AvailabilityManagementState::LegacyUnmanaged
                )
            ));
            let inherited_always_on = target
                .catalog_base
                .instances
                .get(&target.instance_id)
                .is_some_and(|record| record.pinned)
                || target
                    .catalog_target
                    .instances
                    .get(&target.instance_id)
                    .is_some_and(|record| record.pinned);
            let mut evidence = Self::compatibility_evidence_for_target(
                &attachment_target,
                &target,
                availability_batch.effective_at(),
            );
            Self::retain_explicit_attachment_owners_for_initial_migration(
                &mut evidence,
                availability_batch,
            )?;
            let plan = plan_project_lifecycle_migration(
                inherited_always_on,
                &evidence,
                availability_batch.effective_at(),
            )?;

            for path in Self::catalogued_lifecycle_paths(&target) {
                if Self::attachment_path_can_initialize_target(&attachment_target, &target, &path)
                    || path == Self::canonicalize_path(&target.path)
                {
                    attachment_target.replace_project(&path, Vec::new(), false);
                }
            }
            Self::claim_compatibility_projection(
                &mut attachment_target,
                &target.path,
                target.instance_id,
                plan.compatibility_attachments,
                plan.compatibility_manually_stopped,
                availability_batch.effective_at(),
            );

            let mut combined = plan.availability_batch;
            let mut final_always_on = plan.always_on;
            for operation in availability_batch.operations() {
                if let AvailabilityBatchOperation::SetAlwaysOn(enabled) = operation {
                    final_always_on = *enabled;
                }
                combined.push(operation.clone());
            }
            if let Some(record) = target.catalog_target.instances.get_mut(&target.instance_id) {
                record.pinned = final_always_on;
            }
            combined
        };
        let mut availability = self.load_availability(target.instance_id).await?;
        let prepared = availability.prepare_batch(&effective_batch).await?;
        Ok(LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            effective_batch.effective_at(),
            Some(CatalogTransactionImages::new(
                target.catalog_base,
                target.catalog_target,
            )?),
            vec![prepared],
            AttachmentTransactionImages::new(attachment_base, attachment_target),
        )?)
    }

    fn attachment_demands_for_target(
        snapshot: &AttachmentStoreSnapshot,
        target: &CataloguedLifecycleTarget,
        project_paths: &BTreeSet<PathBuf>,
        include_deferred: bool,
    ) -> Result<BTreeSet<DemandKey>> {
        project_paths
            .iter()
            .filter(|project_path| {
                if include_deferred {
                    Self::attachment_path_can_initialize_target(snapshot, target, project_path)
                } else {
                    Self::attachment_path_is_owned_by_target(snapshot, target, project_path)
                }
            })
            .flat_map(|project_path| snapshot.project(project_path).attachments)
            .map(|attachment| attachment.source)
            .filter_map(
                |attachment| match availability_demand_for_attachment_source(&attachment) {
                    Ok(Some(demand)) => Some(Ok(demand)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect::<std::result::Result<_, _>>()
            .map_err(Into::into)
    }

    fn removed_manual_cli_demands(
        base: &AttachmentStoreSnapshot,
        target: &AttachmentStoreSnapshot,
        project_paths: &BTreeSet<PathBuf>,
    ) -> Result<BTreeSet<DemandKey>> {
        project_paths
            .iter()
            .flat_map(|path| {
                let retained = target.project(path).attachments;
                base.project(path)
                    .attachments
                    .into_iter()
                    .filter(move |attachment| !retained.contains(attachment))
            })
            .filter_map(|attachment| match &attachment.source {
                AttachmentSource::ManualCLI(session) => Some(manual_cli_session_demand(session)),
                AttachmentSource::Editor { .. }
                | AttachmentSource::CLI { .. }
                | AttachmentSource::Runtime
                | AttachmentSource::Pin => None,
            })
            .collect::<std::result::Result<_, _>>()
            .map_err(Into::into)
    }

    fn live_owner_renewal_operations(
        demands: &BTreeSet<DemandKey>,
    ) -> Vec<AvailabilityBatchOperation> {
        demands
            .iter()
            .cloned()
            .map(AvailabilityBatchOperation::RevalidateDemand)
            .collect()
    }

    pub async fn project_attach(
        &self,
        project_path: PathBuf,
        source: AttachmentSource,
    ) -> Result<()> {
        self.project_attach_with_convergence(project_path, source, true)
            .await
    }

    pub(crate) async fn project_attach_from_ipc(
        &self,
        project_path: PathBuf,
        source: AttachmentSource,
        standalone: bool,
    ) -> Result<()> {
        let converge_after_publication =
            standalone || !matches!(&source, AttachmentSource::CLI { .. });
        self.project_attach_with_convergence(project_path, source, converge_after_publication)
            .await
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn project_attach_with_convergence(
        &self,
        project_path: PathBuf,
        source: AttachmentSource,
        converge_after_publication: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            !matches!(&source, AttachmentSource::Runtime),
            "Runtime attachment evidence is accepted only from persisted legacy state"
        );
        anyhow::ensure!(
            !matches!(&source, AttachmentSource::ManualCLI(_)),
            "Manual CLI owners are published atomically by their paired Start request"
        );
        if matches!(&source, AttachmentSource::Pin) {
            return self.registry_set_always_on(&project_path, true).await;
        }
        self.ensure_accepting_lifecycle_requests()?;
        let demand = availability_demand_for_attachment_source(&source)?
            .context("only editor and CLI sources may create a live project attachment")?;
        let canonical = Self::canonicalize_path(&project_path);
        let initial_resolution = self.resolve_lifecycle_target(&canonical).await?;
        let pending_initial = match initial_resolution {
            LifecycleTargetResolution::Catalogued(_) => None,
            LifecycleTargetResolution::UnregisteredPhysical { instance_id } => {
                // First registration keeps complete config validation ahead of
                // identity publication. The new availability owner is published
                // immediately after that validated transition.
                let (_, pending) = self
                    .start_runtime(
                        canonical.clone(),
                        None,
                        false,
                        ConfigPhysicalIdentity::Git(instance_id),
                    )
                    .await?;
                Some(pending)
            }
            LifecycleTargetResolution::UnresolvedLegacy => {
                let (_, pending) = self
                    .start_runtime(
                        canonical.clone(),
                        None,
                        false,
                        ConfigPhysicalIdentity::NonGit,
                    )
                    .await?;
                Some(pending)
            }
            LifecycleTargetResolution::Ambiguous => anyhow::bail!(
                "project attach cannot resolve `{}` because it matches multiple catalogued project instances",
                canonical.display()
            ),
        };

        let transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let expected_instance = self
            .resolve_lifecycle_target(&canonical)
            .await?
            .into_catalogued()
            .context("validated project registration did not produce a catalog instance")?
            .instance_id;
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let target = self
            .resolve_lifecycle_target(&canonical)
            .await?
            .into_catalogued()
            .context("validated project registration did not produce a catalog instance")?;
        anyhow::ensure!(
            target.instance_id == expected_instance,
            "project identity changed during attachment publication"
        );
        let lifecycle_paths = Self::catalogued_lifecycle_paths(&target);
        let instance_id = target.instance_id;
        let effective_at = self.availability_now();
        let include_deferred = !self.availability_record_exists(instance_id).await?;
        let (attachment_base, attachment_target, renews_existing_owner) = {
            let attachments = self.attachments.lock().await;
            let base = attachments.snapshot();
            let mut candidate = attachments.clone();
            for path in &lifecycle_paths {
                if Self::attachment_path_is_owned_by_target(&base, &target, path)
                    || (include_deferred
                        && Self::attachment_path_can_initialize_target(&base, &target, path))
                {
                    candidate.reap_stale_attachments_for_with(
                        path,
                        effective_at,
                        Self::legacy_pid_alive,
                    );
                } else if path == &canonical {
                    candidate.forget_project(path);
                }
            }
            let reaped = candidate.snapshot();
            let renews_existing_owner = Self::attachment_demands_for_target(
                &reaped,
                &target,
                &lifecycle_paths,
                include_deferred,
            )?
            .contains(&demand);
            for path in &lifecycle_paths {
                if Self::attachment_path_is_owned_by_target(&reaped, &target, path)
                    || (include_deferred
                        && Self::attachment_path_can_initialize_target(&reaped, &target, path))
                {
                    candidate.detach(path, &source);
                    if !renews_existing_owner {
                        candidate.clear_stopped(path);
                    }
                }
            }
            candidate.attach(Attachment {
                project_path: canonical.clone(),
                source: source.clone(),
                created_at: effective_at,
            })?;
            candidate.set_instance_owner(&canonical, instance_id);
            if !renews_existing_owner {
                candidate.clear_stopped(&canonical);
            }
            (base, candidate.snapshot(), renews_existing_owner)
        };
        let before_demands = Self::attachment_demands_for_target(
            &attachment_base,
            &target,
            &lifecycle_paths,
            include_deferred,
        )?;
        let after_demands = Self::attachment_demands_for_target(
            &attachment_target,
            &target,
            &lifecycle_paths,
            include_deferred,
        )?;
        let removed_manual_cli_demands = Self::removed_manual_cli_demands(
            &attachment_base,
            &attachment_target,
            &lifecycle_paths,
        )?;
        let mut batch = AvailabilityBatch::new(effective_at);
        for removed in before_demands.difference(&after_demands) {
            batch.push(AvailabilityBatchOperation::ReleaseDemand(removed.clone()));
        }
        for demand in removed_manual_cli_demands {
            // Opportunistic stale-owner reaping must retire both halves of a
            // paired Manual CLI session before the new attachment is
            // published. The process demand appears in the generic delta;
            // the session-owned Manual demand is provenance-specific.
            batch.push(AvailabilityBatchOperation::ReleaseDemand(demand));
        }
        if renews_existing_owner {
            // Legacy ProjectAttach is also used automatically after VS Code
            // rediscovers a daemon. Keep an established owner passive so that
            // recovery cannot cross a user pause. a.3.5 splits semantic editor
            // activation/refocus from heartbeat and recovery renewal.
            let renewal = BTreeSet::from([demand]);
            for operation in Self::live_owner_renewal_operations(&renewal) {
                batch.push(operation);
            }
        } else {
            batch.push(AvailabilityBatchOperation::EnsureDemand(demand));
        }
        let transaction = self
            .prepare_project_lifecycle_transaction(
                target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        drop(pending_initial);
        drop(publication_guard);
        drop(transition_guard);
        if !converge_after_publication {
            if !renews_existing_owner {
                // A new semantic owner resumes normal project management even
                // when an older field-less CLI reserves startup for its
                // immediately following Start request. Clear one-off service
                // stops at this attach boundary so a later explicit stop still
                // wins while the bounded compatibility fallback is waiting.
                let coordinator = self.availability_coordinator(instance_id).await;
                let _runtime_guard = coordinator.runtime.lock().await;
                self.clear_service_stop_suppressions(instance_id).await;
            }
            // Older CLIs publish this process owner immediately before opening
            // their streamed Start request. Keep this adapter to publication so
            // startup and its boot events remain owned by that next request. If
            // the process exits without sending Start, preserve the legacy
            // standalone command by converging after its owner is gone.
            if let AttachmentSource::CLI { pid } = source {
                self.converge_legacy_cli_after_process_exit(instance_id, pid);
            }
            return Ok(());
        }
        if renews_existing_owner {
            self.converge_managed_instance(instance_id, None, false, false)
                .await?;
        } else {
            let coordinator = self.availability_coordinator(instance_id).await;
            let _runtime_guard = coordinator.runtime.lock().await;
            self.clear_service_stop_suppressions(instance_id).await;
            self.converge_managed_instance_locked(instance_id, None, false, false)
                .await?;
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
        if matches!(&source, Some(AttachmentSource::Pin)) {
            return self.registry_set_always_on(&project_path, false).await;
        }
        self.ensure_accepting_lifecycle_requests()?;
        let canonical = Self::canonicalize_path(&project_path);
        let transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let initial_resolution = self.resolve_lifecycle_target(&canonical).await?;
        let expected_resolution = initial_resolution.identity();
        let initial_target =
            initial_resolution.into_mutation_target(&canonical, "project detach")?;
        let expected_instance = initial_target.as_ref().map(|target| target.instance_id);
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let resolution = self.resolve_lifecycle_target(&canonical).await?;
        anyhow::ensure!(
            resolution.identity() == expected_resolution,
            "project identity changed during attachment release"
        );
        let target = resolution.into_mutation_target(&canonical, "project detach")?;
        let lifecycle_paths = target.as_ref().map_or_else(
            || BTreeSet::from([canonical.clone()]),
            Self::catalogued_lifecycle_paths,
        );
        let effective_at = self.availability_now();
        let include_deferred = match expected_instance {
            Some(instance_id) => !self.availability_record_exists(instance_id).await?,
            None => false,
        };
        let (attachment_base, attachment_target, stop_unresolved_legacy) = {
            let attachments = self.attachments.lock().await;
            let base = attachments.snapshot();
            let mut candidate = attachments.clone();
            let mut stop_unresolved_legacy = false;
            for path in &lifecycle_paths {
                let attributable = target.as_ref().is_none_or(|target| {
                    Self::attachment_path_is_owned_by_target(&base, target, path)
                        || (include_deferred
                            && Self::attachment_path_can_initialize_target(&base, target, path))
                });
                if !attributable {
                    continue;
                }
                let removed_last_live_owner = if let Some(source) = &source {
                    candidate.detach(path, source)
                } else {
                    candidate.detach_all_non_pin(path)
                };
                stop_unresolved_legacy |=
                    target.is_none() && removed_last_live_owner && !candidate.is_stopped(path);
                let projection = candidate.snapshot().project(path);
                if projection.attachments.is_empty() && !projection.manually_stopped {
                    candidate.clear_instance_owner(path);
                }
            }
            (base, candidate.snapshot(), stop_unresolved_legacy)
        };
        let removed_manual_cli_demands = Self::removed_manual_cli_demands(
            &attachment_base,
            &attachment_target,
            &lifecycle_paths,
        )?;
        let (before_demands, after_demands) = if let Some(target) = &target {
            (
                Self::attachment_demands_for_target(
                    &attachment_base,
                    target,
                    &lifecycle_paths,
                    include_deferred,
                )?,
                Self::attachment_demands_for_target(
                    &attachment_target,
                    target,
                    &lifecycle_paths,
                    include_deferred,
                )?,
            )
        } else {
            (BTreeSet::new(), BTreeSet::new())
        };
        let mut batch = AvailabilityBatch::new(effective_at);
        for removed in before_demands.difference(&after_demands) {
            batch.push(AvailabilityBatchOperation::ReleaseDemand(removed.clone()));
        }
        if expected_instance.is_some() {
            for demand in removed_manual_cli_demands {
                // Exact and bulk detach release only the Manual sessions
                // removed by this attachment transaction. Singleton Manual
                // policy from ForceStart or Runtime migration is independent.
                batch.push(AvailabilityBatchOperation::ReleaseDemand(demand));
            }
        }
        let transaction = if let Some(target) = target {
            self.prepare_project_lifecycle_transaction(
                target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?
        } else {
            let catalog = self.registry.lock().await.clone();
            LifecycleTransaction::new(
                LifecycleTransactionKind::LifecycleMutation,
                effective_at,
                Some(CatalogTransactionImages::new(catalog.clone(), catalog)?),
                Vec::new(),
                AttachmentTransactionImages::new(attachment_base, attachment_target),
            )?
        };
        if stop_unresolved_legacy {
            drop(publication_guard);
            self.stop_unresolved_project_and_publish_attachment_transaction(
                &canonical,
                expected_resolution,
                &transaction,
            )
            .await?;
            drop(transition_guard);
            return Ok(());
        }

        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        drop(publication_guard);
        drop(transition_guard);
        if let Some(instance_id) = expected_instance {
            self.converge_managed_instance(instance_id, None, false, false)
                .await?;
        }
        Ok(())
    }

    async fn reconcile_legacy_attachment_project(&self, project_path: PathBuf) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        let canonical = Self::canonicalize_path(&project_path);
        let transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let initial_resolution = self.resolve_lifecycle_target(&canonical).await?;
        if matches!(
            initial_resolution,
            LifecycleTargetResolution::UnregisteredPhysical { .. }
                | LifecycleTargetResolution::Ambiguous
        ) {
            return Ok(());
        }
        let expected_resolution = initial_resolution.identity();
        let expected_instance = initial_resolution.catalogued_instance_id();
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let resolution = self.resolve_lifecycle_target(&canonical).await?;
        if matches!(
            resolution,
            LifecycleTargetResolution::UnregisteredPhysical { .. }
                | LifecycleTargetResolution::Ambiguous
        ) {
            return Ok(());
        }
        anyhow::ensure!(
            resolution.identity() == expected_resolution,
            "project identity changed during compatibility attachment reconciliation"
        );
        let target = resolution.into_catalogued();
        let lifecycle_paths = target.as_ref().map_or_else(
            || BTreeSet::from([canonical.clone()]),
            Self::catalogued_lifecycle_paths,
        );
        let effective_at = self.availability_now();
        let include_deferred = match expected_instance {
            Some(instance_id) => !self.availability_record_exists(instance_id).await?,
            None => false,
        };
        let (attachment_base, attachment_target, renewed_demands, stop_unresolved_legacy) = {
            let attachments = self.attachments.lock().await;
            let base = attachments.snapshot();
            let evidence = lifecycle_paths
                .iter()
                .filter(|path| {
                    target.as_ref().is_none_or(|target| {
                        Self::attachment_path_is_owned_by_target(&base, target, path)
                            || (include_deferred
                                && Self::attachment_path_can_initialize_target(&base, target, path))
                    })
                })
                .map(|path| {
                    base.compatibility_evidence_at(path, effective_at, Self::legacy_pid_alive)
                })
                .collect::<Vec<_>>();
            let mut candidate = base.clone();
            let mut renewed_demands = BTreeSet::new();
            let mut stop_unresolved_legacy = false;
            for evidence in evidence {
                let projected_owner_before_reap = evidence
                    .attachments
                    .iter()
                    .any(|item| !matches!(item.attachment.source, AttachmentSource::Runtime));
                for item in evidence.attachments {
                    if !item.alive {
                        candidate.remove_exact_attachment(&evidence.project_path, &item.attachment);
                        continue;
                    }
                    if matches!(
                        item.attachment.source,
                        AttachmentSource::CLI { .. }
                            | AttachmentSource::ManualCLI(_)
                            | AttachmentSource::Editor { pid: Some(_), .. }
                    ) && let Some(demand) =
                        availability_demand_for_attachment_source(&item.attachment.source)?
                    {
                        renewed_demands.insert(demand);
                        if let AttachmentSource::ManualCLI(session) = &item.attachment.source {
                            renewed_demands.insert(manual_cli_session_demand(session)?);
                        }
                    }
                }
                let projection = candidate.project(&evidence.project_path);
                let projected_owner_after_reap = projection
                    .attachments
                    .iter()
                    .any(|attachment| !matches!(attachment.source, AttachmentSource::Runtime));
                stop_unresolved_legacy |= target.is_none()
                    && projected_owner_before_reap
                    && !projected_owner_after_reap
                    && !projection.manually_stopped;
                if projection.attachments.is_empty() && !projection.manually_stopped {
                    candidate.clear_instance_owner(&evidence.project_path);
                }
            }
            (base, candidate, renewed_demands, stop_unresolved_legacy)
        };
        let (before_demands, after_demands) = if let Some(target) = &target {
            (
                Self::attachment_demands_for_target(
                    &attachment_base,
                    target,
                    &lifecycle_paths,
                    include_deferred,
                )?,
                Self::attachment_demands_for_target(
                    &attachment_target,
                    target,
                    &lifecycle_paths,
                    include_deferred,
                )?,
            )
        } else {
            (BTreeSet::new(), BTreeSet::new())
        };
        let removed_manual_cli_demands = Self::removed_manual_cli_demands(
            &attachment_base,
            &attachment_target,
            &lifecycle_paths,
        )?;
        let mut batch = AvailabilityBatch::new(effective_at);
        for removed in before_demands.difference(&after_demands) {
            batch.push(AvailabilityBatchOperation::ReleaseDemand(removed.clone()));
        }
        if expected_instance.is_some() {
            for demand in removed_manual_cli_demands {
                batch.push(AvailabilityBatchOperation::ReleaseDemand(demand));
            }
        }
        let live_demands = renewed_demands
            .intersection(&after_demands)
            .cloned()
            .collect::<BTreeSet<_>>();
        if expected_instance.is_some() {
            for operation in Self::live_owner_renewal_operations(&live_demands) {
                batch.push(operation);
            }
        }
        if batch.is_empty() && attachment_base == attachment_target {
            return Ok(());
        }
        let transaction = if let Some(target) = target {
            self.prepare_project_lifecycle_transaction(
                target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?
        } else {
            let catalog = self.registry.lock().await.clone();
            LifecycleTransaction::new(
                LifecycleTransactionKind::LifecycleMutation,
                effective_at,
                Some(CatalogTransactionImages::new(catalog.clone(), catalog)?),
                Vec::new(),
                AttachmentTransactionImages::new(attachment_base, attachment_target),
            )?
        };
        if stop_unresolved_legacy {
            drop(publication_guard);
            self.stop_unresolved_project_and_publish_attachment_transaction(
                &canonical,
                expected_resolution,
                &transaction,
            )
            .await?;
            drop(transition_guard);
            return Ok(());
        }

        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        drop(publication_guard);
        drop(transition_guard);
        Ok(())
    }

    pub async fn project_force_start(&self, project_path: PathBuf) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        let canonical = Self::canonicalize_path(&project_path);
        let pending_initial = match self.resolve_lifecycle_target(&canonical).await? {
            LifecycleTargetResolution::Catalogued(_) => None,
            LifecycleTargetResolution::UnregisteredPhysical { instance_id } => {
                let (_, pending) = self
                    .start_runtime(
                        canonical.clone(),
                        None,
                        false,
                        ConfigPhysicalIdentity::Git(instance_id),
                    )
                    .await?;
                Some(pending)
            }
            LifecycleTargetResolution::UnresolvedLegacy => {
                let (_, pending) = self
                    .start_runtime(
                        canonical.clone(),
                        None,
                        false,
                        ConfigPhysicalIdentity::NonGit,
                    )
                    .await?;
                Some(pending)
            }
            LifecycleTargetResolution::Ambiguous => anyhow::bail!(
                "project start cannot resolve `{}` because it matches multiple catalogued project instances",
                canonical.display()
            ),
        };
        let transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let expected_instance = self
            .resolve_lifecycle_target(&canonical)
            .await?
            .into_catalogued()
            .context("validated project registration did not produce a catalog instance")?
            .instance_id;
        let coordinator = self.availability_coordinator(expected_instance).await;
        let availability_guard = coordinator.runtime.lock().await;
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let target = self
            .resolve_lifecycle_target(&canonical)
            .await?
            .into_catalogued()
            .context("validated project registration did not produce a catalog instance")?;
        anyhow::ensure!(
            target.instance_id == expected_instance,
            "project identity changed during forced start"
        );
        let lifecycle_paths = Self::catalogued_lifecycle_paths(&target);
        let instance_id = target.instance_id;
        let effective_at = self.availability_now();
        let include_deferred = !self.availability_record_exists(instance_id).await?;
        let (attachment_base, attachment_target) = {
            let attachments = self.attachments.lock().await;
            let base = attachments.snapshot();
            let mut candidate = attachments.clone();
            for path in &lifecycle_paths {
                if Self::attachment_path_is_owned_by_target(&base, &target, path)
                    || (include_deferred
                        && Self::attachment_path_can_initialize_target(&base, &target, path))
                {
                    candidate.clear_stopped(path);
                } else if path == &canonical {
                    candidate.forget_project(path);
                }
            }
            (base, candidate.snapshot())
        };
        let batch = AvailabilityBatch::new(effective_at).with_operation(
            AvailabilityBatchOperation::EnsureDemand(DemandKey::manual_cli()),
        );
        let transaction = self
            .prepare_project_lifecycle_transaction(
                target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        self.clear_service_stop_suppressions(instance_id).await;
        drop(pending_initial);
        drop(publication_guard);
        drop(availability_guard);
        drop(transition_guard);
        self.converge_managed_instance(instance_id, None, false, false)
            .await?;
        Ok(())
    }

    pub async fn project_force_stop(&self, project_path: PathBuf) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        let canonical = Self::canonicalize_path(&project_path);
        let transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let initial_resolution = self.resolve_lifecycle_target(&canonical).await?;
        let expected_resolution = initial_resolution.identity();
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let resolution = self.resolve_lifecycle_target(&canonical).await?;
        anyhow::ensure!(
            resolution.identity() == expected_resolution,
            "project identity changed during forced stop"
        );
        let target = resolution
            .into_mutation_target(&canonical, "project stop")?
            .context("project stop requires a catalogued project instance")?;
        let lifecycle_paths = Self::catalogued_lifecycle_paths(&target);
        let instance_id = target.instance_id;
        let effective_at = self.availability_now();
        let include_deferred = !self.availability_record_exists(instance_id).await?;
        let (attachment_base, attachment_target) = {
            let attachments = self.attachments.lock().await;
            let base = attachments.snapshot();
            let mut candidate = attachments.clone();
            for path in &lifecycle_paths {
                if Self::attachment_path_is_owned_by_target(&base, &target, path)
                    || (include_deferred
                        && Self::attachment_path_can_initialize_target(&base, &target, path))
                {
                    candidate.clear_stopped(path);
                } else if path == &canonical {
                    candidate.forget_project(path);
                }
            }
            candidate.mark_stopped(&canonical);
            candidate.set_instance_owner(&canonical, instance_id);
            (base, candidate.snapshot())
        };
        let batch = AvailabilityBatch::new(effective_at)
            .with_operation(AvailabilityBatchOperation::PauseProject);
        let transaction = self
            .prepare_project_lifecycle_transaction(
                target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        drop(publication_guard);
        drop(transition_guard);
        self.converge_managed_instance(instance_id, None, false, false)
            .await?;
        Ok(())
    }

    /// Acquire or renew one semantic demand and return only after the project
    /// has converged to authoritative readiness.
    pub async fn ensure_project(
        &self,
        project_path: PathBuf,
        demand: DemandKey,
    ) -> Result<EnsureProjectResult> {
        demand
            .validate()
            .map_err(|error| anyhow::anyhow!("invalid EnsureProject demand: {error}"))?;
        anyhow::ensure!(
            demand.kind() != DemandKind::LegacyProcessAttachment,
            "EnsureProject does not accept legacy process-attachment demands"
        );

        let canonical = Self::normalize_ensure_project_root(&project_path)?;
        self.wait_for_proxy_listeners().await?;
        self.run_admitted_availability_transition(|| async {
            self.ensure_accepting_lifecycle_requests()?;
            let (instance_id, pending_initial) =
                self.resolve_or_register_ensure_project(&canonical).await?;

            let coordinator = self.availability_coordinator(instance_id).await;
            let _runtime_guard = coordinator.runtime.lock().await;
            self.ensure_accepting_lifecycle_requests()?;
            let publication_guard = self.lifecycle_publication_lock.lock().await;
            self.ensure_lifecycle_publication_available()?;
            let target = self
                .resolve_lifecycle_target(&canonical)
                .await?
                .into_catalogued()
                .context("validated project registration did not produce a catalog instance")?;
            anyhow::ensure!(
                target.instance_id == instance_id,
                "project identity changed during ensure: expected {instance_id}, discovered {}",
                target.instance_id
            );
            anyhow::ensure!(
                self.active_path_for_instance(instance_id).await.is_some(),
                "project instance {instance_id} is no longer active"
            );
            let mut availability = self.load_availability(instance_id).await?;
            let (_, durability_error) =
                Self::capture_availability_publication(availability.ensure_demand(demand).await)?;
            drop(publication_guard);

            self.clear_service_stop_suppressions(instance_id).await;
            let convergence = self
                .converge_managed_instance_locked(instance_id, None, false, true)
                .await;
            let decision = Self::surface_availability_durability(convergence, durability_error)?;
            if !matches!(decision, ConvergenceDecision::EnsureUp) {
                return Err(AvailabilityStartSuperseded {
                    instance_id,
                    decision,
                }
                .into());
            }

            let result = self.ensure_project_result(instance_id).await?;
            let _publication_guard = self.lifecycle_publication_lock.lock().await;
            self.availability_authorizes_start_locked(instance_id)
                .await?;
            drop(pending_initial);
            Ok(result)
        })
        .await
    }

    fn normalize_ensure_project_root(project_path: &Path) -> Result<PathBuf> {
        let absolute = std::path::absolute(project_path)
            .with_context(|| format!("failed to absolutize `{}`", project_path.display()))?;
        let metadata = match std::fs::metadata(&absolute) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect project locator `{}`", absolute.display())
                });
            }
        };
        let root_locator = if metadata.is_some_and(|metadata| metadata.is_file())
            || absolute
                .file_name()
                .is_some_and(|name| name == "locald.toml")
        {
            absolute.parent().with_context(|| {
                format!(
                    "project configuration file `{}` has no parent directory",
                    absolute.display()
                )
            })?
        } else {
            absolute.as_path()
        };
        locald_core::normalize_project_locator(root_locator)
            .with_context(|| format!("failed to normalize `{}`", root_locator.display()))
    }

    async fn wait_for_proxy_listeners(&self) -> Result<()> {
        tokio::time::timeout(SERVICE_READINESS_TIMEOUT, async {
            loop {
                self.ensure_accepting_new_lifecycle_request()?;
                let ports_changed = self.proxy_ports_changed.notified();
                tokio::pin!(ports_changed);
                let _ = ports_changed.as_mut().enable();
                let (http, https) = *self.proxy_ports.lock().await;
                if http.is_some() && https.is_some() {
                    return Ok(());
                }
                ports_changed.await;
            }
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "locald's HTTP and HTTPS proxy listeners did not become ready within {}s; run `locald doctor` and retry",
                SERVICE_READINESS_TIMEOUT.as_secs()
            )
        })?
    }

    async fn ensure_project_result(
        &self,
        instance_id: ProjectInstanceId,
    ) -> Result<EnsureProjectResult> {
        let (project_path, project_name) = {
            let registry = self.registry.lock().await;
            let record = registry
                .instances
                .get(&instance_id)
                .context("ensured project instance is no longer catalogued")?;
            let project_path = record
                .current_path
                .clone()
                .context("ensured project instance has no active path")?;
            (project_path, record.display_name.clone())
        };
        let mut statuses = self
            .list_with_instance_owners(Some(instance_id))
            .await
            .into_iter()
            .map(|(_, status)| status)
            .collect::<Vec<_>>();
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        anyhow::ensure!(
            !statuses.is_empty(),
            "project instance {instance_id} has no required services"
        );

        let mut urls = BTreeSet::new();
        let mut services = Vec::with_capacity(statuses.len());
        for status in statuses {
            anyhow::ensure!(
                status.status == ServiceState::Running
                    && status.health_status == HealthStatus::Healthy,
                "service `{}` did not remain ready while finalizing EnsureProject",
                status.name
            );
            let semantic_url = status
                .url
                .as_ref()
                .and(status.domain.as_ref())
                .map(|domain| format!("https://{domain}"));
            if let Some(url) = &semantic_url {
                urls.insert(url.clone());
            }
            services.push(EnsuredServiceStatus {
                name: status.name,
                service_type: status.service_type,
                status: status.status,
                health_status: status.health_status,
                url: semantic_url,
            });
        }

        Ok(EnsureProjectResult {
            project_path,
            project_name,
            state: EnsureProjectState::Ready,
            services,
            urls: urls.into_iter().collect(),
        })
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
        self.run_admitted_availability_transition(|| async {
            self.ensure_accepting_lifecycle_requests()?;
            let publication_guard = self.lifecycle_publication_lock.lock().await;
            self.ensure_lifecycle_publication_available()?;
            anyhow::ensure!(
                self.active_path_for_instance(instance_id).await.is_some(),
                "project instance {instance_id} is no longer active"
            );
            let mut availability = self.load_availability(instance_id).await?;
            let (result, durability_error) =
                Self::capture_availability_publication(availability.ensure_demand(demand).await)?;
            drop(publication_guard);
            let coordinator = self.availability_coordinator(instance_id).await;
            let _runtime_guard = coordinator.runtime.lock().await;
            self.clear_service_stop_suppressions(instance_id).await;
            let convergence = self
                .converge_managed_instance_locked(instance_id, None, false, false)
                .await;
            Self::surface_availability_durability(convergence, durability_error)?;
            Ok(result.expect("successful availability publication returns its demand result"))
        })
        .await
    }

    /// Enable or disable durable Always On policy and converge the runtime.
    pub async fn project_set_always_on(&self, project_path: &Path, enabled: bool) -> Result<bool> {
        let (instance_id, _) = self
            .required_availability_instance_for_path(project_path)
            .await?;
        self.run_admitted_availability_transition(|| async {
            self.ensure_accepting_lifecycle_requests()?;
            let publication_guard = self.lifecycle_publication_lock.lock().await;
            self.ensure_lifecycle_publication_available()?;
            anyhow::ensure!(
                self.active_path_for_instance(instance_id).await.is_some(),
                "project instance {instance_id} is no longer active"
            );
            let mut availability = self.load_availability(instance_id).await?;
            let (changed, durability_error) =
                Self::capture_availability_publication(availability.set_always_on(enabled).await)?;
            drop(publication_guard);
            let convergence = if enabled {
                let coordinator = self.availability_coordinator(instance_id).await;
                let _runtime_guard = coordinator.runtime.lock().await;
                self.clear_service_stop_suppressions(instance_id).await;
                self.converge_managed_instance_locked(instance_id, None, false, false)
                    .await
            } else {
                self.converge_managed_instance(instance_id, None, false, false)
                    .await
            };
            Self::surface_availability_durability(convergence, durability_error)?;
            Ok(changed.expect("successful availability publication returns its change result"))
        })
        .await
    }

    /// Pause a project through its current activity generation and stop it.
    pub async fn project_pause_availability(&self, project_path: &Path) -> Result<bool> {
        let (instance_id, _) = self
            .required_availability_instance_for_path(project_path)
            .await?;
        self.run_admitted_availability_transition(|| async {
            self.ensure_accepting_lifecycle_requests()?;
            let publication_guard = self.lifecycle_publication_lock.lock().await;
            self.ensure_lifecycle_publication_available()?;
            let mut availability = self.load_availability(instance_id).await?;
            let (changed, durability_error) =
                Self::capture_availability_publication(availability.pause_project().await)?;
            drop(publication_guard);
            let convergence = self
                .converge_managed_instance(instance_id, None, false, false)
                .await;
            Self::surface_availability_durability(convergence, durability_error)?;
            Ok(changed.expect("successful availability publication returns its change result"))
        })
        .await
    }

    /// Recover a published lifecycle intent before admitting IPC, then import
    /// the legacy compatibility stores exactly once.
    pub(crate) async fn recover_and_migrate_lifecycle_state(&self) -> Result<()> {
        self.lifecycle_recovery_required
            .store(true, AtomicOrdering::Release);
        let result = self.recover_and_migrate_lifecycle_state_locked().await;
        self.lifecycle_recovery_required
            .store(result.is_err(), AtomicOrdering::Release);
        result
    }

    async fn recover_and_migrate_lifecycle_state_locked(&self) -> Result<()> {
        let _transition_guard = self.attachment_transition_lock.lock().await;
        let _publication_guard = self.lifecycle_publication_lock.lock().await;
        let preflight = self.lifecycle_journal.preflight().await?;
        let (transaction, marker) = preflight.into_parts();
        if let Some(mut transaction) = transaction {
            let catalog_path = self.registry.lock().await.storage_path().to_path_buf();
            transaction.normalize_catalog_storage_path(&catalog_path);
            self.apply_lifecycle_transaction_locked(&transaction)
                .await?;
        }
        let migration_complete =
            marker.is_some() || self.lifecycle_journal.migration_marker().await?.is_some();
        if migration_complete {
            let catalog = self.registry.lock().await.clone();
            let attachments = self.attachments.lock().await.snapshot();
            validate_attachment_authority(&attachments, &catalog, "authoritative")?;
            self.validate_catalogued_availability_authority(&catalog)
                .await?;
            self.reconcile_catalog_presence_locked().await?;
            self.sync_hosts()
                .await
                .context("failed to reconcile domain claims after lifecycle recovery")?;
            return Ok(());
        }

        let effective_at = self.availability_now();
        let (catalog_base, catalog_path) = {
            let registry = self.registry.lock().await;
            (registry.clone(), registry.storage_path().to_path_buf())
        };
        let (attachment_base, attachment_path, evidence) = {
            let attachments = self.attachments.lock().await;
            (
                attachments.snapshot(),
                attachments.storage_path().to_path_buf(),
                attachments.compatibility_evidence_at(effective_at, Self::legacy_pid_alive),
            )
        };

        self.lifecycle_journal
            .backup_v1_file(LegacyV1File::Catalog, &catalog_path)
            .await?;
        let legacy_registry_path = catalog_path
            .parent()
            .map(|parent| parent.join("registry.json"))
            .unwrap_or_else(Registry::legacy_registry_path);
        self.lifecycle_journal
            .backup_v1_file(LegacyV1File::Registry, &legacy_registry_path)
            .await?;
        self.lifecycle_journal
            .backup_v1_file(LegacyV1File::Attachments, &attachment_path)
            .await?;
        self.lifecycle_journal
            .backup_v1_file(LegacyV1File::RuntimeState, self.state_manager.path())
            .await?;

        let mut evidence_by_instance =
            BTreeMap::<ProjectInstanceId, AttachmentCompatibilityEvidence>::new();
        let mut deferred_evidence = Vec::new();
        for item in evidence {
            let Some(instance_id) =
                Self::catalog_instance_for_legacy_path(&catalog_base, &item.project_path)
            else {
                deferred_evidence.push(item);
                continue;
            };
            let entry = evidence_by_instance.entry(instance_id).or_insert_with(|| {
                AttachmentCompatibilityEvidence {
                    project_path: item.project_path.clone(),
                    attachments: Vec::new(),
                    manually_stopped: false,
                }
            });
            entry.attachments.extend(item.attachments);
            entry.manually_stopped |= item.manually_stopped;
        }

        let mut prepared_availability = Vec::with_capacity(catalog_base.instances.len());
        let mut attachment_target = AttachmentStoreSnapshot::default();
        let mut catalog_target = catalog_base.clone();
        for (instance_id, record) in &catalog_base.instances {
            let project_path = record
                .current_path
                .clone()
                .unwrap_or_else(|| record.last_known_path.clone());
            let mut evidence = evidence_by_instance.remove(instance_id).unwrap_or_else(|| {
                AttachmentCompatibilityEvidence {
                    project_path: project_path.clone(),
                    attachments: Vec::new(),
                    manually_stopped: false,
                }
            });
            let availability_file = availability_path(&self.availability_data_dir, *instance_id);
            let availability_exists = crate::path_entry_exists(&availability_file)
                .await
                .with_context(|| {
                    format!(
                        "failed to inspect availability state `{}`",
                        availability_file.display()
                    )
                })?;
            if !availability_exists {
                Self::normalize_runtime_evidence_for_initial_migration(&mut evidence, effective_at);
            }
            let plan = plan_project_lifecycle_migration(record.pinned, &evidence, effective_at)?;
            let mut availability = self.load_availability(*instance_id).await?;
            let (always_on, compatibility_attachments, compatibility_manually_stopped) =
                if availability_exists {
                    let authoritative = availability.snapshot().await?;
                    prepared_availability.push(
                        availability
                            .prepare_batch(&AvailabilityBatch::new(effective_at))
                            .await?,
                    );
                    (
                        authoritative.always_on(),
                        Self::compatibility_projection_for_existing_availability(
                            plan.compatibility_attachments,
                            &authoritative,
                        )?,
                        authoritative.is_paused(),
                    )
                } else {
                    prepared_availability
                        .push(availability.prepare_batch(&plan.availability_batch).await?);
                    (
                        plan.always_on,
                        plan.compatibility_attachments,
                        plan.compatibility_manually_stopped,
                    )
                };
            if let Some(target_record) = catalog_target.instances.get_mut(instance_id) {
                target_record.pinned = always_on;
            }
            Self::claim_compatibility_projection(
                &mut attachment_target,
                &project_path,
                *instance_id,
                compatibility_attachments,
                compatibility_manually_stopped,
                effective_at,
            );
        }
        for mut evidence in deferred_evidence {
            let evidence_path = Self::canonicalize_path(&evidence.project_path);
            let unresolved_paths = catalog_base
                .unresolved_legacy
                .keys()
                .filter(|path| Self::canonicalize_path(path) == evidence_path)
                .cloned()
                .collect::<Vec<_>>();
            let catalog_pinned = unresolved_paths.iter().any(|path| {
                catalog_base
                    .unresolved_legacy
                    .get(path)
                    .is_some_and(|record| record.pinned)
            });
            Self::normalize_runtime_evidence_for_initial_migration(&mut evidence, effective_at);
            let plan = plan_project_lifecycle_migration(catalog_pinned, &evidence, effective_at)?;
            for path in unresolved_paths {
                if let Some(record) = catalog_target.unresolved_legacy.get_mut(&path) {
                    record.pinned = plan.always_on;
                }
            }
            let mut compatibility_attachments = plan
                .compatibility_attachments
                .into_iter()
                .filter(|attachment| !matches!(attachment.source, AttachmentSource::Pin))
                .collect::<Vec<_>>();
            if let Some(runtime) = evidence
                .attachments
                .iter()
                .filter(|item| matches!(item.attachment.source, AttachmentSource::Runtime))
                .max_by_key(|item| item.attachment.created_at)
            {
                let mut runtime = runtime.attachment.clone();
                runtime.project_path.clone_from(&evidence.project_path);
                compatibility_attachments.push(runtime);
            }
            attachment_target.replace_project(
                &evidence.project_path,
                compatibility_attachments,
                plan.compatibility_manually_stopped,
            );
        }

        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LegacyV1Migration,
            effective_at,
            Some(CatalogTransactionImages::new(catalog_base, catalog_target)?),
            prepared_availability,
            AttachmentTransactionImages::new(attachment_base, attachment_target),
        )?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        self.reconcile_catalog_presence_locked().await?;
        let catalog = self.registry.lock().await.clone();
        self.validate_catalogued_availability_authority(&catalog)
            .await?;
        self.sync_hosts()
            .await
            .context("failed to reconcile domain claims after lifecycle migration")
    }

    async fn validate_catalogued_availability_authority(&self, catalog: &Registry) -> Result<()> {
        for instance_id in catalog.instances.keys().copied() {
            let path = availability_path(&self.availability_data_dir, instance_id);
            let exists = tokio::fs::try_exists(&path).await.with_context(|| {
                format!(
                    "failed to inspect availability state `{}` during lifecycle recovery",
                    path.display()
                )
            })?;
            anyhow::ensure!(
                exists,
                "missing availability state after lifecycle-v2 migration for project instance {instance_id} at `{}`",
                path.display()
            );
            self.load_availability(instance_id).await.with_context(|| {
                format!(
                    "failed to validate availability authority for project instance {instance_id}"
                )
            })?;
        }
        Ok(())
    }

    async fn reconcile_catalog_presence_locked(&self) -> Result<()> {
        let catalog_base = self.registry.lock().await.clone();
        let mut catalog_target = catalog_base.clone();
        catalog_target.reconcile_missing()?;
        if catalog_target == catalog_base {
            return Ok(());
        }
        let attachments = self.attachments.lock().await.snapshot();
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            self.availability_now(),
            Some(CatalogTransactionImages::new(catalog_base, catalog_target)?),
            Vec::new(),
            AttachmentTransactionImages::new(attachments.clone(), attachments),
        )?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await
    }

    async fn create_and_apply_lifecycle_transaction_locked(
        &self,
        transaction: &LifecycleTransaction,
    ) -> Result<()> {
        if let Err(error) = self.lifecycle_journal.create(transaction).await {
            self.lifecycle_recovery_required
                .store(true, AtomicOrdering::Release);
            return Err(error.into());
        }
        self.lifecycle_recovery_required
            .store(true, AtomicOrdering::Release);
        self.apply_lifecycle_transaction_locked(transaction).await?;
        self.lifecycle_recovery_required
            .store(false, AtomicOrdering::Release);
        Ok(())
    }

    async fn apply_lifecycle_transaction_locked(
        &self,
        transaction: &LifecycleTransaction,
    ) -> Result<()> {
        let id = transaction.id();
        let mut phase = transaction.phase();
        self.verify_completed_lifecycle_transaction_phases(transaction)
            .await?;

        if phase == LifecycleTransactionPhase::Prepared {
            if let Some(images) = transaction.catalog() {
                let mut registry = self.registry.lock().await;
                let current = registry.clone();
                anyhow::ensure!(
                    current == *images.base() || current == *images.target(),
                    "lifecycle transaction {id} catalog base no longer matches authoritative state"
                );
                let publication = registry.commit_candidate(images.target().clone()).await;
                if publication.is_ok()
                    || matches!(&publication, Err(CatalogError::PublishedNotDurable { .. }))
                {
                    self.domain_index.store(registry.domain_index().clone());
                }
                publication?;
            }
            self.lifecycle_journal
                .advance(
                    id,
                    LifecycleTransactionPhase::Prepared,
                    LifecycleTransactionPhase::CatalogPublished,
                )
                .await?;
            phase = LifecycleTransactionPhase::CatalogPublished;
        }

        if phase == LifecycleTransactionPhase::CatalogPublished {
            for prepared in transaction.availability() {
                let mut availability = self
                    .load_availability(prepared.project_instance_id())
                    .await?;
                availability.apply_prepared_batch(prepared).await?;
            }
            self.lifecycle_journal
                .advance(
                    id,
                    LifecycleTransactionPhase::CatalogPublished,
                    LifecycleTransactionPhase::AvailabilityPublished,
                )
                .await?;
            phase = LifecycleTransactionPhase::AvailabilityPublished;
        }

        if phase == LifecycleTransactionPhase::AvailabilityPublished {
            let images = transaction.attachments();
            let mut attachments = self.attachments.lock().await;
            let current = attachments.snapshot();
            anyhow::ensure!(
                current == *images.base() || current == *images.target(),
                "lifecycle transaction {id} attachment base no longer matches authoritative state"
            );
            attachments
                .replace_snapshot(images.target().clone())
                .await?;
            self.lifecycle_journal
                .advance(
                    id,
                    LifecycleTransactionPhase::AvailabilityPublished,
                    LifecycleTransactionPhase::CompatibilityPublished,
                )
                .await?;
            phase = LifecycleTransactionPhase::CompatibilityPublished;
        }

        if phase == LifecycleTransactionPhase::CompatibilityPublished {
            if transaction.kind() == LifecycleTransactionKind::LegacyV1Migration {
                self.lifecycle_journal
                    .mark_migration_complete(id, transaction.effective_at())
                    .await?;
            }
            self.lifecycle_journal
                .advance(
                    id,
                    LifecycleTransactionPhase::CompatibilityPublished,
                    LifecycleTransactionPhase::Complete,
                )
                .await?;
            phase = LifecycleTransactionPhase::Complete;
        }

        if phase == LifecycleTransactionPhase::Complete {
            self.lifecycle_journal.clear(id).await?;
        }
        Ok(())
    }

    async fn verify_completed_lifecycle_transaction_phases(
        &self,
        transaction: &LifecycleTransaction,
    ) -> Result<()> {
        let phase = transaction.phase();

        if phase.requires_catalog_target()
            && let Some(images) = transaction.catalog()
        {
            let current = self.registry.lock().await.clone();
            anyhow::ensure!(
                current == *images.target(),
                "lifecycle transaction {} is at phase {phase:?}, but its catalog target is not authoritative",
                transaction.id()
            );
        }
        if phase.requires_availability_targets() {
            for prepared in transaction.availability() {
                let mut availability = self
                    .load_availability(prepared.project_instance_id())
                    .await?;
                let observed = availability
                    .prepare_batch(&AvailabilityBatch::new(transaction.effective_at()))
                    .await?;
                anyhow::ensure!(
                    observed.expected() == prepared.target(),
                    "lifecycle transaction {} is at phase {phase:?}, but availability for {} is not at its target",
                    transaction.id(),
                    prepared.project_instance_id()
                );
            }
        }
        if phase.requires_compatibility_target() {
            let current = self.attachments.lock().await.snapshot();
            anyhow::ensure!(
                current == *transaction.attachments().target(),
                "lifecycle transaction {} is at phase {phase:?}, but its compatibility target is not authoritative",
                transaction.id()
            );
        }
        Ok(())
    }

    fn catalog_instance_for_legacy_path(
        catalog: &Registry,
        path: &Path,
    ) -> Option<ProjectInstanceId> {
        let path = Self::canonicalize_path(path);
        let mut candidates = BTreeSet::new();
        for (instance_id, record) in &catalog.instances {
            if record.current_path.as_deref() == Some(path.as_path())
                || Self::canonicalize_path(&record.last_known_path) == path
            {
                candidates.insert(*instance_id);
            }
        }
        if let Some(instance_id) = catalog.legacy_paths.get(&path) {
            candidates.insert(*instance_id);
        }
        (candidates.len() == 1)
            .then(|| candidates.into_iter().next())
            .flatten()
    }

    fn compatibility_attachments_for_display(
        snapshot: &AttachmentStoreSnapshot,
        project_path: &Path,
        displayed_instance: Option<ProjectInstanceId>,
    ) -> Vec<Attachment> {
        let projection = snapshot.project(project_path);
        let visible = match (projection.instance_owner, displayed_instance) {
            (Some(owner), Some(instance_id)) => owner == instance_id,
            (None, None) => true,
            _ => false,
        };
        if visible {
            projection
                .attachments
                .into_iter()
                .map(|mut attachment| {
                    if let AttachmentSource::ManualCLI(session) = attachment.source {
                        // The retry-stable session UUID is daemon-owned lifecycle
                        // provenance. Compatibility status keeps the safe owner
                        // category and process label without publishing that ID.
                        attachment.source = AttachmentSource::CLI { pid: session.pid() };
                    }
                    attachment
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    fn compatibility_section_for_display(attachments: &[Attachment]) -> ProjectSection {
        if attachments.iter().any(|attachment| {
            !matches!(
                attachment.source,
                AttachmentSource::Pin | AttachmentSource::Runtime
            )
        }) {
            ProjectSection::Active
        } else if attachments
            .iter()
            .any(|attachment| matches!(attachment.source, AttachmentSource::Pin))
        {
            ProjectSection::AlwaysOn
        } else {
            ProjectSection::Recent
        }
    }

    fn legacy_pid_alive(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) | Err(nix::errno::Errno::EPERM) => true,
            Err(_) => false,
        }
    }

    fn converge_legacy_cli_after_process_exit(&self, instance_id: ProjectInstanceId, pid: u32) {
        let manager = self.clone();
        tokio::spawn(async move {
            // Historical `project attach --source cli` is a one-shot command,
            // while historical `up` keeps the same process alive and follows
            // immediately with Start. A bounded exit watch preserves the
            // otherwise wire-identical standalone command without retaining a
            // task for the lifetime of a log-following owner.
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if manager.is_shutting_down() {
                    return;
                }
                if Self::legacy_pid_alive(pid) {
                    continue;
                }
                if let Err(error) = manager
                    .converge_managed_instance(instance_id, None, false, false)
                    .await
                    && !manager.is_shutting_down()
                {
                    warn!(
                        "Failed to converge field-less CLI attachment for project instance {instance_id} after process {pid} exited: {error:#}"
                    );
                }
                return;
            }
        });
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
        self.ensure_accepting_lifecycle_requests()?;
        let canonical = Self::canonicalize_path(project_path);
        let lifecycle_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let initial_resolution = self.resolve_lifecycle_target(&canonical).await?;
        let expected_resolution = initial_resolution.identity();
        let initial_target =
            initial_resolution.into_mutation_target(&canonical, "project removal")?;
        let expected_instance = initial_target.as_ref().map(|target| target.instance_id);
        let availability_guard = if let Some(instance_id) = expected_instance {
            let coordinator = self.availability_coordinator(instance_id).await;
            Some(coordinator.runtime.clone().lock_owned().await)
        } else {
            None
        };

        let mut transition_paths = BTreeSet::from([canonical.clone()]);
        if let Some(target) = &initial_target {
            transition_paths.extend(Self::catalogued_lifecycle_paths(target));
            transition_paths.extend(
                self.services
                    .lock()
                    .await
                    .values()
                    .filter(|service| service.instance_id == target.instance_id)
                    .map(|service| Self::canonicalize_path(&service.path)),
            );
        }
        let transition_guards = self.lock_config_transition_paths(&transition_paths).await;
        let runtime_projection_guard = self.runtime_projection_lock.lock().await;
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;

        let resolution = self.resolve_lifecycle_target(&canonical).await?;
        anyhow::ensure!(
            resolution.identity() == expected_resolution,
            "project identity changed while removal was waiting for lifecycle ownership; retry the removal"
        );
        let target = resolution.into_mutation_target(&canonical, "project removal")?;
        if let Some(target) = &target {
            anyhow::ensure!(
                Self::catalogued_lifecycle_paths(target).is_subset(&transition_paths),
                "project paths changed while removal was waiting for lifecycle ownership; retry the removal"
            );
        }

        let mut retired_paths = transition_paths.iter().cloned().collect::<HashSet<_>>();
        let effective_at = self.availability_now();
        let transaction = if let Some(mut target) = target {
            target.catalog_target.unregister_project(&canonical)?;
            let surviving_paths = Self::catalog_paths(&target.catalog_target);
            retired_paths.retain(|path| !surviving_paths.contains(path));
            let attachment_base = self.attachments.lock().await.snapshot();
            let mut attachment_target = attachment_base.clone();
            for path in &retired_paths {
                attachment_target.replace_project(path, Vec::new(), false);
            }
            let owned_paths = attachment_base
                .instance_owners
                .iter()
                .filter_map(|(path, owner)| (*owner == target.instance_id).then_some(path.clone()))
                .collect::<Vec<_>>();
            for path in owned_paths {
                attachment_target.replace_project(&path, Vec::new(), false);
            }
            self.stop_project_instance_locked(target.instance_id)
                .await?;
            let batch = AvailabilityBatch::new(effective_at)
                .with_operation(AvailabilityBatchOperation::Retire);
            self.prepare_project_lifecycle_transaction(
                target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?
        } else {
            let (catalog_base, mut catalog_target) = {
                let registry = self.registry.lock().await;
                (registry.clone(), registry.clone())
            };
            catalog_target.unregister_project(&canonical)?;
            let surviving_paths = Self::catalog_paths(&catalog_target);
            retired_paths.retain(|path| !surviving_paths.contains(path));
            let attachment_base = self.attachments.lock().await.snapshot();
            let mut attachment_target = attachment_base.clone();
            for path in &retired_paths {
                attachment_target.replace_project(path, Vec::new(), false);
            }
            self.stop_project_locked(&canonical).await?;
            LifecycleTransaction::new(
                LifecycleTransactionKind::LifecycleMutation,
                effective_at,
                Some(CatalogTransactionImages::new(catalog_base, catalog_target)?),
                Vec::new(),
                AttachmentTransactionImages::new(attachment_base, attachment_target),
            )?
        };
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        if let Some(instance_id) = expected_instance {
            self.clear_service_stop_suppressions(instance_id).await;
        }

        let retired_instances = expected_instance
            .into_iter()
            .collect::<HashSet<ProjectInstanceId>>();
        self.retire_config_reload_paths(retired_paths, retired_instances)
            .await;
        if let Some(instance_id) = expected_instance {
            self.services
                .lock()
                .await
                .retain(|_, service| service.instance_id != instance_id);
        } else {
            self.services
                .lock()
                .await
                .retain(|_, service| Self::canonicalize_path(&service.path) != canonical);
        }
        self.persist_state().await;
        drop(publication_guard);
        drop(runtime_projection_guard);
        drop(transition_guards);
        drop(availability_guard);
        drop(lifecycle_guard);
        self.sync_hosts_after_catalog_publish().await;
        Ok(())
    }

    pub async fn project_status(&self, project_path: &Path) -> Result<ProjectStatusInfo> {
        let canonical = Self::canonicalize_path(project_path);
        let resolution = self.resolve_lifecycle_projection(&canonical).await;
        let (project_name, instance_id) = match &resolution {
            LifecycleTargetResolution::Catalogued(target) => (
                target
                    .catalog_target
                    .instances
                    .get(&target.instance_id)
                    .and_then(|record| record.display_name.clone()),
                Some(target.instance_id),
            ),
            LifecycleTargetResolution::UnresolvedLegacy => {
                let registry = self.registry.lock().await;
                (
                    registry
                        .unresolved_legacy
                        .get(&canonical)
                        .and_then(|record| record.display_name.clone()),
                    None,
                )
            }
            LifecycleTargetResolution::UnregisteredPhysical { .. }
            | LifecycleTargetResolution::Ambiguous => (None, None),
        };

        let attachments = {
            let attachments = self.attachments.lock().await;
            let snapshot = attachments.snapshot();
            match resolution {
                LifecycleTargetResolution::Catalogued(_) => {
                    Self::compatibility_attachments_for_display(&snapshot, &canonical, instance_id)
                }
                LifecycleTargetResolution::UnresolvedLegacy => {
                    Self::compatibility_attachments_for_display(&snapshot, &canonical, None)
                }
                LifecycleTargetResolution::UnregisteredPhysical { .. }
                | LifecycleTargetResolution::Ambiguous => Vec::new(),
            }
        };

        let statuses = match instance_id {
            Some(instance_id) => self
                .list_with_instance_owners(Some(instance_id))
                .await
                .into_iter()
                .map(|(_, status)| status)
                .collect(),
            None => Vec::new(),
        };
        let mut services = Vec::new();
        let mut service_details = Vec::new();
        let mut is_running = false;

        for status in statuses {
            if status.status == ServiceState::Running {
                is_running = true;
            }
            services.push(status.name.clone());
            service_details.push(status);
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
        let (registry, registry_projects) = {
            let registry = self.registry.lock().await;
            (registry.clone(), registry.project_entries_by_path())
        };

        let attachment_snapshot = {
            let attachments = self.attachments.lock().await;
            attachments.snapshot()
        };

        let mut all_projects = HashSet::new();
        for path in registry_projects.keys() {
            all_projects.insert(path.clone());
        }
        all_projects.extend(attachment_snapshot.attachments.keys().cloned());
        all_projects.extend(attachment_snapshot.manually_stopped.iter().cloned());

        let statuses = self.list_with_instance_owners(None).await;
        let mut running_instances = HashSet::new();

        for (instance_id, status) in statuses {
            if status.status == ServiceState::Running {
                running_instances.insert(instance_id);
            }
        }

        let mut entries = Vec::new();
        let filter = filter.unwrap_or(ProjectFilter::All);
        for path in all_projects {
            let canonical = Self::canonicalize_path(&path);
            let resolution = self.resolve_lifecycle_projection(&canonical).await;
            let (instance_id, project_name, pinned, attachments_for) = match resolution {
                LifecycleTargetResolution::Catalogued(target) => {
                    let record = target.catalog_target.instances.get(&target.instance_id);
                    (
                        Some(target.instance_id),
                        record.and_then(|record| record.display_name.clone()),
                        record.is_some_and(|record| record.pinned),
                        Self::compatibility_attachments_for_display(
                            &attachment_snapshot,
                            &canonical,
                            Some(target.instance_id),
                        ),
                    )
                }
                LifecycleTargetResolution::UnresolvedLegacy => {
                    let record = registry
                        .unresolved_legacy
                        .get(&canonical)
                        .or_else(|| registry.unresolved_legacy.get(&path));
                    (
                        None,
                        record.and_then(|record| record.display_name.clone()),
                        record.is_some_and(|record| record.pinned),
                        Self::compatibility_attachments_for_display(
                            &attachment_snapshot,
                            &canonical,
                            None,
                        ),
                    )
                }
                LifecycleTargetResolution::UnregisteredPhysical { .. }
                | LifecycleTargetResolution::Ambiguous => (None, None, false, Vec::new()),
            };
            let attachment_section = Self::compatibility_section_for_display(&attachments_for);
            let section = if attachment_section == ProjectSection::Active {
                ProjectSection::Active
            } else if pinned || attachment_section == ProjectSection::AlwaysOn {
                ProjectSection::AlwaysOn
            } else {
                ProjectSection::Recent
            };
            let is_running = instance_id.is_some_and(|id| running_instances.contains(&id));

            let entry = ProjectListEntry {
                project_path: canonical,
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
        self.registry_set_always_on(path, true).await
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_unpin(&self, path: &std::path::Path) -> Result<()> {
        self.registry_set_always_on(path, false).await
    }

    async fn registry_set_always_on(&self, path: &Path, enabled: bool) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        let canonical = Self::canonicalize_path(path);
        let transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let initial_resolution = self.resolve_lifecycle_target(&canonical).await?;
        let expected_resolution = initial_resolution.identity();
        let initial_target =
            initial_resolution.into_mutation_target(&canonical, "Always On update")?;
        let Some(_initial_target) = initial_target else {
            let publication_guard = self.lifecycle_publication_lock.lock().await;
            self.ensure_lifecycle_publication_available()?;
            let resolution = self.resolve_lifecycle_target(&canonical).await?;
            anyhow::ensure!(
                resolution.identity() == expected_resolution,
                "project identity materialized while Always On was waiting for lifecycle ownership; retry the command"
            );
            let confirmed_target =
                resolution.into_mutation_target(&canonical, "Always On update")?;
            anyhow::ensure!(
                confirmed_target.is_none(),
                "project identity materialized while Always On was waiting for lifecycle ownership; retry the command"
            );
            let (catalog_base, mut catalog_target) = {
                let registry = self.registry.lock().await;
                (registry.clone(), registry.clone())
            };
            let changed = if enabled {
                catalog_target.pin_project(&canonical)
            } else {
                catalog_target.unpin_project(&canonical)
            };
            anyhow::ensure!(changed, "Project not found in registry");
            let attachment_base = self.attachments.lock().await.snapshot();
            let mut attachment_target = attachment_base.clone();
            let compatibility = attachment_target.project(&canonical);
            attachment_target.replace_project(
                &canonical,
                compatibility
                    .attachments
                    .into_iter()
                    .filter(|attachment| {
                        enabled || !matches!(attachment.source, AttachmentSource::Pin)
                    })
                    .collect(),
                compatibility.manually_stopped && !enabled,
            );
            let transaction = LifecycleTransaction::new(
                LifecycleTransactionKind::LifecycleMutation,
                self.availability_now(),
                Some(CatalogTransactionImages::new(catalog_base, catalog_target)?),
                Vec::new(),
                AttachmentTransactionImages::new(attachment_base, attachment_target),
            )?;
            self.create_and_apply_lifecycle_transaction_locked(&transaction)
                .await?;
            drop(publication_guard);
            drop(transition_guard);
            self.sync_hosts_after_catalog_publish().await;
            return Ok(());
        };
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let resolution = self.resolve_lifecycle_target(&canonical).await?;
        anyhow::ensure!(
            resolution.identity() == expected_resolution,
            "project identity changed during Always On update"
        );
        let mut target = resolution
            .into_mutation_target(&canonical, "Always On update")?
            .context("Project not found in registry")?;
        let lifecycle_paths = Self::catalogued_lifecycle_paths(&target);
        let changed = if enabled {
            target.catalog_target.pin_project(&canonical)
        } else {
            target.catalog_target.unpin_project(&canonical)
        };
        anyhow::ensure!(changed, "Project not found in registry");
        let instance_id = target.instance_id;
        let effective_at = self.availability_now();
        let include_deferred = !self.availability_record_exists(instance_id).await?;
        let attachment_base = self.attachments.lock().await.snapshot();
        let mut attachment_target = attachment_base.clone();
        if enabled {
            for path in &lifecycle_paths {
                if Self::attachment_path_is_owned_by_target(&attachment_base, &target, path)
                    || (include_deferred
                        && Self::attachment_path_can_initialize_target(
                            &attachment_base,
                            &target,
                            path,
                        ))
                {
                    let compatibility = attachment_target.project(path);
                    attachment_target.replace_project(path, compatibility.attachments, false);
                } else if path == &canonical {
                    attachment_target.replace_project(path, Vec::new(), false);
                }
            }
        } else {
            for path in &lifecycle_paths {
                if !(Self::attachment_path_is_owned_by_target(&attachment_base, &target, path)
                    || include_deferred
                        && Self::attachment_path_can_initialize_target(
                            &attachment_base,
                            &target,
                            path,
                        ))
                {
                    if path == &canonical {
                        attachment_target.replace_project(path, Vec::new(), false);
                    }
                    continue;
                }
                let compatibility = attachment_target.project(path);
                attachment_target.replace_project(
                    path,
                    compatibility
                        .attachments
                        .into_iter()
                        .filter(|attachment| !matches!(attachment.source, AttachmentSource::Pin))
                        .collect(),
                    compatibility.manually_stopped,
                );
            }
        }
        let batch = AvailabilityBatch::new(effective_at)
            .with_operation(AvailabilityBatchOperation::SetAlwaysOn(enabled));
        let transaction = self
            .prepare_project_lifecycle_transaction(
                target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        drop(publication_guard);
        drop(transition_guard);
        if enabled {
            let coordinator = self.availability_coordinator(instance_id).await;
            let _runtime_guard = coordinator.runtime.lock().await;
            self.clear_service_stop_suppressions(instance_id).await;
            self.converge_managed_instance_locked(instance_id, None, false, false)
                .await?;
        } else {
            self.converge_managed_instance(instance_id, None, false, false)
                .await?;
        }
        Ok(())
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn registry_clean(&self) -> Result<usize> {
        self.ensure_accepting_lifecycle_requests()?;
        let lifecycle_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let initial_catalog = self.registry.lock().await.clone();
        let initial_instance_ids = initial_catalog
            .instances
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let mut availability_guards = Vec::with_capacity(initial_instance_ids.len());
        for instance_id in &initial_instance_ids {
            let coordinator = self.availability_coordinator(*instance_id).await;
            availability_guards.push(coordinator.runtime.clone().lock_owned().await);
        }

        let mut transition_paths = Self::catalog_paths(&initial_catalog)
            .into_iter()
            .collect::<BTreeSet<_>>();
        transition_paths.extend(
            self.services
                .lock()
                .await
                .values()
                .map(|service| Self::canonicalize_path(&service.path)),
        );
        let transition_guards = self.lock_config_transition_paths(&transition_paths).await;
        let runtime_projection_guard = self.runtime_projection_lock.lock().await;
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;

        let catalog_base = self.registry.lock().await.clone();
        anyhow::ensure!(
            catalog_base
                .instances
                .keys()
                .copied()
                .eq(initial_instance_ids.iter().copied()),
            "project catalog changed while cleanup was waiting for lifecycle ownership; retry cleanup"
        );
        anyhow::ensure!(
            Self::catalog_paths(&catalog_base)
                .into_iter()
                .all(|path| transition_paths.contains(&path)),
            "project paths changed while cleanup was waiting for lifecycle ownership; retry cleanup"
        );

        let mut catalog_target = catalog_base.clone();
        for (instance_id, record) in &catalog_base.instances {
            let availability_file = availability_path(&self.availability_data_dir, *instance_id);
            let availability_exists = tokio::fs::try_exists(&availability_file)
                .await
                .with_context(|| {
                    format!(
                        "failed to inspect availability state `{}`",
                        availability_file.display()
                    )
                })?;
            let always_on = if availability_exists {
                let mut availability = self.load_availability(*instance_id).await?;
                availability.snapshot().await?.always_on()
            } else {
                record.pinned
            };
            catalog_target
                .instances
                .get_mut(instance_id)
                .expect("catalog target contains every base instance")
                .pinned = always_on;
        }
        let count = catalog_target.prune_missing_projects()?;
        let removed_instance_ids = catalog_base
            .instances
            .keys()
            .filter(|instance_id| !catalog_target.instances.contains_key(instance_id))
            .copied()
            .collect::<Vec<_>>();
        if catalog_target == catalog_base {
            return Ok(count);
        }

        let removed_set = removed_instance_ids.iter().copied().collect::<HashSet<_>>();
        let surviving_paths = Self::catalog_paths(&catalog_target);
        let mut removed_paths = Self::catalog_paths(&catalog_base)
            .into_iter()
            .filter(|path| !surviving_paths.contains(path))
            .collect::<HashSet<_>>();
        removed_paths.extend(
            self.services
                .lock()
                .await
                .values()
                .filter(|service| removed_set.contains(&service.instance_id))
                .map(|service| Self::canonicalize_path(&service.path))
                .filter(|path| !surviving_paths.contains(path)),
        );

        for instance_id in &removed_instance_ids {
            self.stop_project_instance_locked(*instance_id).await?;
        }
        let uncatalogued_stop_paths = self
            .services
            .lock()
            .await
            .values()
            .filter(|service| {
                !removed_set.contains(&service.instance_id)
                    && removed_paths.contains(&Self::canonicalize_path(&service.path))
            })
            .map(|service| Self::canonicalize_path(&service.path))
            .collect::<BTreeSet<_>>();
        for path in &uncatalogued_stop_paths {
            self.stop_project_locked(path).await?;
        }

        let effective_at = self.availability_now();
        let mut prepared_availability = Vec::with_capacity(removed_instance_ids.len());
        for instance_id in &removed_instance_ids {
            let mut availability = self.load_availability(*instance_id).await?;
            prepared_availability.push(
                availability
                    .prepare_batch(
                        &AvailabilityBatch::new(effective_at)
                            .with_operation(AvailabilityBatchOperation::Retire),
                    )
                    .await?,
            );
        }
        let attachment_base = self.attachments.lock().await.snapshot();
        let mut attachment_target = attachment_base.clone();
        for path in &removed_paths {
            attachment_target.replace_project(path, Vec::new(), false);
        }
        let removed_owner_paths = attachment_base
            .instance_owners
            .iter()
            .filter_map(|(path, owner)| removed_set.contains(owner).then_some(path.clone()))
            .collect::<Vec<_>>();
        for path in removed_owner_paths {
            attachment_target.replace_project(&path, Vec::new(), false);
        }
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            effective_at,
            Some(CatalogTransactionImages::new(catalog_base, catalog_target)?),
            prepared_availability,
            AttachmentTransactionImages::new(attachment_base, attachment_target),
        )?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        let retired_paths = removed_paths.clone();
        self.retire_config_reload_paths(removed_paths, removed_set.clone())
            .await;
        self.services.lock().await.retain(|_, service| {
            !removed_set.contains(&service.instance_id)
                && !retired_paths.contains(&Self::canonicalize_path(&service.path))
        });
        self.service_stop_suppressions
            .lock()
            .await
            .retain(|(owner, _)| !removed_set.contains(owner));
        self.persist_state().await;
        drop(publication_guard);
        drop(runtime_projection_guard);
        drop(transition_guards);
        drop(availability_guards);
        drop(lifecycle_guard);
        self.sync_hosts_after_catalog_publish().await;
        Ok(count)
    }

    pub(crate) async fn reconcile_legacy_attachment_owners(&self) -> Result<()> {
        if self.is_shutting_down() {
            return Ok(());
        }
        let effective_at = self.availability_now();
        let project_paths = {
            let attachments = self.attachments.lock().await;
            attachments
                .compatibility_evidence_at(effective_at, Self::legacy_pid_alive)
                .into_iter()
                .map(|evidence| evidence.project_path)
                .collect::<Vec<_>>()
        };

        let mut failures = Vec::new();
        for path in project_paths {
            if let Err(error) = self.reconcile_legacy_attachment_project(path.clone()).await {
                failures.push(format!("`{}`: {error:#}", path.display()));
            }
        }
        if !failures.is_empty() {
            anyhow::bail!(
                "failed to reconcile {} legacy attachment project(s): {}",
                failures.len(),
                failures.join("; ")
            );
        }
        Ok(())
    }

    pub async fn reap_and_stop_orphans(&self) {
        if let Err(error) = self.reconcile_legacy_attachment_owners().await {
            warn!("Failed to reconcile legacy project attachments: {error:#}");
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

    async fn availability_management_state(
        &self,
        instance_id: ProjectInstanceId,
    ) -> Result<AvailabilityManagementState> {
        let path = availability_path(&self.availability_data_dir, instance_id);
        if tokio::fs::try_exists(&path)
            .await
            .with_context(|| format!("failed to inspect availability state `{}`", path.display()))?
        {
            return Ok(AvailabilityManagementState::Managed);
        }
        if self
            .pending_initial_availability
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&instance_id)
        {
            return Ok(AvailabilityManagementState::PendingInitial);
        }
        anyhow::ensure!(
            self.lifecycle_journal.migration_marker().await?.is_none(),
            "project instance {instance_id} is missing availability state after lifecycle-v2 migration; restart locald to recover authoritative lifecycle state"
        );
        Ok(AvailabilityManagementState::LegacyUnmanaged)
    }

    async fn availability_is_managed(&self, instance_id: ProjectInstanceId) -> Result<bool> {
        Ok(matches!(
            self.availability_management_state(instance_id).await?,
            AvailabilityManagementState::Managed
        ))
    }

    async fn sweep_availability(
        availability: &mut AvailabilityStore<SharedAvailabilityClock>,
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

    async fn ensure_service_start_availability_locked(
        &self,
        instance_id: ProjectInstanceId,
    ) -> Result<Option<anyhow::Error>> {
        let _publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.ensure_lifecycle_publication_available()?;
        match self.availability_management_state(instance_id).await? {
            AvailabilityManagementState::Managed => {}
            AvailabilityManagementState::PendingInitial => anyhow::bail!(
                "project instance {instance_id} is awaiting its initial availability publication"
            ),
            AvailabilityManagementState::LegacyUnmanaged => return Ok(None),
        }
        anyhow::ensure!(
            self.active_path_for_instance(instance_id).await.is_some(),
            "project instance {instance_id} is no longer active"
        );
        let mut availability = self.load_availability(instance_id).await?;
        let (_, durability_error) = Self::capture_availability_publication(
            availability
                .ensure_demand(DemandKey::stopped_page_resume())
                .await,
        )?;
        Ok(durability_error)
    }

    async fn availability_authorizes_start(&self, instance_id: ProjectInstanceId) -> Result<()> {
        let _publication_guard = self.lifecycle_publication_lock.lock().await;
        self.availability_authorizes_start_locked(instance_id).await
    }

    async fn availability_authorizes_start_locked(
        &self,
        instance_id: ProjectInstanceId,
    ) -> Result<()> {
        self.ensure_accepting_lifecycle_requests()?;
        self.ensure_lifecycle_publication_available()?;
        match self.availability_management_state(instance_id).await? {
            AvailabilityManagementState::Managed => {}
            AvailabilityManagementState::PendingInitial => anyhow::bail!(
                "project instance {instance_id} is awaiting its initial availability publication"
            ),
            AvailabilityManagementState::LegacyUnmanaged => return Ok(()),
        }
        let mut availability = self.load_availability(instance_id).await?;
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
        let _publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        self.ensure_lifecycle_publication_available()?;
        match self.availability_management_state(instance_id).await? {
            AvailabilityManagementState::Managed => {}
            AvailabilityManagementState::PendingInitial => anyhow::bail!(
                "project instance {instance_id} is awaiting its initial availability publication"
            ),
            AvailabilityManagementState::LegacyUnmanaged => return Ok(()),
        }
        let mut availability = self.load_availability(instance_id).await?;
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
        let suppressed = self
            .service_stop_suppressions
            .lock()
            .await
            .iter()
            .filter_map(|(owner, name)| (*owner == instance_id).then_some(name.clone()))
            .collect::<HashSet<_>>();
        let runtimes = {
            let services = self.services.lock().await;
            services
                .iter()
                .filter(|(_, service)| service.instance_id == instance_id)
                .map(|(name, service)| {
                    (
                        name.clone(),
                        service.runtime_state.clone(),
                        service.health_status,
                    )
                })
                .collect::<Vec<_>>()
        };
        if runtimes.is_empty() {
            return false;
        }

        for (name, runtime, health_status) in runtimes {
            if suppressed.contains(&name) {
                continue;
            }
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

    async fn clear_service_stop_suppression(&self, instance_id: ProjectInstanceId, name: &str) {
        self.service_stop_suppressions
            .lock()
            .await
            .remove(&(instance_id, name.to_owned()));
    }

    async fn clear_service_stop_suppressions_for(
        &self,
        instance_id: ProjectInstanceId,
        service_names: &HashSet<String>,
    ) {
        self.service_stop_suppressions
            .lock()
            .await
            .retain(|(owner, name)| *owner != instance_id || !service_names.contains(name));
    }

    async fn clear_service_stop_suppressions(&self, instance_id: ProjectInstanceId) {
        self.service_stop_suppressions
            .lock()
            .await
            .retain(|(owner, _)| *owner != instance_id);
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
        manual_cli_session: Option<ManualCliSession>,
        legacy_cli_peer_pid: Option<u32>,
    ) -> Result<()> {
        let transition_guard = self.attachment_transition_lock.lock().await;
        self.ensure_accepting_lifecycle_requests()?;
        let coordinator = self.availability_coordinator(instance_id).await;
        let availability_guard = coordinator.runtime.lock().await;
        let publication_guard = self.lifecycle_publication_lock.lock().await;
        self.ensure_lifecycle_publication_available()?;
        let requested_path = Self::canonicalize_path(&requested_path);
        let target = match self.resolve_lifecycle_target(&requested_path).await? {
            LifecycleTargetResolution::Catalogued(target) if target.instance_id == instance_id => {
                target
            }
            LifecycleTargetResolution::Catalogued(target) => anyhow::bail!(
                "project identity changed during start: expected {instance_id}, discovered {}",
                target.instance_id
            ),
            LifecycleTargetResolution::UnregisteredPhysical {
                instance_id: discovered,
            } => anyhow::bail!(
                "project identity changed during start: expected {instance_id}, discovered unregistered physical instance {discovered}"
            ),
            LifecycleTargetResolution::UnresolvedLegacy | LifecycleTargetResolution::Ambiguous => {
                let active_path = self
                    .active_path_for_instance(instance_id)
                    .await
                    .context("catalogued project instance has no active path")?;
                match self.resolve_lifecycle_target(&active_path).await? {
                    LifecycleTargetResolution::Catalogued(target)
                        if target.instance_id == instance_id =>
                    {
                        target
                    }
                    LifecycleTargetResolution::Catalogued(target) => anyhow::bail!(
                        "catalogued project instance {instance_id} resolves to {} at its active path",
                        target.instance_id
                    ),
                    LifecycleTargetResolution::UnregisteredPhysical {
                        instance_id: discovered,
                    } => anyhow::bail!(
                        "catalogued project instance {instance_id} was replaced by unregistered physical instance {discovered} at its active path"
                    ),
                    LifecycleTargetResolution::UnresolvedLegacy
                    | LifecycleTargetResolution::Ambiguous => anyhow::bail!(
                        "catalogued project instance {instance_id} cannot be resolved at its active path"
                    ),
                }
            }
        };
        let lifecycle_paths = Self::catalogued_lifecycle_paths(&target);
        let effective_at = self.availability_now();
        let include_deferred = !self.availability_record_exists(instance_id).await?;
        let attachment_base = self.attachments.lock().await.snapshot();
        let mut attachment_target = attachment_base.clone();
        for path in &lifecycle_paths {
            if !(Self::attachment_path_is_owned_by_target(&attachment_base, &target, path)
                || include_deferred
                    && Self::attachment_path_can_initialize_target(&attachment_base, &target, path))
            {
                if path == &requested_path {
                    attachment_target.replace_project(path, Vec::new(), false);
                }
                continue;
            }
            let mut compatibility = attachment_target.project(path);
            if let Some(session) = manual_cli_session {
                let source = session.attachment_source();
                compatibility
                    .attachments
                    .retain(|attachment| attachment.source != source);
            }
            attachment_target.replace_project(path, compatibility.attachments, false);
        }
        let legacy_cli_demand = if manual_cli_session.is_none() {
            legacy_cli_peer_pid
                .and_then(|peer_pid| {
                    lifecycle_paths
                        .iter()
                        .filter(|path| {
                            Self::attachment_path_is_owned_by_target(
                                &attachment_base,
                                &target,
                                path,
                            ) || include_deferred
                                && Self::attachment_path_can_initialize_target(
                                    &attachment_base,
                                    &target,
                                    path,
                                )
                        })
                        .flat_map(|path| attachment_base.project(path).attachments)
                        .find_map(|attachment| match attachment.source {
                            AttachmentSource::CLI { pid } if pid == peer_pid => {
                                Some(AttachmentSource::CLI { pid })
                            }
                            AttachmentSource::Editor { .. }
                            | AttachmentSource::CLI { .. }
                            | AttachmentSource::ManualCLI(_)
                            | AttachmentSource::Runtime
                            | AttachmentSource::Pin => None,
                        })
                })
                .map(|source| availability_demand_for_attachment_source(&source))
                .transpose()?
                .flatten()
        } else {
            None
        };
        let mut batch = AvailabilityBatch::new(effective_at);
        if let Some(session) = manual_cli_session {
            let source = session.attachment_source();
            let mut compatibility = attachment_target.project(&requested_path).attachments;
            compatibility.retain(|attachment| attachment.source != source);
            compatibility.push(Attachment {
                project_path: requested_path.clone(),
                source: source.clone(),
                created_at: effective_at,
            });
            attachment_target.replace_project(&requested_path, compatibility, false);
            attachment_target.set_instance_owner(&requested_path, instance_id);
            batch.push(AvailabilityBatchOperation::EnsureDemand(
                manual_cli_session_demand(&session)?,
            ));
            batch.push(AvailabilityBatchOperation::EnsureDemand(
                availability_demand_for_attachment_source(&source)?
                    .context("Manual CLI session must have a process-bound demand")?,
            ));
        } else if let Some(demand) = legacy_cli_demand {
            // A pre-session CLI already published its process attachment on a
            // preceding connection. Kernel peer provenance lets this streamed
            // Start renew that exact owner instead of creating an ownerless
            // four-hour Manual demand that its legacy Detach cannot release.
            batch.push(AvailabilityBatchOperation::EnsureDemand(demand));
        } else {
            batch.push(AvailabilityBatchOperation::EnsureDemand(
                DemandKey::manual_cli(),
            ));
        }
        let transaction = self
            .prepare_project_lifecycle_transaction(
                *target,
                &batch,
                attachment_base,
                attachment_target,
            )
            .await?;
        self.create_and_apply_lifecycle_transaction_locked(&transaction)
            .await?;
        self.clear_service_stop_suppressions(instance_id).await;
        drop(publication_guard);
        drop(availability_guard);
        drop(transition_guard);

        if let Some(path) = self.active_path_for_instance(instance_id).await {
            self.watch_config(path).await;
        }
        self.converge_managed_instance(instance_id, event_tx, verbose, true)
            .await?;
        Ok(())
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

        match self.availability_management_state(instance_id).await? {
            AvailabilityManagementState::Managed => {
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
            AvailabilityManagementState::PendingInitial => return Ok(()),
            AvailabilityManagementState::LegacyUnmanaged => {}
        }

        let path = requested_path
            .or(self.active_path_for_instance(instance_id).await)
            .context("catalogued project instance has no active path")?;
        let action = self
            .apply_config_for_instance(path, None, false, Some(instance_id), true)
            .await;

        match self.availability_management_state(instance_id).await? {
            AvailabilityManagementState::Managed => {
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
            }
            AvailabilityManagementState::PendingInitial
            | AvailabilityManagementState::LegacyUnmanaged => action,
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
        self.converge_managed_instance_locked(instance_id, event_tx, verbose, apply_config_when_up)
            .await
    }

    async fn converge_managed_instance_locked(
        &self,
        instance_id: ProjectInstanceId,
        event_tx: Option<tokio::sync::mpsc::Sender<BootEvent>>,
        verbose: bool,
        apply_config_when_up: bool,
    ) -> Result<ConvergenceDecision> {
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
            let publication_guard = self.lifecycle_publication_lock.lock().await;
            self.ensure_lifecycle_publication_available()?;
            let mut availability = match self.load_availability(instance_id).await {
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
            drop(publication_guard);
            let project_path = requested_path
                .take()
                .or(self.active_path_for_instance(instance_id).await);

            let Some(project_path) = project_path else {
                let action = self.stop_project_instance(instance_id).await;
                let _publication_guard = self.lifecycle_publication_lock.lock().await;
                self.ensure_lifecycle_publication_available()?;
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
                    .finish_availability_action_locked(
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
                        let action = async {
                            self.apply_config_for_instance(
                                project_path,
                                event_tx.clone(),
                                options.verbose,
                                Some(instance_id),
                                false,
                            )
                            .await?;
                            anyhow::ensure!(
                                self.project_runtime_is_ready(instance_id).await,
                                "project instance {instance_id} did not become ready after availability convergence"
                            );
                            Ok(())
                        }
                        .await;
                        (action, true, true)
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

            let _publication_guard = self.lifecycle_publication_lock.lock().await;
            self.ensure_lifecycle_publication_available()?;
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
                .finish_availability_action_locked(
                    &mut availability,
                    decision,
                    action,
                    clear_on_success,
                )
                .await;
            return Self::surface_availability_durability(result, durability_error);
        }
    }

    async fn finish_availability_action_locked(
        &self,
        availability: &mut AvailabilityStore<SharedAvailabilityClock>,
        decision: ConvergenceDecision,
        action: Result<()>,
        clear_on_success: bool,
    ) -> Result<ConvergenceDecision> {
        self.ensure_lifecycle_publication_available()?;
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
        locald_core::normalize_project_locator(path).unwrap_or_else(|_| path.to_path_buf())
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

    async fn lock_config_transition_paths(
        &self,
        paths: &BTreeSet<PathBuf>,
    ) -> Vec<OwnedMutexGuard<()>> {
        let mut guards = Vec::with_capacity(paths.len());
        for path in paths {
            let (_, transition_lock) = self.transition_lock_for_path(path).await;
            guards.push(transition_lock.lock_owned().await);
        }
        guards
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

    async fn service_instance_id(&self, name: &str) -> Option<ProjectInstanceId> {
        let services = self.services.lock().await;
        services.get(name).map(|service| service.instance_id)
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
    use crate::state::StateSaveFault;
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
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::tempdir;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    const TEST_STARTUP_BOUNDARY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    const TEST_AVAILABILITY_START_SECONDS: u64 = 1_000_000;

    #[derive(Clone, Debug)]
    struct FakeAvailabilityClock {
        seconds: Arc<AtomicU64>,
    }

    impl FakeAvailabilityClock {
        fn new(seconds: u64) -> Self {
            Self {
                seconds: Arc::new(AtomicU64::new(seconds)),
            }
        }

        fn advance(&self, duration: Duration) {
            self.seconds.fetch_add(duration.as_secs(), Ordering::SeqCst);
        }

        fn time(&self) -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(self.seconds.load(Ordering::SeqCst))
        }
    }

    impl Clock for FakeAvailabilityClock {
        fn now(&self) -> SystemTime {
            self.time()
        }
    }

    #[test]
    fn compatibility_status_keeps_manual_cli_session_identity_private() {
        let project_path = PathBuf::from("/tmp/manual-cli-status");
        let session = ManualCliSession::new(42);
        let session_id = session.id().to_string();
        let mut snapshot = AttachmentStoreSnapshot::default();
        snapshot.replace_project(
            &project_path,
            vec![Attachment {
                project_path: project_path.clone(),
                source: session.attachment_source(),
                created_at: SystemTime::UNIX_EPOCH,
            }],
            false,
        );

        let visible =
            ProcessManager::compatibility_attachments_for_display(&snapshot, &project_path, None);

        assert!(matches!(
            visible.first().map(|attachment| &attachment.source),
            Some(AttachmentSource::CLI { pid: 42 })
        ));
        assert!(
            !serde_json::to_string(&visible)
                .expect("serialize compatibility status")
                .contains(&session_id)
        );
    }

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
        spawn_identity: Option<(u32, PersistedProcessIdentity)>,
        prepare_entered: Option<Arc<tokio::sync::Notify>>,
        release_prepare: Option<Arc<tokio::sync::Notify>>,
        start_entered: Option<Arc<tokio::sync::Notify>>,
        start_release: Option<Arc<tokio::sync::Notify>>,
        start_count: Option<Arc<AtomicUsize>>,
        fail_prepare: bool,
        stop_count: Arc<AtomicUsize>,
    }

    impl ScriptedController {
        fn materialize_spawn_identity(&mut self) {
            if let Some((pid, identity)) = self.spawn_identity.take() {
                self.state.pid = Some(pid);
                self.process_identity = Some(identity);
            }
        }
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
            if let Some(entered) = &self.prepare_entered {
                entered.notify_one();
            }
            if let Some(release) = &self.release_prepare {
                release.notified().await;
            }
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            if let Some(start_count) = &self.start_count {
                start_count.fetch_add(1, Ordering::SeqCst);
            }
            if let Some(entered) = &self.start_entered {
                entered.notify_one();
                if let Some(release) = &self.start_release {
                    release.notified().await;
                }
                self.materialize_spawn_identity();
                self.state.status = ServiceState::Running;
                return Ok(());
            }
            self.materialize_spawn_identity();
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
        release: Option<Arc<tokio::sync::Notify>>,
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
                spawn_identity: None,
                prepare_entered: None,
                release_prepare: None,
                start_entered: Some(self.entered.clone()),
                start_release: self.release.clone(),
                start_count: None,
                fail_prepare: false,
                stop_count: self.stop_count.clone(),
            }))
        }
    }

    #[derive(Debug)]
    struct BlockingPrepareFactory {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        start_count: Arc<AtomicUsize>,
        stop_count: Arc<AtomicUsize>,
    }

    impl ServiceFactory for BlockingPrepareFactory {
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
                    pid: None,
                    port: None,
                    status: ServiceState::Building,
                    health_status: HealthStatus::Unknown,
                },
                process_identity: None,
                spawn_identity: Some((
                    43,
                    test_process_identity(1_235, 43, "/test/blocking-prepare-worker"),
                )),
                prepare_entered: Some(self.entered.clone()),
                release_prepare: Some(self.release.clone()),
                start_entered: None,
                start_release: None,
                start_count: Some(self.start_count.clone()),
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
                spawn_identity: None,
                prepare_entered: None,
                release_prepare: None,
                start_entered: None,
                start_release: None,
                start_count: None,
                fail_prepare,
                stop_count: self.stop_count.clone(),
            }))
        }
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
                spawn_identity: Some((
                    44,
                    test_process_identity(1_236, 44, "/test/counting-start-worker"),
                )),
                prepare_entered: None,
                release_prepare: None,
                start_entered: None,
                start_release: None,
                start_count: None,
                fail_prepare: false,
                stop_count: Arc::new(AtomicUsize::new(0)),
            }))
        }
    }

    #[derive(Debug)]
    struct RetryingTcpReadinessFactory {
        creates: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
    }

    impl ServiceFactory for RetryingTcpReadinessFactory {
        fn can_handle(&self, config: &ServiceConfig) -> bool {
            matches!(
                config,
                ServiceConfig::Typed(TypedServiceConfig::Exec(_)) | ServiceConfig::Legacy(_)
            )
        }

        fn create(
            &self,
            name: String,
            _config: &ServiceConfig,
            ctx: &ServiceContext,
        ) -> Arc<Mutex<dyn ServiceController>> {
            let creation = self.creates.fetch_add(1, Ordering::SeqCst);
            Arc::new(Mutex::new(RetryingTcpReadinessController {
                id: name,
                port: ctx.port.expect("portful readiness fixture receives a port"),
                pid: 50 + u32::try_from(creation).expect("creation count fits in PID"),
                bind_on_start: creation > 0,
                running: false,
                listener: None,
                stops: self.stops.clone(),
            }))
        }
    }

    #[derive(Debug)]
    struct RetryingTcpReadinessController {
        id: String,
        port: u16,
        pid: u32,
        bind_on_start: bool,
        running: bool,
        listener: Option<tokio::net::TcpListener>,
        stops: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ServiceController for RetryingTcpReadinessController {
        fn id(&self) -> &str {
            &self.id
        }

        async fn prepare(&mut self) -> Result<()> {
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            self.running = true;
            if self.bind_on_start {
                self.listener = Some(
                    tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, self.port))
                        .await
                        .with_context(|| format!("bind readiness fixture on {}", self.port))?,
                );
            }
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            self.listener = None;
            self.running = false;
            Ok(())
        }

        async fn read_state(&self) -> RuntimeState {
            RuntimeState {
                pid: self.running.then_some(self.pid),
                port: Some(self.port),
                status: if self.running {
                    ServiceState::Running
                } else {
                    ServiceState::Stopped
                },
                health_status: if self.running {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unknown
                },
            }
        }

        fn owned_process_id(&self) -> Option<u32> {
            self.running.then_some(self.pid)
        }

        fn process_identity(&self) -> Option<PersistedProcessIdentity> {
            self.running.then(|| {
                test_process_identity(
                    2_000 + u64::from(self.pid),
                    i32::try_from(self.pid).expect("fixture PID fits i32"),
                    "/test/readiness-controller",
                )
            })
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
    struct BlockingFailOnceStopController {
        id: String,
        state: RuntimeState,
        stop_attempts: Arc<AtomicUsize>,
        first_stop_entered: Arc<tokio::sync::Notify>,
        release_first_stop: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl ServiceController for BlockingFailOnceStopController {
        fn id(&self) -> &str {
            &self.id
        }

        async fn prepare(&mut self) -> Result<()> {
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            self.state.status = ServiceState::Running;
            self.state.health_status = HealthStatus::Healthy;
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            let attempt = self.stop_attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                self.first_stop_entered.notify_one();
                self.release_first_stop.notified().await;
                anyhow::bail!("injected first stop failure");
            }
            self.state.pid = None;
            self.state.status = ServiceState::Stopped;
            self.state.health_status = HealthStatus::Unknown;
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
            Ok(None)
        }
    }

    #[derive(Debug)]
    struct BlockingSuccessfulStopController {
        id: String,
        state: RuntimeState,
        stop_entered: Arc<tokio::sync::Notify>,
        release_stop: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl ServiceController for BlockingSuccessfulStopController {
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
            self.stop_entered.notify_one();
            self.release_stop.notified().await;
            self.state.status = ServiceState::Stopped;
            self.state.health_status = HealthStatus::Unknown;
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
        #[cfg(target_os = "macos")]
        let mut domains = service_domains
            .iter()
            .filter(|domain| **domain != "localhost" && !domain.ends_with(".localhost"))
            .map(|domain| (*domain).to_owned())
            .collect::<Vec<_>>();
        #[cfg(not(target_os = "macos"))]
        let mut domains = vec![
            "dev.docs.local".to_owned(),
            "dev.locald.local".to_owned(),
            "docs.local".to_owned(),
            "locald.local".to_owned(),
        ];
        #[cfg(not(target_os = "macos"))]
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
        availability_manager_with_clock(
            root,
            project_path,
            project_name,
            SharedAvailabilityClock::system(),
        )
        .await
    }

    fn unregistered_availability_manager(root: &Path) -> ProcessManager {
        let mut manager = ProcessManager::new_with_availability_data_dir(
            root.join("notify.sock"),
            Arc::new(StateManager::with_path(root.join("state.json"))),
            Arc::new(Mutex::new(Registry::with_path(root.join("catalog.json")))),
            Arc::new(Mutex::new(AttachmentStore::new(
                root.join("attachments.json"),
            ))),
            None,
            root.join("availability-data"),
        )
        .expect("create unregistered availability manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager
    }

    async fn availability_manager_with_clock(
        root: &Path,
        project_path: &Path,
        project_name: &str,
        clock: SharedAvailabilityClock,
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
        let mut manager = ProcessManager::new_with_availability_data_dir_and_clock(
            root.join("notify.sock"),
            Arc::new(StateManager::with_path(root.join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                root.join("attachments.json"),
            ))),
            None,
            availability_data_dir.clone(),
            clock,
        )
        .expect("create availability process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        (manager, instance_id, availability_data_dir)
    }

    async fn reopen_availability_manager_with_clock(
        root: &Path,
        availability_data_dir: PathBuf,
        clock: SharedAvailabilityClock,
    ) -> ProcessManager {
        let catalog = Registry::load_from_paths(locald_core::CatalogPaths::for_data_dir(root))
            .await
            .expect("reload availability catalog");
        let mut manager = ProcessManager::new_with_availability_data_dir_and_clock(
            root.join("reopened-notify.sock"),
            Arc::new(StateManager::with_path(root.join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                root.join("attachments.json"),
            ))),
            None,
            availability_data_dir,
            clock,
        )
        .expect("reopen availability process manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager
    }

    async fn journal_transaction_at_phase(
        manager: &ProcessManager,
        transaction: &LifecycleTransaction,
        target_phase: LifecycleTransactionPhase,
    ) -> LifecycleTransaction {
        manager
            .lifecycle_journal
            .create(transaction)
            .await
            .expect("create lifecycle transaction fixture");
        let mut phase = LifecycleTransactionPhase::Prepared;
        while phase != target_phase {
            let next = phase.next().expect("target phase follows prepared");
            manager
                .lifecycle_journal
                .advance(transaction.id(), phase, next)
                .await
                .expect("advance lifecycle transaction fixture");
            phase = next;
        }
        let mut loaded = manager
            .lifecycle_journal
            .load()
            .await
            .expect("load lifecycle transaction fixture")
            .expect("lifecycle transaction fixture exists");
        let catalog_path = manager.registry.lock().await.storage_path().to_path_buf();
        loaded.normalize_catalog_storage_path(&catalog_path);
        loaded
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

    fn write_unready_availability_worker_config(
        project_path: &Path,
        project_name: &str,
        domain: &str,
        service_names: &[&str],
    ) {
        let mut config = format!("[project]\nname = \"{project_name}\"\ndomain = \"{domain}\"\n");
        for service_name in service_names {
            config.push_str(&format!(
                "\n[services.{service_name}]\ntype = \"worker\"\ncommand = \"unused-by-test-factory\"\nhealth_check = \"false\"\n"
            ));
        }
        std::fs::write(project_path.join("locald.toml"), config)
            .expect("write unready availability worker config");
    }

    struct UnregisteredReplacementFixture {
        manager: ProcessManager,
        repository: PathBuf,
        worktree: PathBuf,
        historical_instance: ProjectInstanceId,
        replacement_instance: ProjectInstanceId,
        availability_data_dir: PathBuf,
        host_calls: Arc<StdMutex<Vec<Vec<String>>>>,
    }

    async fn unregistered_replacement_fixture(root: &Path) -> UnregisteredReplacementFixture {
        let repository = root.join("replacement-repository");
        std::fs::create_dir(&repository).expect("create replacement repository");
        git(&repository, &["init", "-b", "main"]);
        git(&repository, &["config", "user.name", "locald tests"]);
        git(
            &repository,
            &["config", "user.email", "locald@example.test"],
        );
        std::fs::write(repository.join("README.md"), "fixture\n")
            .expect("write replacement fixture");
        write_availability_worker_config(
            &repository,
            "historical",
            "historical.localhost",
            &["web"],
        );
        git(&repository, &["add", "README.md", "locald.toml"]);
        git(&repository, &["commit", "-m", "initial"]);

        let worktree = root.join("replacement-worktree");
        let worktree_arg = worktree.to_str().expect("UTF-8 replacement worktree path");
        git(
            &repository,
            &["worktree", "add", "-b", "historical", worktree_arg],
        );
        let canonical =
            std::fs::canonicalize(&worktree).expect("canonical historical worktree path");
        let catalog_path = root.join("catalog.json");
        let mut catalog = Registry::with_path(catalog_path);
        let historical_instance = catalog
            .register_project(
                Registry::discover(canonical.clone())
                    .await
                    .expect("discover historical worktree"),
                Some("historical".to_owned()),
            )
            .expect("register historical worktree");
        assert!(catalog.pin_project(&canonical));
        catalog
            .replace_domain_claims(
                historical_instance,
                [DomainClaim::service(
                    "historical.localhost"
                        .parse()
                        .expect("valid historical domain"),
                    historical_instance,
                    "historical:web".to_owned(),
                )],
            )
            .expect("install historical domain claim");
        catalog.save().await.expect("persist historical catalog");

        let attachments_path = root.join("attachments.json");
        let mut attachments = AttachmentStore::new(attachments_path);
        let created_at = SystemTime::now();
        attachments.replace_project(
            &canonical,
            vec![
                Attachment {
                    project_path: canonical.clone(),
                    source: AttachmentSource::Pin,
                    created_at,
                },
                Attachment {
                    project_path: canonical.clone(),
                    source: AttachmentSource::Editor {
                        name: "vscode".to_owned(),
                        id: "historical-window".to_owned(),
                        pid: None,
                    },
                    created_at,
                },
                Attachment {
                    project_path: canonical.clone(),
                    source: AttachmentSource::CLI { pid: u32::MAX },
                    created_at,
                },
            ],
            false,
        );
        attachments.set_instance_owner(&canonical, historical_instance);
        attachments
            .save()
            .await
            .expect("persist historical attachments");

        let availability_data_dir = root.join("availability-data");
        let mut availability = AvailabilityStore::load(&availability_data_dir, historical_instance)
            .await
            .expect("load historical availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable historical Always On");
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("seed historical demand");

        let mut manager = ProcessManager::new_with_availability_data_dir(
            root.join("notify.sock"),
            Arc::new(StateManager::with_path(root.join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(attachments)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create unregistered replacement manager");
        let host_calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: Arc::clone(&host_calls),
        }));
        manager.services.lock().await.insert(
            "historical:web".to_owned(),
            availability_test_service(historical_instance, "historical", &canonical, false),
        );
        manager
            .persist_state_checked()
            .await
            .expect("persist historical runtime state");

        git(&repository, &["worktree", "remove", worktree_arg]);
        git(&repository, &["worktree", "prune"]);
        git(
            &repository,
            &["worktree", "add", "-b", "replacement", worktree_arg],
        );
        let replacement_discovery = Registry::discover(
            std::fs::canonicalize(&worktree).expect("canonical replacement worktree path"),
        )
        .await
        .expect("discover replacement worktree");
        let mut probe_catalog = manager.registry.lock().await.clone();
        let replacement_instance = probe_catalog
            .register_project(replacement_discovery, Some("replacement".to_owned()))
            .expect("derive replacement instance identity");
        assert_ne!(historical_instance, replacement_instance);

        UnregisteredReplacementFixture {
            manager,
            repository,
            worktree,
            historical_instance,
            replacement_instance,
            availability_data_dir,
            host_calls,
        }
    }

    #[tokio::test]
    async fn legacy_start_journals_manual_demand_and_clears_stop_mirror() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-start-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "legacy-start").await;
        write_availability_worker_config(
            &project_path,
            "legacy-start",
            "legacy-start.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        {
            let mut attachments = manager.attachments.lock().await;
            attachments.mark_stopped(&project_path);
            attachments.save().await.expect("persist stop mirror");
        }

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("legacy Start should ensure manual availability");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load started availability");
        let snapshot = availability
            .snapshot()
            .await
            .expect("read started availability");
        assert!(
            snapshot
                .demands()
                .iter()
                .any(|demand| demand.key().kind() == locald_core::DemandKind::ManualCli)
        );
        assert!(!snapshot.is_paused());
        assert!(!manager.attachments.lock().await.is_stopped(&project_path));
        assert!(
            manager
                .get_service_controller("legacy-start:web")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn legacy_cli_attach_defers_convergence_to_its_streamed_start() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-streamed-start-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "legacy-streamed-start").await;
        write_availability_worker_config(
            &project_path,
            "legacy-streamed-start",
            "legacy-streamed-start.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let peer_pid = std::process::id();
        let source = AttachmentSource::CLI { pid: peer_pid };
        let process_demand = availability_demand_for_attachment_source(&source)
            .expect("map legacy CLI owner")
            .expect("legacy CLI owner has a process demand");

        manager
            .project_attach_from_ipc(project_path.clone(), source.clone(), false)
            .await
            .expect("publish the pre-session CLI owner");
        assert!(
            manager
                .get_service_controller("legacy-streamed-start:web")
                .await
                .is_none(),
            "legacy attach must leave startup to the following streamed Start"
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load attached availability");
        let attached = availability
            .snapshot()
            .await
            .expect("read attached availability");
        assert!(
            attached
                .demands()
                .iter()
                .any(|demand| demand.key() == &process_demand)
        );
        assert!(
            attached
                .demands()
                .iter()
                .all(|demand| demand.key() != &DemandKey::manual_cli())
        );

        manager
            .project_force_stop(project_path.clone())
            .await
            .expect("pause the pre-session owner generation");
        let paused = availability.snapshot().await.expect("read paused owner");
        assert!(paused.is_paused());

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);
        manager
            .start_from_ipc(
                project_path.clone(),
                Some(event_tx),
                false,
                None,
                Some(peer_pid),
            )
            .await
            .expect("matching legacy Start resumes and converges the process owner");
        assert!(
            manager
                .get_service_controller("legacy-streamed-start:web")
                .await
                .is_some()
        );
        assert!(
            std::iter::from_fn(|| event_rx.try_recv().ok()).any(|event| matches!(
                event,
                BootEvent::StepStarted { id, .. } if id == "legacy-streamed-start:web"
            )),
            "service startup must be visible to the streamed Start"
        );
        let started = availability
            .snapshot()
            .await
            .expect("read matching legacy Start availability");
        assert!(!started.is_paused());
        assert_eq!(
            started.activity_generation(),
            paused.activity_generation() + 1,
            "matching streamed Start contributes one semantic activity generation"
        );
        assert!(
            started
                .demands()
                .iter()
                .any(|demand| demand.key() == &process_demand)
        );
        assert!(
            started
                .demands()
                .iter()
                .all(|demand| demand.key() != &DemandKey::manual_cli())
        );

        manager
            .project_detach(project_path, Some(source))
            .await
            .expect("legacy Ctrl-C releases the process owner");
        let detached = availability
            .snapshot()
            .await
            .expect("read detached legacy owner");
        assert!(detached.demands().is_empty());
        assert!(detached.shutdown_cooldown_until().is_some());
    }

    #[tokio::test]
    async fn mismatched_legacy_start_peer_keeps_an_independent_manual_demand() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("mismatched-legacy-peer-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "mismatched-legacy-peer").await;
        write_availability_worker_config(
            &project_path,
            "mismatched-legacy-peer",
            "mismatched-legacy-peer.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let attached_pid = std::process::id();
        let different_peer_pid = attached_pid
            .checked_add(1)
            .unwrap_or_else(|| attached_pid.saturating_sub(1));
        let source = AttachmentSource::CLI { pid: attached_pid };

        manager
            .project_attach_from_ipc(project_path.clone(), source.clone(), false)
            .await
            .expect("publish unrelated legacy CLI owner");
        manager
            .start_from_ipc(
                project_path.clone(),
                None,
                false,
                None,
                Some(different_peer_pid),
            )
            .await
            .expect("mismatched legacy Start acquires independent Manual demand");
        manager
            .project_detach(project_path, Some(source))
            .await
            .expect("detach unrelated legacy CLI owner");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load mismatched-peer availability");
        let detached = availability
            .snapshot()
            .await
            .expect("read mismatched-peer availability");
        assert!(
            detached
                .demands()
                .iter()
                .any(|demand| demand.key() == &DemandKey::manual_cli())
        );
        assert!(detached.demands().iter().all(|demand| {
            demand.key().kind() != locald_core::DemandKind::LegacyProcessAttachment
        }));
    }

    #[tokio::test]
    async fn standalone_cli_attach_converges_immediately() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("standalone-cli-attach-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "standalone-cli-attach").await;
        write_availability_worker_config(
            &project_path,
            "standalone-cli-attach",
            "standalone-cli-attach.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        manager
            .project_attach_from_ipc(
                project_path,
                AttachmentSource::CLI {
                    pid: std::process::id(),
                },
                true,
            )
            .await
            .expect("standalone CLI attach converges its demand");

        assert!(
            manager
                .get_service_controller("standalone-cli-attach:web")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn fieldless_cli_attach_converges_after_an_unpaired_owner_exits() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("fieldless-cli-attach-project");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "fieldless-cli-attach").await;
        write_availability_worker_config(
            &project_path,
            "fieldless-cli-attach",
            "fieldless-cli-attach.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let prior_owner = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "prior-window".to_owned(),
            pid: None,
        };

        manager
            .project_attach(project_path.clone(), prior_owner.clone())
            .await
            .expect("start project under prior owner");
        manager
            .stop("fieldless-cli-attach:web")
            .await
            .expect("stop service under prior owner");
        manager
            .project_detach(project_path.clone(), Some(prior_owner))
            .await
            .expect("release prior owner");
        assert!(
            manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "fieldless-cli-attach:web".to_owned()))
        );

        manager
            .project_attach_from_ipc(project_path, AttachmentSource::CLI { pid: u32::MAX }, false)
            .await
            .expect("publish field-less CLI attachment");
        assert!(
            manager
                .get_service_controller("fieldless-cli-attach:web")
                .await
                .is_none(),
            "field-less attachment initially reserves startup for a paired Start"
        );
        assert!(
            !manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "fieldless-cli-attach:web".to_owned())),
            "new field-less owner resumes automatic service management at publication"
        );

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if manager
                    .get_service_controller("fieldless-cli-attach:web")
                    .await
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("an exited unpaired owner triggers standalone convergence");
    }

    #[tokio::test]
    async fn cli_detach_releases_the_manual_demand_created_by_legacy_start() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("cli-detach-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "cli-detach").await;
        write_availability_worker_config(
            &project_path,
            "cli-detach",
            "cli-detach.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let session = ManualCliSession::new(std::process::id());
        let source = session.attachment_source();

        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(session))
            .await
            .expect("paired Start publishes the Manual CLI session");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load CLI availability");
        let attached = availability
            .snapshot()
            .await
            .expect("read attached CLI demands");
        let session_demand =
            manual_cli_session_demand(&session).expect("manual CLI session demand should be valid");
        assert!(
            attached
                .demands()
                .iter()
                .any(|demand| demand.key() == &session_demand)
        );
        assert!(attached.demands().iter().any(|demand| {
            demand.key().kind() == locald_core::DemandKind::LegacyProcessAttachment
        }));

        manager
            .project_detach(project_path, Some(source))
            .await
            .expect("Ctrl-C compatibility detach releases the CLI session");

        let detached = availability
            .snapshot()
            .await
            .expect("read detached CLI demands");
        assert!(detached.demands().is_empty());
        assert!(detached.shutdown_cooldown_until().is_some());
    }

    #[tokio::test]
    async fn retried_paired_start_reuses_one_manual_cli_owner_and_demand() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("retried-cli-start-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "retried-cli-start").await;
        write_availability_worker_config(
            &project_path,
            "retried-cli-start",
            "retried-cli-start.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let session = ManualCliSession::new(std::process::id());

        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(session))
            .await
            .expect("publish paired Manual CLI start");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load first paired-start availability");
        let first = availability
            .snapshot()
            .await
            .expect("read first paired-start availability");

        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(session))
            .await
            .expect("retry the same paired Manual CLI start");
        let retried = availability
            .snapshot()
            .await
            .expect("read retried paired-start availability");
        assert_eq!(retried.activity_generation(), first.activity_generation());
        assert_eq!(retried.demands().len(), 2);
        let session_demand = manual_cli_session_demand(&session)
            .expect("construct retried Manual CLI session demand");
        let process_demand =
            availability_demand_for_attachment_source(&session.attachment_source())
                .expect("construct retried process demand")
                .expect("Manual CLI session has a process demand");
        assert!(
            retried
                .demands()
                .iter()
                .any(|lease| lease.key() == &session_demand)
        );
        assert!(
            retried
                .demands()
                .iter()
                .any(|lease| lease.key() == &process_demand)
        );
        let attachments = manager
            .attachments
            .lock()
            .await
            .snapshot()
            .project(&project_path)
            .attachments;
        assert_eq!(
            attachments
                .iter()
                .filter(|attachment| attachment.source == session.attachment_source())
                .count(),
            1
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up retried paired-start project");
    }

    #[tokio::test]
    async fn detaching_one_manual_cli_session_preserves_other_owners() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("multiple-cli-session-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "multiple-cli-session").await;
        write_availability_worker_config(
            &project_path,
            "multiple-cli-session",
            "multiple-cli-session.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let first_session = ManualCliSession::new(std::process::id());
        let second_session = ManualCliSession::new(std::process::id().saturating_add(1));
        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(first_session))
            .await
            .expect("publish first Manual CLI session");
        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(second_session))
            .await
            .expect("publish second Manual CLI session");

        manager
            .project_detach(
                project_path.clone(),
                Some(first_session.attachment_source()),
            )
            .await
            .expect("detach only the first Manual CLI session");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load multiple-session availability");
        let detached = availability
            .snapshot()
            .await
            .expect("read availability after one session detaches");
        let first_demand = manual_cli_session_demand(&first_session)
            .expect("construct first Manual CLI session demand");
        let second_demand = manual_cli_session_demand(&second_session)
            .expect("construct second Manual CLI session demand");
        let first_process =
            availability_demand_for_attachment_source(&first_session.attachment_source())
                .expect("construct first process demand")
                .expect("first session has a process demand");
        let second_process =
            availability_demand_for_attachment_source(&second_session.attachment_source())
                .expect("construct second process demand")
                .expect("second session has a process demand");
        assert!(
            detached
                .demands()
                .iter()
                .all(|lease| lease.key() != &first_demand)
        );
        assert!(
            detached
                .demands()
                .iter()
                .all(|lease| lease.key() != &first_process)
        );
        assert!(
            detached
                .demands()
                .iter()
                .any(|lease| lease.key() == &second_demand)
        );
        assert!(
            detached
                .demands()
                .iter()
                .any(|lease| lease.key() == &second_process)
        );
        assert!(detached.desired_up_at(SystemTime::now()));
        assert!(
            manager
                .get_service_controller("multiple-cli-session:web")
                .await
                .is_some()
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up multiple-session project");
    }

    #[tokio::test]
    async fn manual_cli_detach_preserves_independent_singleton_manual_policy() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("manual-policy-cli-session-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "manual-policy-cli-session").await;
        write_availability_worker_config(
            &project_path,
            "manual-policy-cli-session",
            "manual-policy-cli-session.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("publish independent singleton Manual demand");
        let session = ManualCliSession::new(std::process::id());
        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(session))
            .await
            .expect("publish paired Manual CLI session");

        manager
            .project_detach(project_path.clone(), Some(session.attachment_source()))
            .await
            .expect("detach paired Manual CLI session");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load singleton policy availability");
        let detached = availability
            .snapshot()
            .await
            .expect("read singleton policy availability");
        assert!(
            detached
                .demands()
                .iter()
                .any(|lease| lease.key() == &DemandKey::manual_cli())
        );
        let session_demand = manual_cli_session_demand(&session)
            .expect("construct detached Manual CLI session demand");
        assert!(
            detached
                .demands()
                .iter()
                .all(|lease| lease.key() != &session_demand)
        );
        assert!(detached.desired_up_at(SystemTime::now()));

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up singleton policy project");
    }

    #[tokio::test]
    async fn dead_manual_cli_session_reaper_releases_both_owned_demands() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("dead-cli-session-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "dead-cli-session").await;
        write_availability_worker_config(
            &project_path,
            "dead-cli-session",
            "dead-cli-session.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let session = ManualCliSession::new(u32::MAX);
        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(session))
            .await
            .expect("publish eventually stale Manual CLI session");
        let evidence = manager
            .attachments
            .lock()
            .await
            .snapshot()
            .compatibility_evidence_at(
                &ProcessManager::canonicalize_path(&project_path),
                SystemTime::now(),
                ProcessManager::legacy_pid_alive,
            );
        assert_eq!(evidence.attachments.len(), 1);
        assert!(!evidence.attachments[0].alive);

        manager
            .reconcile_legacy_attachment_project(project_path.clone())
            .await
            .expect("reap dead Manual CLI session");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load reaped Manual CLI availability");
        let reaped = availability
            .snapshot()
            .await
            .expect("read reaped Manual CLI availability");
        assert!(
            reaped.demands().is_empty(),
            "dead session left availability demands: {:#?}",
            reaped.demands()
        );
        assert!(reaped.shutdown_cooldown_until().is_some());
        assert!(
            manager
                .attachments
                .lock()
                .await
                .snapshot()
                .project(&project_path)
                .attachments
                .is_empty()
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up reaped Manual CLI project");
    }

    #[tokio::test]
    async fn new_attachment_reaping_a_dead_manual_cli_session_releases_both_owned_demands() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("reattached-dead-cli-session-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "reattached-dead-cli-session").await;
        write_availability_worker_config(
            &project_path,
            "reattached-dead-cli-session",
            "reattached-dead-cli-session.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let stale_session = ManualCliSession::new(u32::MAX);
        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(stale_session))
            .await
            .expect("publish eventually stale Manual CLI session");
        let editor = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "replacement-window".to_owned(),
            pid: Some(std::process::id()),
        };

        manager
            .project_attach(project_path.clone(), editor.clone())
            .await
            .expect("new editor attachment reaps the stale Manual CLI session");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load reattached availability");
        let reattached = availability
            .snapshot()
            .await
            .expect("read reattached availability");
        let stale_manual =
            manual_cli_session_demand(&stale_session).expect("construct stale Manual CLI demand");
        let stale_process =
            availability_demand_for_attachment_source(&stale_session.attachment_source())
                .expect("construct stale process demand")
                .expect("Manual CLI session has a process demand");
        assert!(
            reattached
                .demands()
                .iter()
                .all(|lease| lease.key() != &stale_manual && lease.key() != &stale_process)
        );
        assert!(
            reattached
                .demands()
                .iter()
                .any(|lease| { lease.key().kind() == locald_core::DemandKind::VsCodeWindow })
        );

        manager
            .project_detach(project_path, Some(editor))
            .await
            .expect("detach replacement editor");
        let detached = availability
            .snapshot()
            .await
            .expect("read fully detached availability");
        assert!(
            detached.demands().is_empty(),
            "replacement detach left orphan demands: {:#?}",
            detached.demands()
        );
        assert!(detached.shutdown_cooldown_until().is_some());
    }

    #[tokio::test]
    async fn bulk_detach_releases_a_removed_manual_cli_session() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("bulk-cli-detach-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "bulk-cli-detach").await;
        write_availability_worker_config(
            &project_path,
            "bulk-cli-detach",
            "bulk-cli-detach.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let session = ManualCliSession::new(std::process::id());
        manager
            .start_with_manual_cli_session(project_path.clone(), None, false, Some(session))
            .await
            .expect("paired Start publishes the Manual CLI session");
        manager
            .project_detach(project_path, None)
            .await
            .expect("bulk detach releases every compatibility owner");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load bulk-detach availability");
        let detached = availability
            .snapshot()
            .await
            .expect("read bulk-detached availability");
        assert!(detached.demands().is_empty());
        assert!(detached.shutdown_cooldown_until().is_some());
    }

    #[tokio::test]
    async fn generic_cli_detach_preserves_an_independently_owned_manual_demand() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("generic-cli-detach-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "generic-cli-detach").await;
        write_availability_worker_config(
            &project_path,
            "generic-cli-detach",
            "generic-cli-detach.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let source = AttachmentSource::CLI {
            pid: std::process::id(),
        };

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("legacy Start acquires an independent Manual demand");
        manager
            .project_attach(project_path.clone(), source.clone())
            .await
            .expect("attach generic compatibility CLI owner");
        manager
            .project_detach(project_path, Some(source))
            .await
            .expect("detach generic compatibility owner");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load generic CLI availability");
        let detached = availability
            .snapshot()
            .await
            .expect("read availability after generic CLI detach");
        assert!(
            detached
                .demands()
                .iter()
                .any(|demand| demand.key() == &DemandKey::manual_cli())
        );
        assert!(detached.demands().iter().all(|demand| {
            demand.key().kind() != locald_core::DemandKind::LegacyProcessAttachment
        }));
    }

    #[tokio::test]
    async fn editor_attach_and_detach_project_exact_availability_owner() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("editor-owner-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "editor-owner").await;
        write_availability_worker_config(
            &project_path,
            "editor-owner",
            "editor-owner.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let source = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "window-a".to_owned(),
            pid: None,
        };

        manager
            .project_attach(project_path.clone(), source.clone())
            .await
            .expect("attach editor owner");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load editor availability");
        assert!(
            availability
                .snapshot()
                .await
                .expect("read editor demand")
                .demands()
                .iter()
                .any(|demand| demand.key().kind() == locald_core::DemandKind::VsCodeWindow)
        );

        manager
            .project_detach(project_path.clone(), Some(source))
            .await
            .expect("detach exact editor owner");
        let detached = availability
            .snapshot()
            .await
            .expect("read detached availability");
        assert!(detached.demands().is_empty());
        assert!(detached.shutdown_cooldown_until().is_some());
    }

    #[tokio::test]
    async fn compatibility_reattach_revalidates_existing_owner_without_crossing_pause() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("editor-reattach-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "editor-reattach").await;
        write_availability_worker_config(
            &project_path,
            "editor-reattach",
            "editor-reattach.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let source = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "window-a".to_owned(),
            pid: None,
        };

        manager
            .project_attach(project_path.clone(), source.clone())
            .await
            .expect("attach editor owner");
        manager
            .project_force_stop(project_path.clone())
            .await
            .expect("pause attached project");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load paused availability");
        let paused = availability
            .snapshot()
            .await
            .expect("read paused availability");
        assert_eq!(paused.activity_generation(), 1);
        assert!(paused.is_paused());

        manager
            .project_attach(project_path.clone(), source)
            .await
            .expect("rediscover existing editor owner");
        let revalidated = availability
            .snapshot()
            .await
            .expect("read revalidated availability");
        assert_eq!(revalidated.activity_generation(), 1);
        assert!(revalidated.is_paused());
        assert!(!revalidated.desired_up_at(SystemTime::now()));
        assert!(manager.attachments.lock().await.is_stopped(&project_path));
    }

    #[tokio::test]
    async fn startup_revalidates_an_expired_live_owner_without_crossing_pause() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("startup-owner-revalidation-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "startup-owner-revalidation").await;
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("seed completed migration authority");
        let source = AttachmentSource::CLI {
            pid: std::process::id(),
        };
        let demand = availability_demand_for_attachment_source(&source)
            .expect("map live process owner")
            .expect("CLI owner has a demand");
        let expired_at = SystemTime::now()
            .checked_sub(locald_core::LEGACY_PROCESS_DEMAND_TTL + std::time::Duration::from_secs(1))
            .expect("construct expired lease time");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load startup owner availability");
        availability
            .apply_batch(
                &AvailabilityBatch::new(expired_at)
                    .with_operation(AvailabilityBatchOperation::EnsureDemand(demand.clone())),
            )
            .await
            .expect("seed expired process demand");
        availability
            .pause_project()
            .await
            .expect("pause startup owner generation");
        let before = availability
            .snapshot()
            .await
            .expect("read paused expired demand");
        assert!(before.is_paused());
        assert!(before.live_demands_at(SystemTime::now()).next().is_none());
        {
            let mut attachments = manager.attachments.lock().await;
            attachments.replace_project(
                &project_path,
                vec![Attachment {
                    project_path: project_path.clone(),
                    source,
                    created_at: expired_at,
                }],
                true,
            );
            attachments.set_instance_owner(&project_path, instance_id);
        }

        manager
            .reconcile_legacy_attachment_owners()
            .await
            .expect("revalidate live compatibility owner before first sweep");
        manager.converge_all_project_availability().await;

        let after = availability
            .snapshot()
            .await
            .expect("read revalidated paused demand");
        assert_eq!(after.activity_generation(), before.activity_generation());
        assert!(after.is_paused());
        assert!(!after.desired_up_at(SystemTime::now()));
        let lease = after
            .demands()
            .iter()
            .find(|lease| lease.key() == &demand)
            .expect("revalidated demand remains present");
        assert!(
            lease
                .expires_at()
                .is_some_and(|expiry| expiry > SystemTime::now())
        );
    }

    #[tokio::test]
    async fn owner_reconciliation_publishes_without_entering_runtime_prepare() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("publication-only-owner-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "publication-only-owner").await;
        write_availability_worker_config(
            &project_path,
            "publication-only-owner",
            "publication-only-owner.localhost",
            &["web"],
        );
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("seed completed migration authority");
        let source = AttachmentSource::CLI {
            pid: std::process::id(),
        };
        let demand = availability_demand_for_attachment_source(&source)
            .expect("map live process owner")
            .expect("CLI owner has a demand");
        let expired_at = SystemTime::now()
            .checked_sub(locald_core::LEGACY_PROCESS_DEMAND_TTL + std::time::Duration::from_secs(1))
            .expect("construct expired lease time");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load publication-only availability");
        availability
            .apply_batch(
                &AvailabilityBatch::new(expired_at)
                    .with_operation(AvailabilityBatchOperation::EnsureDemand(demand.clone())),
            )
            .await
            .expect("seed expired process demand");
        {
            let mut attachments = manager.attachments.lock().await;
            attachments.replace_project(
                &project_path,
                vec![Attachment {
                    project_path: project_path.clone(),
                    source,
                    created_at: expired_at,
                }],
                false,
            );
            attachments.set_instance_owner(&project_path, instance_id);
        }
        let prepare_entered = Arc::new(tokio::sync::Notify::new());
        let release_prepare = Arc::new(tokio::sync::Notify::new());
        manager.factories.insert(
            0,
            Arc::new(BlockingPrepareFactory {
                entered: prepare_entered.clone(),
                release: release_prepare.clone(),
                start_count: Arc::new(AtomicUsize::new(0)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        tokio::time::timeout(
            TEST_STARTUP_BOUNDARY_TIMEOUT,
            manager.reconcile_legacy_attachment_owners(),
        )
        .await
        .expect("owner publication returns before runtime prepare")
        .expect("owner publication succeeds");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                prepare_entered.notified()
            )
            .await
            .is_err()
        );
        let renewed = availability
            .snapshot()
            .await
            .expect("read publication-only renewal");
        assert!(
            renewed
                .demands()
                .iter()
                .find(|lease| lease.key() == &demand)
                .and_then(locald_core::DemandLease::expires_at)
                .is_some_and(|expiry| expiry > SystemTime::now())
        );

        let convergence = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .converge_managed_instance(instance_id, None, false, true)
                    .await
            }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            prepare_entered.notified(),
        )
        .await
        .expect("explicit convergence enters runtime prepare");
        release_prepare.notify_one();
        convergence
            .await
            .expect("convergence task joins")
            .expect("convergence completes after prepare release");
    }

    #[tokio::test]
    async fn broken_legacy_owner_does_not_starve_later_owner_revalidation() {
        let dir = tempdir().expect("create temporary directory");
        let broken_path = dir.path().join("a-broken-owner-project");
        let healthy_path = dir.path().join("z-healthy-owner-project");
        std::fs::create_dir_all(&broken_path).expect("create broken owner project");
        std::fs::create_dir_all(&healthy_path).expect("create healthy owner project");
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let broken_instance = catalog
            .register_project(
                Registry::discover(broken_path.clone())
                    .await
                    .expect("discover broken owner project"),
                Some("broken-owner".to_owned()),
            )
            .expect("register broken owner project");
        let healthy_instance = catalog
            .register_project(
                Registry::discover(healthy_path.clone())
                    .await
                    .expect("discover healthy owner project"),
                Some("healthy-owner".to_owned()),
            )
            .expect("register healthy owner project");
        catalog.save().await.expect("save owner catalog");
        let availability_data_dir = dir.path().join("availability-data");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
            availability_data_dir.clone(),
        )
        .expect("create owner reconciliation manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("seed completed migration authority");

        let source = AttachmentSource::CLI {
            pid: std::process::id(),
        };
        let demand = availability_demand_for_attachment_source(&source)
            .expect("map owner demand")
            .expect("CLI owner has a demand");
        let expired_at = SystemTime::now()
            .checked_sub(locald_core::LEGACY_PROCESS_DEMAND_TTL + std::time::Duration::from_secs(1))
            .expect("construct expired owner lease time");
        let mut healthy = AvailabilityStore::load(&availability_data_dir, healthy_instance)
            .await
            .expect("load healthy owner availability");
        healthy
            .apply_batch(
                &AvailabilityBatch::new(expired_at)
                    .with_operation(AvailabilityBatchOperation::EnsureDemand(demand.clone())),
            )
            .await
            .expect("seed expired healthy owner demand");
        {
            let mut attachments = manager.attachments.lock().await;
            for (path, instance_id) in [
                (&broken_path, broken_instance),
                (&healthy_path, healthy_instance),
            ] {
                attachments.replace_project(
                    path,
                    vec![Attachment {
                        project_path: path.clone(),
                        source: source.clone(),
                        created_at: expired_at,
                    }],
                    false,
                );
                attachments.set_instance_owner(path, instance_id);
            }
        }

        let error = manager
            .reconcile_legacy_attachment_owners()
            .await
            .expect_err("broken owner remains a strict aggregate failure");
        assert!(format!("{error:#}").contains("a-broken-owner-project"));
        assert!(!availability_path(&availability_data_dir, broken_instance).exists());
        let renewed = healthy
            .snapshot()
            .await
            .expect("read renewed healthy owner");
        let lease = renewed
            .demands()
            .iter()
            .find(|lease| lease.key() == &demand)
            .expect("healthy owner remains present");
        assert!(
            lease
                .expires_at()
                .is_some_and(|expiry| expiry > SystemTime::now())
        );
    }

    #[tokio::test]
    async fn prepared_cold_v1_journal_uses_exact_catalog_base_and_replays() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("prepared-cold-v1-project");
        std::fs::create_dir_all(&project_path).expect("create prepared cold v1 project");
        let paths = locald_core::CatalogPaths::for_data_dir(dir.path());
        let registry = serde_json::json!({
            "projects": {
                project_path.to_string_lossy().as_ref(): {
                    "path": project_path,
                    "name": "prepared-cold-v1",
                    "pinned": false,
                    "last_seen": SystemTime::now()
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize prepared cold v1 registry"),
        )
        .await
        .expect("write prepared cold v1 registry");
        let legacy_attachments = AttachmentStore::new(paths.legacy_attachments.clone());
        legacy_attachments
            .save()
            .await
            .expect("persist prepared cold v1 attachments");
        let catalog = Registry::load_from_paths_for_lifecycle_recovery(paths.clone(), true)
            .await
            .expect("build prepared cold v1 candidate");
        let instance_id = *catalog
            .instances
            .keys()
            .next()
            .expect("prepared cold v1 candidate has one instance");
        let prepared_catalog = catalog.clone();
        let availability_data_dir = dir.path().join("availability-data");
        let manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(paths.legacy_runtime_state.clone())),
            Arc::new(Mutex::new(catalog.clone())),
            Arc::new(Mutex::new(legacy_attachments)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create prepared cold v1 manager");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load prepared cold v1 availability");
        let effective_at = SystemTime::now();
        let prepared = availability
            .prepare_batch(
                &AvailabilityBatch::new(effective_at)
                    .with_operation(AvailabilityBatchOperation::Initialize),
            )
            .await
            .expect("prepare cold v1 availability target");
        let attachments = manager.attachments.lock().await.snapshot();
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LegacyV1Migration,
            effective_at,
            Some(
                CatalogTransactionImages::new(catalog.clone(), catalog)
                    .expect("prepare cold v1 catalog images"),
            ),
            vec![prepared],
            AttachmentTransactionImages::new(attachments.clone(), attachments),
        )
        .expect("prepare cold v1 replay transaction");
        manager
            .lifecycle_journal
            .backup_v1_file(LegacyV1File::Registry, &paths.legacy_registry)
            .await
            .expect("backup prepared cold v1 registry");
        manager
            .lifecycle_journal
            .backup_v1_file(LegacyV1File::Attachments, &paths.legacy_attachments)
            .await
            .expect("backup prepared cold v1 attachments");
        manager
            .lifecycle_journal
            .create(&transaction)
            .await
            .expect("persist Prepared cold v1 journal");
        assert!(!paths.catalog.exists());

        let preflight = manager
            .lifecycle_journal
            .preflight()
            .await
            .expect("preflight Prepared cold v1 journal");
        assert!(!preflight.has_v2_authority());
        let rebound_catalog_path = dir.path().join("rebound/catalog.json");
        let exact_catalog_base = preflight
            .prepared_legacy_catalog_base(&rebound_catalog_path)
            .expect("Prepared cold v1 journal exposes its exact catalog base");
        let mut rebound_prepared_catalog = prepared_catalog.clone();
        rebound_prepared_catalog.set_storage_path(rebound_catalog_path.clone());
        assert_eq!(exact_catalog_base, rebound_prepared_catalog);
        assert_eq!(exact_catalog_base.storage_path(), rebound_catalog_path);
        assert!(exact_catalog_base.instances.contains_key(&instance_id));

        // Re-running legacy discovery would generate new non-Git identities.
        // The startup preflight must instead seed replay from the exact base
        // already recorded in the Prepared journal.
        let rediscovered = Registry::load_from_paths_for_lifecycle_recovery(paths.clone(), true)
            .await
            .expect("rebuild a distinct cold v1 candidate for comparison");
        let rediscovered_instance = *rediscovered
            .instances
            .keys()
            .next()
            .expect("rediscovered cold v1 candidate has one instance");
        assert_ne!(rediscovered_instance, instance_id);
        assert!(!paths.catalog.exists());
        let mut attachments = AttachmentStore::new(paths.legacy_attachments.clone());
        attachments
            .load()
            .await
            .expect("reload legacy compatibility state before replay");
        let mut fresh = ProcessManager::new_with_availability_data_dir(
            dir.path().join("fresh-notify.sock"),
            Arc::new(StateManager::with_path(paths.legacy_runtime_state.clone())),
            Arc::new(Mutex::new(exact_catalog_base)),
            Arc::new(Mutex::new(attachments)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create fresh prepared cold v1 manager");
        fresh.set_host_syncer(Arc::new(NoopHostSyncer));
        fresh
            .recover_and_migrate_lifecycle_state()
            .await
            .expect("replay Prepared cold v1 transaction");

        assert!(rebound_catalog_path.exists());
        assert!(!paths.catalog.exists());
        assert!(availability_path(&availability_data_dir, instance_id).exists());
        assert!(!availability_path(&availability_data_dir, rediscovered_instance).exists());
        assert_eq!(*fresh.registry.lock().await, rebound_prepared_catalog);
        assert!(
            fresh
                .lifecycle_journal
                .migration_marker()
                .await
                .expect("load replayed cold v1 marker")
                .is_some()
        );
        assert!(
            fresh
                .lifecycle_journal
                .load()
                .await
                .expect("inspect cleared cold v1 journal")
                .is_none()
        );
    }

    #[tokio::test]
    async fn cold_v1_migration_backs_up_and_publishes_one_restart_stable_authority() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("cold-v1-project");
        std::fs::create_dir_all(&project_path).expect("create cold v1 project");
        let paths = locald_core::CatalogPaths::for_data_dir(dir.path());
        let registry = serde_json::json!({
            "projects": {
                project_path.to_string_lossy().as_ref(): {
                    "path": project_path,
                    "name": "cold-v1",
                    "pinned": true,
                    "last_seen": SystemTime::now()
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize cold v1 registry"),
        )
        .await
        .expect("write cold v1 registry");
        let cli_source = AttachmentSource::CLI {
            pid: std::process::id(),
        };
        let editor_source = AttachmentSource::Editor {
            name: "vscode".to_owned(),
            id: "cold-v1-window".to_owned(),
            pid: None,
        };
        let migration_started_at = SystemTime::now();
        let mut legacy_attachments = AttachmentStore::new(paths.legacy_attachments.clone());
        legacy_attachments.replace_project(
            &project_path,
            vec![
                Attachment {
                    project_path: project_path.clone(),
                    source: AttachmentSource::Runtime,
                    created_at: SystemTime::UNIX_EPOCH,
                },
                Attachment {
                    project_path: project_path.clone(),
                    source: AttachmentSource::Pin,
                    created_at: SystemTime::now(),
                },
                Attachment {
                    project_path: project_path.clone(),
                    source: cli_source.clone(),
                    created_at: SystemTime::now(),
                },
                Attachment {
                    project_path: project_path.clone(),
                    source: editor_source.clone(),
                    created_at: migration_started_at - std::time::Duration::from_secs(5 * 60),
                },
            ],
            true,
        );
        legacy_attachments
            .save()
            .await
            .expect("persist cold v1 attachments");
        let state_manager = Arc::new(StateManager::with_path(paths.legacy_runtime_state.clone()));
        state_manager
            .save(&ServerState::default())
            .await
            .expect("persist cold v1 runtime state");
        let registry_bytes =
            std::fs::read(&paths.legacy_registry).expect("read cold v1 registry source");
        let attachment_bytes =
            std::fs::read(&paths.legacy_attachments).expect("read cold v1 attachment source");
        let runtime_bytes =
            std::fs::read(&paths.legacy_runtime_state).expect("read cold v1 runtime source");

        let catalog = Registry::load_from_paths_for_lifecycle_recovery(paths.clone(), true)
            .await
            .expect("build cold v1 catalog candidate");
        assert!(!paths.catalog.exists());
        let instance_id = *catalog
            .instances
            .keys()
            .next()
            .expect("cold v1 candidate has one instance");
        let availability_data_dir = dir.path().join("availability-data");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            state_manager,
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(legacy_attachments)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create cold v1 migration manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));

        manager
            .recover_and_migrate_lifecycle_state()
            .await
            .expect("migrate cold v1 authority");

        assert!(paths.catalog.exists());
        assert!(
            manager
                .lifecycle_journal
                .migration_marker()
                .await
                .expect("load cold v1 migration marker")
                .is_some()
        );
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect cleared cold v1 journal")
                .is_none()
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load cold v1 availability");
        let snapshot = availability
            .snapshot()
            .await
            .expect("read cold v1 availability");
        assert!(snapshot.always_on());
        assert!(snapshot.is_paused());
        assert_eq!(snapshot.trusted_launch_path(), None);
        for expected in [
            DemandKey::manual_cli(),
            availability_demand_for_attachment_source(&cli_source)
                .expect("map cold v1 CLI")
                .expect("cold v1 CLI has demand"),
            availability_demand_for_attachment_source(&editor_source)
                .expect("map cold v1 editor")
                .expect("cold v1 editor has demand"),
        ] {
            assert!(
                snapshot
                    .demands()
                    .iter()
                    .any(|lease| lease.key() == &expected),
                "missing migrated demand {expected:?}"
            );
        }
        let migrated_editor = manager
            .attachments
            .lock()
            .await
            .attachments_for(&project_path)
            .into_iter()
            .find(|attachment| attachment.source == editor_source)
            .cloned()
            .expect("retain migrated pidless editor compatibility owner");
        assert!(migrated_editor.created_at >= migration_started_at);
        let backup_dir = availability_data_dir.join("v1-backups");
        assert!(!backup_dir.join("catalog.json").exists());
        assert_eq!(
            std::fs::read(backup_dir.join("registry.json")).expect("read cold v1 registry backup"),
            registry_bytes
        );
        assert_eq!(
            std::fs::read(backup_dir.join("attachments.json"))
                .expect("read cold v1 attachment backup"),
            attachment_bytes
        );
        assert_eq!(
            std::fs::read(backup_dir.join("state.json")).expect("read cold v1 runtime backup"),
            runtime_bytes
        );
        let catalog_after = std::fs::read(&paths.catalog).expect("read migrated catalog");
        let availability_after =
            std::fs::read(availability_path(&availability_data_dir, instance_id))
                .expect("read migrated availability");
        let attachments_after =
            std::fs::read(&paths.legacy_attachments).expect("read migrated compatibility state");

        let catalog = Registry::load_from_paths_for_lifecycle_recovery(paths.clone(), false)
            .await
            .expect("reload exact migrated catalog");
        let mut attachments = AttachmentStore::new(paths.legacy_attachments.clone());
        attachments
            .load_exact()
            .await
            .expect("reload exact migrated compatibility state");
        let mut fresh = ProcessManager::new_with_availability_data_dir(
            dir.path().join("fresh-notify.sock"),
            Arc::new(StateManager::with_path(paths.legacy_runtime_state.clone())),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(attachments)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create fresh migrated manager");
        fresh.set_host_syncer(Arc::new(NoopHostSyncer));
        fresh
            .recover_and_migrate_lifecycle_state()
            .await
            .expect("reopen migrated authority idempotently");

        assert_eq!(
            std::fs::read(&paths.catalog).expect("reread migrated catalog"),
            catalog_after
        );
        assert_eq!(
            std::fs::read(availability_path(&availability_data_dir, instance_id))
                .expect("reread migrated availability"),
            availability_after
        );
        assert_eq!(
            std::fs::read(&paths.legacy_attachments).expect("reread migrated compatibility state"),
            attachments_after
        );
    }

    #[test]
    fn deferred_pidless_editor_keeps_its_original_expiry_until_instance_claim() {
        let project_path = PathBuf::from("/projects/deferred-editor");
        let original = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        let migration_at = original + std::time::Duration::from_secs(5 * 60);
        let claim_at = original + std::time::Duration::from_secs(29 * 60);
        let source = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "legacy-window".to_owned(),
            pid: None,
        };
        let attachment = Attachment {
            project_path: project_path.clone(),
            source,
            created_at: original,
        };
        let mut deferred = AttachmentStoreSnapshot::default();
        deferred.replace_project(&project_path, vec![attachment], false);

        let migration_evidence =
            deferred.compatibility_evidence_at(&project_path, migration_at, |_| false);
        let migration_plan =
            plan_project_lifecycle_migration(false, &migration_evidence, migration_at)
                .expect("plan deferred migration");
        assert_eq!(
            migration_plan.compatibility_attachments[0].created_at,
            original
        );
        deferred.replace_project(
            &project_path,
            migration_plan.compatibility_attachments,
            false,
        );

        let after_original_expiry = original + std::time::Duration::from_secs(30 * 60 + 1);
        assert!(
            !deferred
                .compatibility_evidence_at(&project_path, after_original_expiry, |_| false)
                .attachments[0]
                .alive,
            "unclaimed evidence must not receive a fresh legacy retention window"
        );

        let claim_evidence = deferred.compatibility_evidence_at(&project_path, claim_at, |_| false);
        let claim_plan = plan_project_lifecycle_migration(false, &claim_evidence, claim_at)
            .expect("plan deferred claim");
        let instance_id = "00000000-0000-4000-8000-000000000123"
            .parse()
            .expect("parse project instance ID");
        ProcessManager::claim_compatibility_projection(
            &mut deferred,
            &project_path,
            instance_id,
            claim_plan.compatibility_attachments,
            false,
            claim_at,
        );
        assert_eq!(deferred.instance_owner(&project_path), Some(instance_id));
        assert_eq!(
            deferred.project(&project_path).attachments[0].created_at,
            claim_at
        );
        assert!(
            deferred
                .compatibility_evidence_at(&project_path, after_original_expiry, |_| false)
                .attachments[0]
                .alive,
            "claim starts the v2 editor lease from the claim epoch"
        );
        assert!(
            !deferred
                .compatibility_evidence_at(
                    &project_path,
                    claim_at + locald_core::VSCODE_DEMAND_TTL,
                    |_| false,
                )
                .attachments[0]
                .alive
        );
    }

    #[tokio::test]
    async fn deferred_v1_evidence_materializes_into_initial_availability_authority() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("deferred-v1-project");
        let normalized_project_path = locald_core::normalize_project_locator(&project_path)
            .expect("normalize deferred project locator");
        let paths = locald_core::CatalogPaths::for_data_dir(dir.path());
        let registry = serde_json::json!({
            "projects": {
                project_path.to_string_lossy().as_ref(): {
                    "path": project_path,
                    "name": "deferred-v1",
                    "pinned": false,
                    "last_seen": SystemTime::now()
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize deferred v1 registry"),
        )
        .await
        .expect("write deferred v1 registry");
        let cli_source = AttachmentSource::CLI {
            pid: std::process::id(),
        };
        let mut legacy_attachments = AttachmentStore::new(paths.legacy_attachments.clone());
        legacy_attachments.replace_project(
            &project_path,
            vec![
                Attachment {
                    project_path: project_path.clone(),
                    source: AttachmentSource::Runtime,
                    created_at: SystemTime::UNIX_EPOCH,
                },
                Attachment {
                    project_path: project_path.clone(),
                    source: AttachmentSource::Pin,
                    created_at: SystemTime::now(),
                },
                Attachment {
                    project_path: project_path.clone(),
                    source: cli_source.clone(),
                    created_at: SystemTime::now(),
                },
            ],
            true,
        );
        legacy_attachments
            .save()
            .await
            .expect("persist deferred v1 attachments");

        let catalog = Registry::load_from_paths_for_lifecycle_recovery(paths.clone(), true)
            .await
            .expect("build deferred v1 catalog candidate");
        assert!(catalog.instances.is_empty());
        assert!(
            catalog
                .unresolved_legacy
                .contains_key(&normalized_project_path)
        );
        let availability_data_dir = dir.path().join("availability-data");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(paths.legacy_runtime_state.clone())),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(legacy_attachments)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create deferred v1 migration manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        manager
            .recover_and_migrate_lifecycle_state()
            .await
            .expect("retain unresolved v1 lifecycle evidence");
        assert!(manager.registry.lock().await.instances.is_empty());
        assert!(
            manager
                .registry
                .lock()
                .await
                .unresolved_legacy
                .get(&normalized_project_path)
                .is_some_and(|record| record.pinned)
        );
        assert!(manager.attachments.lock().await.is_stopped(&project_path));
        {
            let mut attachments = manager.attachments.lock().await;
            let base = attachments.snapshot();
            let mut target = base.clone();
            let mut deferred = target.project(&project_path).attachments;
            assert!(
                deferred
                    .iter()
                    .any(|attachment| attachment.source == cli_source)
            );
            assert!(
                deferred
                    .iter()
                    .any(|attachment| matches!(attachment.source, AttachmentSource::Runtime))
            );
            assert!(
                !deferred
                    .iter()
                    .any(|attachment| matches!(attachment.source, AttachmentSource::Pin))
            );
            for attachment in &mut deferred {
                if matches!(attachment.source, AttachmentSource::Runtime) {
                    attachment.created_at = SystemTime::UNIX_EPOCH;
                }
            }
            target.replace_project(&project_path, deferred, true);
            attachments
                .replace_snapshot(target)
                .await
                .expect("age deferred Runtime compatibility evidence");
        }

        std::fs::create_dir_all(&project_path).expect("materialize deferred project");
        write_availability_worker_config(
            &project_path,
            "deferred-v1",
            "deferred-v1.localhost",
            &["web"],
        );
        let catalog_base = manager.registry.lock().await.clone();
        let mut catalog_target = catalog_base.clone();
        let discovery = Registry::discover(project_path.clone())
            .await
            .expect("discover materialized deferred project");
        let probe_instance = catalog_target
            .register_project(discovery, Some("deferred-v1".to_owned()))
            .expect("probe deferred registration target");
        let probe_target = CataloguedLifecycleTarget {
            instance_id: probe_instance,
            path: project_path.clone(),
            catalog_base,
            catalog_target,
        };
        let probe_evidence = ProcessManager::compatibility_evidence_for_target(
            &manager.attachments.lock().await.snapshot(),
            &probe_target,
            SystemTime::now(),
        );
        assert!(probe_evidence.manually_stopped);
        assert_eq!(probe_evidence.attachments.len(), 2);
        assert!(
            probe_evidence
                .attachments
                .iter()
                .any(|item| item.attachment.source == cli_source && item.alive)
        );
        assert!(probe_evidence.attachments.iter().any(|item| {
            matches!(item.attachment.source, AttachmentSource::Runtime) && !item.alive
        }));
        let (instance_id, pending) = manager
            .start_runtime(
                project_path.clone(),
                None,
                false,
                ConfigPhysicalIdentity::NonGit,
            )
            .await
            .expect("materialize deferred project authority");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load materialized deferred availability");
        let materialized = availability
            .snapshot()
            .await
            .expect("read materialized deferred availability");
        let cli_demand = availability_demand_for_attachment_source(&cli_source)
            .expect("map deferred CLI owner")
            .expect("deferred CLI owner has a demand");
        assert!(materialized.always_on());
        assert!(materialized.is_paused(), "{materialized:#?}");
        assert_eq!(materialized.trusted_launch_path(), None);
        assert!(
            materialized
                .demands()
                .iter()
                .any(|lease| lease.key() == &cli_demand)
        );
        assert!(
            !materialized
                .demands()
                .iter()
                .any(|lease| lease.key() == &DemandKey::manual_cli())
        );
        let attachments = manager.attachments.lock().await.snapshot();
        assert_eq!(attachments.instance_owner(&project_path), Some(instance_id));
        assert!(attachments.project(&project_path).manually_stopped);
        assert!(
            attachments
                .project(&project_path)
                .attachments
                .iter()
                .any(|attachment| attachment.source == cli_source)
        );
        assert!(
            !attachments
                .project(&project_path)
                .attachments
                .iter()
                .any(|attachment| matches!(attachment.source, AttachmentSource::Runtime))
        );
        assert!(
            manager
                .registry
                .lock()
                .await
                .instances
                .get(&instance_id)
                .is_some_and(|record| record.pinned)
        );
        drop(pending);

        manager
            .project_force_start(project_path.clone())
            .await
            .expect("resume materialized deferred project");
        let resumed = availability
            .snapshot()
            .await
            .expect("read resumed deferred availability");
        assert!(resumed.always_on());
        assert!(!resumed.is_paused());
        assert!(resumed.desired_up_at(SystemTime::now()));
        assert!(
            resumed
                .demands()
                .iter()
                .any(|lease| lease.key() == &DemandKey::manual_cli())
        );
        assert!(
            resumed
                .demands()
                .iter()
                .any(|lease| lease.key() == &cli_demand)
        );
        let attachments = manager.attachments.lock().await.snapshot();
        assert!(!attachments.project(&project_path).manually_stopped);
        assert_eq!(attachments.instance_owner(&project_path), Some(instance_id));
    }

    #[tokio::test]
    async fn migration_preserves_existing_availability_as_lifecycle_authority() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("existing-availability-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "existing-availability").await;

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load existing availability");
        availability
            .apply_batch(
                &AvailabilityBatch::new(SystemTime::now())
                    .with_operation(AvailabilityBatchOperation::EnsureDemand(
                        DemandKey::manual_cli(),
                    ))
                    .with_operation(AvailabilityBatchOperation::SetTrustedLaunchPath(Some(
                        "/trusted/bin".to_owned(),
                    )))
                    .with_operation(AvailabilityBatchOperation::PauseProject),
            )
            .await
            .expect("seed authoritative availability");
        let authoritative = availability
            .snapshot()
            .await
            .expect("read authoritative availability");

        {
            let mut registry = manager.registry.lock().await;
            registry
                .instances
                .get_mut(&instance_id)
                .expect("catalogued instance")
                .pinned = true;
            registry
                .save()
                .await
                .expect("persist conflicting catalog pin");
        }
        {
            let mut attachments = manager.attachments.lock().await;
            attachments.replace_project(
                &project_path,
                vec![
                    Attachment {
                        project_path: project_path.clone(),
                        source: AttachmentSource::Runtime,
                        created_at: SystemTime::UNIX_EPOCH,
                    },
                    Attachment {
                        project_path: project_path.clone(),
                        source: AttachmentSource::Pin,
                        created_at: SystemTime::now(),
                    },
                    Attachment {
                        project_path: project_path.clone(),
                        source: AttachmentSource::CLI {
                            pid: std::process::id(),
                        },
                        created_at: SystemTime::now(),
                    },
                ],
                true,
            );
        }

        manager
            .recover_and_migrate_lifecycle_state()
            .await
            .expect("migrate around existing authority");

        assert_eq!(
            availability
                .snapshot()
                .await
                .expect("reload preserved availability"),
            authoritative
        );
        assert!(
            !manager
                .registry
                .lock()
                .await
                .instances
                .get(&instance_id)
                .expect("catalogued instance after migration")
                .pinned
        );
        let attachments = manager.attachments.lock().await;
        assert!(attachments.attachments_for(&project_path).is_empty());
        assert!(attachments.is_stopped(&project_path));
        assert_eq!(
            attachments.snapshot().instance_owner(&project_path),
            Some(instance_id)
        );
    }

    #[tokio::test]
    async fn completed_migration_rejects_uncatalogued_attachment_authority_before_reconciliation() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("invalid-attachment-authority");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "invalid-authority").await;
        let uncatalogued: ProjectInstanceId = "00000000-0000-4000-8000-000000000999"
            .parse()
            .expect("parse uncatalogued project instance");
        {
            let mut attachments = manager.attachments.lock().await;
            attachments.replace_project(
                &project_path,
                vec![Attachment {
                    project_path: project_path.clone(),
                    source: AttachmentSource::Pin,
                    created_at: SystemTime::now(),
                }],
                false,
            );
            attachments.set_instance_owner(&project_path, uncatalogued);
        }
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("publish completed migration marker");
        let catalog_before = manager.registry.lock().await.clone();
        let attachments_before = manager.attachments.lock().await.snapshot();

        let error = manager
            .recover_and_migrate_lifecycle_state()
            .await
            .expect_err("uncatalogued attachment authority must block startup recovery");

        assert!(format!("{error:#}").contains("uncatalogued project instance"));
        assert_eq!(*manager.registry.lock().await, catalog_before);
        assert_eq!(
            manager.attachments.lock().await.snapshot(),
            attachments_before
        );
    }

    #[tokio::test]
    async fn completed_migration_rejects_malformed_availability_before_reconciliation() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("malformed-availability-authority");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "malformed-availability").await;
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("publish completed migration marker");
        let availability_file = availability_path(&availability_data_dir, instance_id);
        std::fs::create_dir_all(
            availability_file
                .parent()
                .expect("availability file has a parent directory"),
        )
        .expect("create availability parent directory");
        let malformed = b"{ malformed availability";
        std::fs::write(&availability_file, malformed).expect("write malformed availability");
        let catalog_before = manager.registry.lock().await.clone();
        let attachments_before = manager.attachments.lock().await.snapshot();

        let error = manager
            .recover_and_migrate_lifecycle_state()
            .await
            .expect_err("malformed availability authority must block startup recovery");

        assert!(format!("{error:#}").contains("failed to validate availability authority"));
        assert_eq!(*manager.registry.lock().await, catalog_before);
        assert_eq!(
            manager.attachments.lock().await.snapshot(),
            attachments_before
        );
        assert_eq!(
            std::fs::read(availability_file).expect("read preserved malformed availability"),
            malformed
        );
    }

    #[tokio::test]
    async fn catalog_published_replay_requires_the_catalog_target() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("catalog-phase-replay");
        let (manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "catalog-phase").await;
        let catalog_base = manager.registry.lock().await.clone();
        let mut catalog_target = catalog_base.clone();
        catalog_target
            .instances
            .get_mut(&instance_id)
            .expect("catalogued replay instance")
            .pinned = true;
        let attachments = manager.attachments.lock().await.snapshot();
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            SystemTime::now(),
            Some(
                CatalogTransactionImages::new(catalog_base.clone(), catalog_target)
                    .expect("prepare catalog replay images"),
            ),
            Vec::new(),
            AttachmentTransactionImages::new(attachments.clone(), attachments),
        )
        .expect("prepare catalog replay transaction");
        let replay = journal_transaction_at_phase(
            &manager,
            &transaction,
            LifecycleTransactionPhase::CatalogPublished,
        )
        .await;

        let error = manager
            .apply_lifecycle_transaction_locked(&replay)
            .await
            .expect_err("rolled-back catalog must block replay");

        assert!(format!("{error:#}").contains("catalog target is not authoritative"));
        assert_eq!(*manager.registry.lock().await, catalog_base);
        assert_eq!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("reload blocked catalog journal")
                .expect("blocked catalog journal remains")
                .phase(),
            LifecycleTransactionPhase::CatalogPublished
        );
    }

    #[tokio::test]
    async fn availability_published_replay_requires_every_availability_target() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("availability-phase-replay");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "availability-phase").await;
        let catalog = manager.registry.lock().await.clone();
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load replay availability");
        let prepared = availability
            .prepare_batch(
                &AvailabilityBatch::new(SystemTime::now())
                    .with_operation(AvailabilityBatchOperation::Initialize),
            )
            .await
            .expect("prepare availability replay target");
        let attachments = manager.attachments.lock().await.snapshot();
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            prepared.batch().effective_at(),
            Some(
                CatalogTransactionImages::new(catalog.clone(), catalog)
                    .expect("prepare stable catalog images"),
            ),
            vec![prepared],
            AttachmentTransactionImages::new(attachments.clone(), attachments),
        )
        .expect("prepare availability replay transaction");
        let replay = journal_transaction_at_phase(
            &manager,
            &transaction,
            LifecycleTransactionPhase::AvailabilityPublished,
        )
        .await;

        let error = manager
            .apply_lifecycle_transaction_locked(&replay)
            .await
            .expect_err("rolled-back availability must block replay");

        assert!(format!("{error:#}").contains("availability for"));
        assert!(!availability_path(&availability_data_dir, instance_id).exists());
        assert_eq!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("reload blocked availability journal")
                .expect("blocked availability journal remains")
                .phase(),
            LifecycleTransactionPhase::AvailabilityPublished
        );
    }

    #[tokio::test]
    async fn complete_replay_requires_the_compatibility_target_before_clear() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("compatibility-phase-replay");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "compatibility-phase").await;
        let catalog = manager.registry.lock().await.clone();
        let attachment_base = manager.attachments.lock().await.snapshot();
        let mut attachment_target = attachment_base.clone();
        attachment_target.replace_project(
            &project_path,
            vec![Attachment {
                project_path: project_path.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            }],
            false,
        );
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            SystemTime::now(),
            Some(
                CatalogTransactionImages::new(catalog.clone(), catalog)
                    .expect("prepare stable catalog images"),
            ),
            Vec::new(),
            AttachmentTransactionImages::new(attachment_base.clone(), attachment_target),
        )
        .expect("prepare compatibility replay transaction");
        let replay = journal_transaction_at_phase(
            &manager,
            &transaction,
            LifecycleTransactionPhase::Complete,
        )
        .await;

        let error = manager
            .apply_lifecycle_transaction_locked(&replay)
            .await
            .expect_err("rolled-back compatibility state must block journal clear");

        assert!(format!("{error:#}").contains("compatibility target is not authoritative"));
        assert_eq!(manager.attachments.lock().await.snapshot(), attachment_base);
        assert_eq!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("reload blocked complete journal")
                .expect("blocked complete journal remains")
                .phase(),
            LifecycleTransactionPhase::Complete
        );
    }

    #[tokio::test]
    async fn recreated_worktree_does_not_inherit_path_keyed_lifecycle_evidence() {
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
            r#"[project]
name = "replacement"
domain = "replacement.localhost"

[services.web]
type = "worker"
command = "unused-by-test-factory"
"#,
        )
        .expect("write project config");
        git(&repository, &["add", "README.md", "locald.toml"]);
        git(&repository, &["commit", "-m", "initial"]);

        let worktree = dir.path().join("worktree");
        let worktree_arg = worktree.to_str().expect("UTF-8 worktree path");
        git(
            &repository,
            &["worktree", "add", "-b", "first", worktree_arg],
        );
        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let first_instance = catalog
            .register_project(
                Registry::discover(
                    std::fs::canonicalize(&worktree).expect("canonical first worktree"),
                )
                .await
                .expect("discover first worktree"),
                Some("first".to_owned()),
            )
            .expect("register first worktree");

        git(&repository, &["worktree", "remove", worktree_arg]);
        git(&repository, &["worktree", "prune"]);
        git(
            &repository,
            &["worktree", "add", "-b", "second", worktree_arg],
        );
        let canonical = std::fs::canonicalize(&worktree).expect("canonical replacement worktree");
        let replacement_instance = catalog
            .register_project(
                Registry::discover(canonical.clone())
                    .await
                    .expect("discover replacement worktree"),
                Some("replacement".to_owned()),
            )
            .expect("register replacement worktree");
        assert_ne!(first_instance, replacement_instance);
        catalog.save().await.expect("save replacement catalog");

        let mut attachment_store = AttachmentStore::new(dir.path().join("attachments.json"));
        attachment_store.replace_project(
            &canonical,
            vec![
                Attachment {
                    project_path: canonical.clone(),
                    source: AttachmentSource::Runtime,
                    created_at: SystemTime::now(),
                },
                Attachment {
                    project_path: canonical.clone(),
                    source: AttachmentSource::Pin,
                    created_at: SystemTime::now(),
                },
                Attachment {
                    project_path: canonical.clone(),
                    source: AttachmentSource::CLI {
                        pid: std::process::id(),
                    },
                    created_at: SystemTime::now(),
                },
            ],
            true,
        );
        attachment_store.set_instance_owner(&canonical, first_instance);

        let availability_data_dir = dir.path().join("availability-data");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(attachment_store)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create replacement manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let status = manager
            .project_status(&canonical)
            .await
            .expect("inspect replacement before semantic activity");
        assert_eq!(status.project_name.as_deref(), Some("replacement"));
        assert!(status.attachments.is_empty());
        assert!(!status.is_running);
        let listed = manager
            .project_list(None)
            .await
            .expect("list replacement before semantic activity");
        let replacement_entry = listed
            .iter()
            .find(|entry| entry.project_path == canonical)
            .expect("replacement project is listed");
        assert!(replacement_entry.attachments.is_empty());
        assert_eq!(replacement_entry.section, ProjectSection::Recent);
        assert!(!replacement_entry.is_running);

        manager
            .project_force_start(canonical.clone())
            .await
            .expect("start replacement instance");

        let mut replacement = AvailabilityStore::load(&availability_data_dir, replacement_instance)
            .await
            .expect("load replacement availability");
        let snapshot = replacement
            .snapshot()
            .await
            .expect("read replacement availability");
        assert!(!snapshot.always_on());
        assert!(!snapshot.is_paused());
        assert_eq!(snapshot.demands().len(), 1);
        assert_eq!(
            snapshot.demands()[0].key().kind(),
            locald_core::DemandKind::ManualCli
        );
        let attachments = manager.attachments.lock().await;
        assert!(attachments.attachments_for(&canonical).is_empty());
        assert!(!attachments.is_stopped(&canonical));
        assert_eq!(attachments.snapshot().instance_owner(&canonical), None);
    }

    #[tokio::test]
    async fn initial_config_publication_rejects_a_recreated_worktree() {
        let dir = tempdir().expect("create temporary directory");
        let repository = dir.path().join("publication-race-repository");
        std::fs::create_dir(&repository).expect("create publication race repository");
        git(&repository, &["init", "-b", "main"]);
        git(&repository, &["config", "user.name", "locald tests"]);
        git(
            &repository,
            &["config", "user.email", "locald@example.test"],
        );
        std::fs::write(repository.join("README.md"), "fixture\n")
            .expect("write publication race fixture");
        write_availability_worker_config(
            &repository,
            "publication-race",
            "publication-race.localhost",
            &["web"],
        );
        git(&repository, &["add", "README.md", "locald.toml"]);
        git(&repository, &["commit", "-m", "initial"]);

        let worktree = dir.path().join("publication-race-worktree");
        let worktree_arg = worktree.to_str().expect("UTF-8 worktree path");
        git(
            &repository,
            &["worktree", "add", "-b", "first", worktree_arg],
        );
        let initial_discovery =
            Registry::discover(std::fs::canonicalize(&worktree).expect("canonical first worktree"))
                .await
                .expect("discover first worktree");
        let initial_instance = match initial_discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.identity.project_instance_id,
            ProjectDiscovery::NonGit { .. } => panic!("linked worktree must have Git identity"),
        };

        let catalog = Registry::with_path(dir.path().join("catalog.json"));
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
            dir.path().join("availability-data"),
        )
        .expect("create publication race manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        let domain_index_before = manager.domain_index().snapshot();
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        manager.set_config_publication_hook(ConfigPublicationHook {
            reached: reached.clone(),
            resume: resume.clone(),
        });

        let start = tokio::spawn({
            let manager = manager.clone();
            let worktree = worktree.clone();
            async move { manager.start(worktree, None, false).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), reached.notified())
            .await
            .expect("initial config reaches the publication boundary");

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
        .expect("discover replacement worktree");
        let replacement_instance = match replacement_discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.identity.project_instance_id,
            ProjectDiscovery::NonGit { .. } => panic!("replacement must have Git identity"),
        };
        assert_ne!(replacement_instance, initial_instance);
        resume.notify_one();

        let error = tokio::time::timeout(std::time::Duration::from_secs(2), start)
            .await
            .expect("initial config publication returns promptly")
            .expect("initial config task joins")
            .expect_err("recreated worktree must supersede initial config publication");
        assert!(format!("{error:#}").contains("identity changed while loading configuration"));
        let catalog = manager.registry.lock().await;
        assert!(catalog.instances.is_empty());
        drop(catalog);
        assert!(manager.services.lock().await.is_empty());
        assert_eq!(manager.domain_index().snapshot(), domain_index_before);
    }

    #[tokio::test]
    async fn pending_initial_availability_defers_reload_and_start_authorization() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("pending-initial-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "pending-initial").await;
        write_availability_worker_config(
            &project_path,
            "pending-initial",
            "pending-initial.localhost",
            &["web"],
        );
        manager
            .pending_initial_availability
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(instance_id);

        assert_eq!(
            manager
                .availability_management_state(instance_id)
                .await
                .expect("classify pending initial availability"),
            AvailabilityManagementState::PendingInitial
        );
        let error = manager
            .availability_authorizes_start(instance_id)
            .await
            .expect_err("pending initial availability cannot authorize start");
        assert!(format!("{error:#}").contains("awaiting its initial availability publication"));
        manager
            .reload_catalogued_instance(instance_id, project_path.clone())
            .await
            .expect("pending watcher reload is deferred");
        assert!(manager.services.lock().await.is_empty());

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load pending initial availability");
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("publish initial availability");
        assert_eq!(
            manager
                .availability_management_state(instance_id)
                .await
                .expect("classify published initial availability"),
            AvailabilityManagementState::Managed
        );
        manager
            .availability_authorizes_start(instance_id)
            .await
            .expect("published demand authorizes start even before pending guard drops");
    }

    #[tokio::test]
    async fn initial_config_publication_journals_catalog_with_idle_availability() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("initial-registration-project");
        std::fs::create_dir_all(&project_path).expect("create initial project");
        write_availability_worker_config(
            &project_path,
            "initial-registration",
            "initial-registration.localhost",
            &["web"],
        );
        let availability_data_dir = dir.path().join("availability-data");
        let catalog_path = dir.path().join("catalog.json");
        let attachments_path = dir.path().join("attachments.json");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(Registry::with_path(catalog_path.clone()))),
            Arc::new(Mutex::new(AttachmentStore::new(attachments_path.clone()))),
            None,
            availability_data_dir.clone(),
        )
        .expect("create initial registration manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("seed completed migration authority");

        let (instance_id, pending) = manager
            .start_runtime(
                project_path.clone(),
                None,
                false,
                ConfigPhysicalIdentity::NonGit,
            )
            .await
            .expect("publish initial config without its caller demand");

        assert!(catalog_path.exists());
        assert!(availability_path(&availability_data_dir, instance_id).exists());
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect completed initial journal")
                .is_none()
        );
        drop(pending);

        let catalog = Registry::load_from_paths_for_lifecycle_recovery(
            locald_core::CatalogPaths::for_data_dir(dir.path()),
            false,
        )
        .await
        .expect("reload exact initial catalog");
        let mut attachments = AttachmentStore::new(attachments_path);
        attachments
            .load_exact()
            .await
            .expect("reload exact initial compatibility state");
        let mut fresh = ProcessManager::new_with_availability_data_dir(
            dir.path().join("fresh-notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(catalog)),
            Arc::new(Mutex::new(attachments)),
            None,
            availability_data_dir.clone(),
        )
        .expect("create fresh initial registration manager");
        fresh.set_host_syncer(Arc::new(NoopHostSyncer));
        fresh
            .recover_and_migrate_lifecycle_state()
            .await
            .expect("recover crash boundary between registration and first demand");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load recovered idle availability");
        let snapshot = availability
            .snapshot()
            .await
            .expect("read recovered idle availability");
        assert!(snapshot.demands().is_empty());
        assert!(!snapshot.always_on());
        assert!(!snapshot.is_paused());
        assert_eq!(snapshot.trusted_launch_path(), None);
        assert!(!snapshot.desired_up_at(SystemTime::now()));
    }

    #[tokio::test]
    async fn completed_migration_fails_closed_when_catalogued_availability_is_missing() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("missing-v2-availability-project");
        let (manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "missing-v2-availability").await;
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("publish completed migration marker");
        let catalog_before = manager.registry.lock().await.clone();
        let attachments_before = manager.attachments.lock().await.snapshot();

        let recovery_error = manager
            .recover_and_migrate_lifecycle_state()
            .await
            .expect_err("startup cannot reconstruct missing v2 authority from mirrors");
        assert!(
            format!("{recovery_error:#}")
                .contains("missing availability state after lifecycle-v2 migration")
        );
        assert!(format!("{recovery_error:#}").contains(&instance_id.to_string()));
        assert_eq!(*manager.registry.lock().await, catalog_before);
        assert_eq!(
            manager.attachments.lock().await.snapshot(),
            attachments_before
        );
        assert!(manager.services.lock().await.is_empty());
    }

    #[tokio::test]
    async fn compatibility_mutation_cannot_reconstruct_missing_post_migration_authority() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("missing-compatibility-authority-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "missing-compatibility-authority")
                .await;
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("publish completed migration marker");
        let catalog_before = manager.registry.lock().await.clone();
        let attachments_before = manager.attachments.lock().await.snapshot();

        let error = manager
            .project_force_start(project_path)
            .await
            .expect_err("legacy start cannot synthesize missing v2 authority");
        let message = format!("{error:#}");
        assert!(message.contains("missing availability state after lifecycle-v2 migration"));
        assert!(message.contains(&instance_id.to_string()));
        assert_eq!(*manager.registry.lock().await, catalog_before);
        assert_eq!(
            manager.attachments.lock().await.snapshot(),
            attachments_before
        );
        assert!(!availability_path(&availability_data_dir, instance_id).exists());
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect lifecycle journal after rejected mutation")
                .is_none()
        );
        assert!(manager.services.lock().await.is_empty());
    }

    #[tokio::test]
    async fn registry_list_preserves_saved_entries_when_git_discovery_fails() {
        let dir = tempdir().expect("create temporary directory");
        let repository = dir.path().join("registry-list-repository");
        std::fs::create_dir(&repository).expect("create registry-list repository");
        git(&repository, &["init", "-b", "main"]);
        git(&repository, &["config", "user.name", "locald tests"]);
        git(
            &repository,
            &["config", "user.email", "locald@example.test"],
        );
        std::fs::write(repository.join("README.md"), "fixture\n")
            .expect("write registry-list fixture");
        git(&repository, &["add", "README.md"]);
        git(&repository, &["commit", "-m", "initial"]);

        let worktree = dir.path().join("registry-list-worktree");
        let worktree_arg = worktree
            .to_str()
            .expect("UTF-8 registry-list worktree path");
        git(
            &repository,
            &["worktree", "add", "-b", "registry-list", worktree_arg],
        );
        let worktree = std::fs::canonicalize(worktree).expect("canonical registry-list worktree");
        let healthy = dir.path().join("healthy-project");
        std::fs::create_dir(&healthy).expect("create healthy project");
        let healthy = std::fs::canonicalize(healthy).expect("canonical healthy project");

        let mut catalog = Registry::with_path(dir.path().join("catalog.json"));
        let broken_instance = catalog
            .register_project(
                Registry::discover(worktree.clone())
                    .await
                    .expect("discover registry-list worktree"),
                Some("saved-worktree".to_owned()),
            )
            .expect("register registry-list worktree");
        assert!(catalog.pin_project(&worktree));
        catalog
            .register_project(
                Registry::discover(healthy.clone())
                    .await
                    .expect("discover healthy project"),
                Some("healthy".to_owned()),
            )
            .expect("register healthy project");
        catalog.save().await.expect("persist registry-list catalog");
        let saved_entries = catalog.project_entries();
        let saved_worktree = saved_entries
            .iter()
            .find(|entry| entry.path == worktree)
            .expect("saved worktree entry")
            .clone();

        let registry = Arc::new(Mutex::new(catalog));
        let manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            registry.clone(),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
        )
        .expect("create registry-list manager");
        let catalog_before = registry.lock().await.clone();

        let git_locator = std::fs::read_to_string(worktree.join(".git"))
            .expect("read linked-worktree Git locator");
        let git_admin = PathBuf::from(
            git_locator
                .trim()
                .strip_prefix("gitdir: ")
                .expect("linked-worktree locator prefix"),
        );
        std::fs::rename(&git_admin, git_admin.with_extension("unavailable"))
            .expect("make linked-worktree Git metadata unavailable");
        let discovery_error = Registry::discover(worktree.clone())
            .await
            .expect_err("broken linked worktree discovery fails");
        assert!(format!("{discovery_error:#}").contains("git worktree repair"));

        let listed = manager
            .registry_list()
            .await
            .expect("list retains saved entries when one discovery fails");
        assert!(listed.contains(&saved_worktree));
        let listed_healthy = listed
            .iter()
            .find(|entry| entry.path == healthy)
            .expect("healthy sibling remains visible");
        assert_eq!(listed_healthy.name.as_deref(), Some("healthy"));
        assert!(!listed_healthy.pinned);
        assert_eq!(*registry.lock().await, catalog_before);
        assert!(
            registry
                .lock()
                .await
                .instances
                .contains_key(&broken_instance)
        );

        let status = manager
            .project_status(&worktree)
            .await
            .expect("status retains saved identity when discovery fails");
        assert_eq!(status.project_path, worktree);
        assert_eq!(status.project_name.as_deref(), Some("saved-worktree"));
        assert!(!status.is_running);

        let projects = manager
            .project_list(Some(ProjectFilter::All))
            .await
            .expect("project list retains all saved entries when discovery fails");
        let listed_worktree = projects
            .iter()
            .find(|entry| entry.project_path == worktree)
            .expect("broken saved worktree remains in project list");
        assert_eq!(
            listed_worktree.project_name.as_deref(),
            Some("saved-worktree")
        );
        assert_eq!(listed_worktree.section, ProjectSection::AlwaysOn);
        assert!(
            projects.iter().any(|entry| entry.project_path == healthy),
            "healthy sibling remains in the project list"
        );

        let mutation_error = manager
            .registry_unpin(&worktree)
            .await
            .expect_err("lifecycle mutation remains fail-closed");
        assert!(format!("{mutation_error:#}").contains("git worktree repair"));
    }

    #[tokio::test]
    async fn unregistered_replacement_cannot_observe_or_mutate_historical_instance() {
        let dir = tempdir().expect("create temporary directory");
        let fixture = unregistered_replacement_fixture(dir.path()).await;
        let manager = &fixture.manager;
        let canonical =
            std::fs::canonicalize(&fixture.worktree).expect("canonical replacement path");
        let catalog_path = dir.path().join("catalog.json");
        let attachments_path = dir.path().join("attachments.json");
        let state_path = dir.path().join("state.json");
        let historical_availability_path =
            availability_path(&fixture.availability_data_dir, fixture.historical_instance);
        let catalog_before = manager.registry.lock().await.clone();
        let catalog_bytes_before = std::fs::read(&catalog_path).expect("read catalog baseline");
        let attachments_before = manager.attachments.lock().await.snapshot();
        let attachment_bytes_before =
            std::fs::read(&attachments_path).expect("read attachment baseline");
        let mut historical_availability =
            AvailabilityStore::load(&fixture.availability_data_dir, fixture.historical_instance)
                .await
                .expect("reload historical availability baseline");
        let availability_before = historical_availability
            .snapshot()
            .await
            .expect("snapshot historical availability baseline");
        let availability_bytes_before = std::fs::read(&historical_availability_path)
            .expect("read historical availability baseline");
        let state_bytes_before = std::fs::read(&state_path).expect("read runtime baseline");
        let domains_before = manager.domain_index.snapshot();
        let historical_controller = manager
            .get_service_controller("historical:web")
            .await
            .expect("historical controller exists");
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect lifecycle journal baseline")
                .is_none()
        );

        let status = manager
            .project_status(&canonical)
            .await
            .expect("inspect unregistered replacement");
        assert!(status.project_name.is_none());
        assert!(status.attachments.is_empty());
        assert!(!status.is_running);
        assert!(status.services.is_empty());
        assert!(status.service_details.is_empty());

        let listed = manager
            .project_list(Some(ProjectFilter::All))
            .await
            .expect("list unregistered replacement");
        let replacement_entry = listed
            .iter()
            .find(|entry| entry.project_path == canonical)
            .expect("sanitized replacement entry remains visible");
        assert!(replacement_entry.project_name.is_none());
        assert!(replacement_entry.attachments.is_empty());
        assert!(!replacement_entry.is_running);
        assert_eq!(replacement_entry.section, ProjectSection::Recent);
        for filter in [ProjectFilter::Active, ProjectFilter::Pinned] {
            assert!(
                manager
                    .project_list(Some(filter))
                    .await
                    .expect("filter unregistered replacement")
                    .iter()
                    .all(|entry| entry.project_path != canonical)
            );
        }

        let registered = manager
            .registry_list()
            .await
            .expect("list compatibility registry");
        let replacement_registry_entry = registered
            .iter()
            .find(|entry| entry.path == canonical)
            .expect("sanitized compatibility registry entry remains visible");
        assert!(replacement_registry_entry.name.is_none());
        assert!(!replacement_registry_entry.pinned);

        manager
            .reconcile_legacy_attachment_project(canonical.clone())
            .await
            .expect("background attachment reconciliation is observational");
        manager
            .reload_config(canonical.clone())
            .await
            .expect("background config reload is observational");

        let assert_unregistered = |error: anyhow::Error| {
            let message = format!("{error:#}");
            assert!(message.contains(&fixture.replacement_instance.to_string()));
            assert!(message.contains("locald up"));
        };
        assert_unregistered(
            manager
                .project_detach(canonical.clone(), None)
                .await
                .expect_err("detach must reject an unregistered replacement"),
        );
        assert_unregistered(
            manager
                .project_force_stop(canonical.clone())
                .await
                .expect_err("stop must reject an unregistered replacement"),
        );
        assert_unregistered(
            manager
                .registry_pin(&canonical)
                .await
                .expect_err("pin must reject an unregistered replacement"),
        );
        assert_unregistered(
            manager
                .registry_unpin(&canonical)
                .await
                .expect_err("unpin must reject an unregistered replacement"),
        );
        assert_unregistered(
            manager
                .remove_project(&canonical)
                .await
                .expect_err("remove must reject an unregistered replacement"),
        );

        assert_eq!(*manager.registry.lock().await, catalog_before);
        assert_eq!(
            std::fs::read(&catalog_path).expect("read preserved catalog"),
            catalog_bytes_before
        );
        assert_eq!(
            manager.attachments.lock().await.snapshot(),
            attachments_before
        );
        assert_eq!(
            std::fs::read(&attachments_path).expect("read preserved attachments"),
            attachment_bytes_before
        );
        let mut historical_availability =
            AvailabilityStore::load(&fixture.availability_data_dir, fixture.historical_instance)
                .await
                .expect("reload preserved historical availability");
        assert_eq!(
            historical_availability
                .snapshot()
                .await
                .expect("snapshot preserved historical availability"),
            availability_before
        );
        assert_eq!(
            std::fs::read(&historical_availability_path)
                .expect("read preserved historical availability"),
            availability_bytes_before
        );
        assert_eq!(
            std::fs::read(&state_path).expect("read preserved runtime state"),
            state_bytes_before
        );
        assert_eq!(manager.domain_index.snapshot(), domains_before);
        let current_controller = manager
            .get_service_controller("historical:web")
            .await
            .expect("historical controller remains installed");
        assert!(Arc::ptr_eq(&historical_controller, &current_controller));
        assert!(
            fixture
                .host_calls
                .lock()
                .expect("host sync calls mutex poisoned")
                .is_empty()
        );
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect preserved lifecycle journal")
                .is_none()
        );
        assert!(
            !manager
                .registry
                .lock()
                .await
                .instances
                .contains_key(&fixture.replacement_instance)
        );
    }

    #[tokio::test]
    async fn unmanaged_background_reload_cannot_register_a_physical_replacement() {
        let dir = tempdir().expect("create temporary directory");
        let fixture = unregistered_replacement_fixture(dir.path()).await;
        let historical_availability_path =
            availability_path(&fixture.availability_data_dir, fixture.historical_instance);
        std::fs::remove_file(historical_availability_path)
            .expect("make historical instance use the unmanaged reload path");
        let catalog_before = fixture.manager.registry.lock().await.clone();
        let historical_controller = fixture
            .manager
            .get_service_controller("historical:web")
            .await
            .expect("historical controller exists");

        let error = fixture
            .manager
            .reload_catalogued_instance(fixture.historical_instance, fixture.worktree.clone())
            .await
            .expect_err("background reload must retain the established instance identity");

        assert!(format!("{error:#}").contains("project identity changed"));
        assert_eq!(*fixture.manager.registry.lock().await, catalog_before);
        assert!(
            !fixture
                .manager
                .registry
                .lock()
                .await
                .instances
                .contains_key(&fixture.replacement_instance)
        );
        let current_controller = fixture
            .manager
            .get_service_controller("historical:web")
            .await
            .expect("historical controller remains installed");
        assert!(Arc::ptr_eq(&historical_controller, &current_controller));
    }

    #[tokio::test]
    async fn missing_reused_path_still_retires_the_unique_historical_instance() {
        let dir = tempdir().expect("create temporary directory");
        let fixture = unregistered_replacement_fixture(dir.path()).await;
        let canonical_worktree =
            std::fs::canonicalize(&fixture.worktree).expect("canonical replacement worktree path");
        let worktree_arg = fixture
            .worktree
            .to_str()
            .expect("UTF-8 replacement worktree path");
        git(&fixture.repository, &["worktree", "remove", worktree_arg]);
        git(&fixture.repository, &["worktree", "prune"]);
        let historical_availability_path =
            availability_path(&fixture.availability_data_dir, fixture.historical_instance);
        assert!(historical_availability_path.exists());

        fixture
            .manager
            .remove_project(&canonical_worktree)
            .await
            .expect("remove the unique missing historical instance");

        let registry = fixture.manager.registry.lock().await;
        assert!(
            !registry
                .instances
                .contains_key(&fixture.historical_instance)
        );
        assert!(
            !registry
                .instances
                .contains_key(&fixture.replacement_instance)
        );
        drop(registry);
        assert!(!historical_availability_path.exists());
        let attachments = fixture.manager.attachments.lock().await.snapshot();
        assert!(
            attachments
                .project(&canonical_worktree)
                .attachments
                .is_empty()
        );
        assert_eq!(attachments.instance_owner(&canonical_worktree), None);
        assert!(
            fixture
                .manager
                .get_service_controller("historical:web")
                .await
                .is_none()
        );
        assert!(
            fixture
                .manager
                .state_manager
                .load()
                .await
                .expect("load retired runtime state")
                .services
                .is_empty()
        );
        assert!(
            fixture
                .manager
                .domain_index
                .snapshot()
                .domains_for_instance(fixture.historical_instance)
                .is_empty()
        );
        assert_eq!(
            fixture
                .host_calls
                .lock()
                .expect("host sync calls mutex poisoned")
                .last(),
            Some(&expected_hosts(&[]))
        );
        assert!(
            fixture
                .manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect completed lifecycle journal")
                .is_none()
        );
    }

    #[tokio::test]
    async fn stop_all_pauses_desired_instance_without_runtime_projection() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("stop-all-policy-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "stop-all-policy").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load stop-all availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable policy without runtime");
        assert!(manager.services.lock().await.is_empty());

        manager.stop_all().await.expect("stop all projects");

        let snapshot = availability.snapshot().await.expect("read stop-all result");
        assert!(snapshot.always_on());
        assert!(snapshot.is_paused());
        assert!(manager.attachments.lock().await.is_stopped(&project_path));
    }

    #[tokio::test]
    async fn registry_pin_projects_always_on_without_pin_attachment() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("registry-pin-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "registry-pin").await;
        write_availability_worker_config(
            &project_path,
            "registry-pin",
            "registry-pin.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        manager
            .registry_pin(&project_path)
            .await
            .expect("pin project");

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load pin availability");
        assert!(
            availability
                .snapshot()
                .await
                .expect("read pin policy")
                .always_on()
        );
        let attachments = manager.attachments.lock().await;
        assert!(
            !attachments
                .attachments_for(&project_path)
                .iter()
                .any(|attachment| matches!(&attachment.source, AttachmentSource::Pin))
        );
        drop(attachments);
        let entry = manager
            .project_list(None)
            .await
            .expect("list pinned project")
            .into_iter()
            .find(|entry| entry.project_path == ProcessManager::canonicalize_path(&project_path))
            .expect("pinned project entry");
        assert_eq!(entry.section, ProjectSection::AlwaysOn);
    }

    #[tokio::test]
    async fn legacy_pin_attachment_maps_owner_only_pause_to_always_on_policy() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-pin-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "legacy-pin").await;
        write_availability_worker_config(
            &project_path,
            "legacy-pin",
            "legacy-pin.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("seed completed migration authority");
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load legacy pin availability");
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("seed demand before pause");
        availability
            .pause_project()
            .await
            .expect("seed paused policy");
        {
            let mut attachments = manager.attachments.lock().await;
            attachments.replace_project(&project_path, Vec::new(), true);
            attachments.set_instance_owner(&project_path, instance_id);
        }

        manager
            .project_attach(project_path.clone(), AttachmentSource::Pin)
            .await
            .expect("legacy Pin attach enables Always On");

        let enabled = availability
            .snapshot()
            .await
            .expect("read enabled Always On policy");
        assert!(enabled.always_on());
        assert!(!enabled.is_paused());
        let compatibility = manager.attachments.lock().await.snapshot();
        assert!(!compatibility.project(&project_path).manually_stopped);
        assert_eq!(compatibility.instance_owner(&project_path), None);
        assert!(compatibility.project(&project_path).attachments.is_empty());

        manager
            .project_detach(project_path.clone(), Some(AttachmentSource::Pin))
            .await
            .expect("legacy Pin detach disables Always On");

        assert!(
            !availability
                .snapshot()
                .await
                .expect("read disabled Always On policy")
                .always_on()
        );
        assert!(
            !manager
                .registry
                .lock()
                .await
                .instances
                .get(&instance_id)
                .expect("catalogued legacy Pin project")
                .pinned
        );
    }

    #[tokio::test]
    async fn remove_project_retires_authoritative_availability() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("retire-availability-project");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "retire-availability").await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load removable availability");
        availability
            .set_always_on(true)
            .await
            .expect("persist removable availability");

        manager
            .remove_project(&project_path)
            .await
            .expect("remove project transactionally");

        assert!(
            !tokio::fs::try_exists(availability_path(&availability_data_dir, instance_id,))
                .await
                .expect("inspect retired availability")
        );
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
    async fn fake_clock_final_owner_expiry_preserves_then_stops_running_project() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("timed-shutdown-project");
        let clock = FakeAvailabilityClock::new(TEST_AVAILABILITY_START_SECONDS);
        let (mut manager, instance_id, _availability_data_dir) = availability_manager_with_clock(
            dir.path(),
            &project_path,
            "timed-shutdown",
            SharedAvailabilityClock::new(clock.clone()),
        )
        .await;
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: stop_count.clone(),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "timed-shutdown",
            "timed-shutdown.localhost",
            &["web"],
        );

        manager
            .project_ensure_availability(&project_path, DemandKey::manual_cli())
            .await
            .expect("start project through manual demand");
        assert!(manager.project_runtime_is_ready(instance_id).await);

        clock.advance(locald_core::MANUAL_DEMAND_TTL);
        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("converge exact manual expiry"),
            Some(ConvergenceDecision::PreserveRuntimeUntil {
                deadline: clock.time() + locald_core::SHUTDOWN_COOLDOWN,
            })
        );
        assert!(manager.project_runtime_is_ready(instance_id).await);
        assert_eq!(stop_count.load(Ordering::SeqCst), 0);

        clock.advance(locald_core::SHUTDOWN_COOLDOWN);
        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("converge exact cooldown expiry"),
            Some(ConvergenceDecision::EnsureDown)
        );
        assert!(!manager.project_runtime_exists(instance_id).await);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fake_clock_expiring_one_owner_preserves_runtime_until_the_last_owner_expires() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("timed-owners-project");
        let clock = FakeAvailabilityClock::new(TEST_AVAILABILITY_START_SECONDS);
        let (mut manager, instance_id, _availability_data_dir) = availability_manager_with_clock(
            dir.path(),
            &project_path,
            "timed-owners",
            SharedAvailabilityClock::new(clock.clone()),
        )
        .await;
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: stop_count.clone(),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "timed-owners",
            "timed-owners.localhost",
            &["web"],
        );

        manager
            .project_ensure_availability(
                &project_path,
                DemandKey::vs_code_window("owner-window").expect("construct editor demand"),
            )
            .await
            .expect("start through editor owner");
        manager
            .project_ensure_availability(
                &project_path,
                DemandKey::agent_conversation("owner-conversation")
                    .expect("construct agent demand"),
            )
            .await
            .expect("add agent owner");

        clock.advance(locald_core::VSCODE_DEMAND_TTL);
        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("expire only editor owner"),
            Some(ConvergenceDecision::EnsureUp)
        );
        assert!(manager.project_runtime_is_ready(instance_id).await);
        assert_eq!(stop_count.load(Ordering::SeqCst), 0);

        clock.advance(locald_core::AGENT_DEMAND_TTL - locald_core::VSCODE_DEMAND_TTL);
        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("expire final agent owner"),
            Some(ConvergenceDecision::PreserveRuntimeUntil {
                deadline: clock.time() + locald_core::SHUTDOWN_COOLDOWN,
            })
        );
        assert!(manager.project_runtime_is_ready(instance_id).await);

        clock.advance(locald_core::SHUTDOWN_COOLDOWN);
        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("stop after final-owner cooldown"),
            Some(ConvergenceDecision::EnsureDown)
        );
        assert!(!manager.project_runtime_exists(instance_id).await);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fake_clock_restart_restores_a_still_live_persisted_demand() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("live-demand-restore-project");
        let clock = FakeAvailabilityClock::new(TEST_AVAILABILITY_START_SECONDS);
        let (mut manager, instance_id, availability_data_dir) = availability_manager_with_clock(
            dir.path(),
            &project_path,
            "live-demand-restore",
            SharedAvailabilityClock::new(clock.clone()),
        )
        .await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "live-demand-restore",
            "live-demand-restore.localhost",
            &["web"],
        );
        manager
            .project_ensure_availability(&project_path, DemandKey::manual_cli())
            .await
            .expect("persist live manual demand");
        manager
            .state_manager
            .save(&ServerState::default())
            .await
            .expect("persist empty runtime evidence for restart");
        drop(manager);

        clock.advance(locald_core::MANUAL_DEMAND_TTL - Duration::from_secs(1));
        let mut reopened = reopen_availability_manager_with_clock(
            dir.path(),
            availability_data_dir,
            SharedAvailabilityClock::new(clock),
        )
        .await;
        reopened.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let restore_plan = reopened
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile empty restart evidence");
        reopened.restore_policy_owned_projects(restore_plan).await;

        assert!(reopened.project_runtime_is_ready(instance_id).await);
        reopened
            .project_pause_availability(&project_path)
            .await
            .expect("clean up restored demand runtime");
    }

    #[tokio::test]
    async fn fake_clock_restart_never_restores_expired_demand_or_cooldown() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("expired-demand-restore-project");
        let clock = FakeAvailabilityClock::new(TEST_AVAILABILITY_START_SECONDS);
        let (manager, instance_id, availability_data_dir) = availability_manager_with_clock(
            dir.path(),
            &project_path,
            "expired-demand-restore",
            SharedAvailabilityClock::new(clock.clone()),
        )
        .await;
        write_availability_worker_config(
            &project_path,
            "expired-demand-restore",
            "expired-demand-restore.localhost",
            &["web"],
        );
        manager
            .load_availability(instance_id)
            .await
            .expect("load expiring availability")
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("persist expiring manual demand");
        manager
            .state_manager
            .save(&ServerState {
                services: vec![PersistedServiceState {
                    name: "expired-demand-restore:web".to_owned(),
                    config: test_config_with_domain(
                        "expired-demand-restore",
                        "expired-demand-restore.localhost",
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
            .expect("persist stale running evidence");
        drop(manager);

        clock.advance(locald_core::MANUAL_DEMAND_TTL);
        let reopened = reopen_availability_manager_with_clock(
            dir.path(),
            availability_data_dir,
            SharedAvailabilityClock::new(clock.clone()),
        )
        .await;
        let restore_plan = reopened
            .reconcile_stale_runtime_state()
            .await
            .expect("retire stale runtime evidence");
        reopened.restore_policy_owned_projects(restore_plan).await;

        assert!(!reopened.project_runtime_exists(instance_id).await);
        let snapshot = reopened
            .load_availability(instance_id)
            .await
            .expect("reload expired availability")
            .snapshot()
            .await
            .expect("read expired availability");
        assert!(snapshot.demands().is_empty());
        assert_eq!(
            snapshot.shutdown_cooldown_until(),
            Some(clock.time() + locald_core::SHUTDOWN_COOLDOWN)
        );

        clock.advance(locald_core::SHUTDOWN_COOLDOWN);
        assert_eq!(
            reopened
                .converge_project_availability(&project_path)
                .await
                .expect("converge elapsed restart cooldown"),
            Some(ConvergenceDecision::EnsureDown)
        );
        assert!(!reopened.project_runtime_exists(instance_id).await);
    }

    #[tokio::test]
    async fn fake_clock_restart_restores_always_on_but_preserves_its_pause() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("always-on-pause-restore-project");
        let clock = FakeAvailabilityClock::new(TEST_AVAILABILITY_START_SECONDS);
        let (manager, instance_id, availability_data_dir) = availability_manager_with_clock(
            dir.path(),
            &project_path,
            "always-on-pause-restore",
            SharedAvailabilityClock::new(clock.clone()),
        )
        .await;
        write_availability_worker_config(
            &project_path,
            "always-on-pause-restore",
            "always-on-pause-restore.localhost",
            &["web"],
        );
        manager
            .load_availability(instance_id)
            .await
            .expect("load Always On availability")
            .set_always_on(true)
            .await
            .expect("persist Always On policy");
        manager
            .state_manager
            .save(&ServerState::default())
            .await
            .expect("persist empty Always On runtime evidence");
        drop(manager);

        let mut reopened = reopen_availability_manager_with_clock(
            dir.path(),
            availability_data_dir.clone(),
            SharedAvailabilityClock::new(clock.clone()),
        )
        .await;
        reopened.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let restore_plan = reopened
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile Always On restart evidence");
        reopened.restore_policy_owned_projects(restore_plan).await;
        assert!(reopened.project_runtime_is_ready(instance_id).await);

        reopened
            .project_pause_availability(&project_path)
            .await
            .expect("pause restored Always On project");
        assert!(!reopened.project_runtime_exists(instance_id).await);
        drop(reopened);

        let mut paused_reopen = reopen_availability_manager_with_clock(
            dir.path(),
            availability_data_dir,
            SharedAvailabilityClock::new(clock),
        )
        .await;
        paused_reopen.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let restore_plan = paused_reopen
            .reconcile_stale_runtime_state()
            .await
            .expect("reconcile paused Always On restart evidence");
        paused_reopen
            .restore_policy_owned_projects(restore_plan)
            .await;
        assert!(!paused_reopen.project_runtime_exists(instance_id).await);
        let paused = paused_reopen
            .load_availability(instance_id)
            .await
            .expect("reload paused Always On availability")
            .snapshot()
            .await
            .expect("read paused Always On availability");
        assert!(paused.always_on());
        assert!(paused.is_paused());

        assert!(
            paused_reopen
                .project_set_always_on(&project_path, true)
                .await
                .expect("semantic Always On activity resumes project")
        );
        assert!(paused_reopen.project_runtime_is_ready(instance_id).await);
        paused_reopen
            .project_pause_availability(&project_path)
            .await
            .expect("clean up resumed Always On runtime");
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
    async fn legacy_start_resumes_a_paused_managed_project() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("start-paused-project");
        let (mut manager, instance_id, availability_data_dir) =
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
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("acquire manual demand");
        availability.pause_project().await.expect("pause project");
        let paused_generation = availability
            .snapshot()
            .await
            .expect("load paused availability")
            .activity_generation();

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("legacy Start resumes paused availability");

        let resumed = availability
            .snapshot()
            .await
            .expect("load resumed availability");
        assert_eq!(resumed.activity_generation(), paused_generation + 1);
        assert!(!resumed.is_paused());
        assert!(
            resumed
                .demands()
                .iter()
                .any(|demand| { demand.key().kind() == locald_core::DemandKind::ManualCli })
        );
        assert!(
            manager
                .get_service_controller("start-paused:web")
                .await
                .is_some()
        );
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
                .is_some()
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up resumed project");
    }

    #[tokio::test]
    async fn named_service_stop_survives_automatic_project_convergence() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("service-stop-project");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "service-stop").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "service-stop",
            "service-stop.localhost",
            &["web", "worker"],
        );

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start demanded two-service project");
        let sibling = manager
            .get_service_controller("service-stop:worker")
            .await
            .expect("running sibling service");

        manager
            .stop("service-stop:web")
            .await
            .expect("stop only the selected service");
        assert!(
            manager
                .get_service_controller("service-stop:web")
                .await
                .is_none()
        );

        manager.converge_all_project_availability().await;
        assert!(
            manager
                .get_service_controller("service-stop:web")
                .await
                .is_none(),
            "automatic convergence must respect the one-off service stop"
        );
        assert!(Arc::ptr_eq(
            &sibling,
            &manager
                .get_service_controller("service-stop:worker")
                .await
                .expect("sibling remains running")
        ));
        let mut service_events = manager.event_sender.subscribe();

        let reloaded_config_source = r#"
[project]
name = "service-stop"
domain = "service-stop-reloaded.localhost"

[services.web]
type = "worker"
command = "updated-but-still-stopped"

[services.worker]
type = "worker"
command = "unused-by-test-factory"
"#;
        let reloaded_config: LocaldConfig =
            toml::from_str(reloaded_config_source).expect("parse reloaded stopped-service config");
        std::fs::write(project_path.join("locald.toml"), reloaded_config_source)
            .expect("write reloaded stopped-service config");

        manager
            .reload_catalogued_instance(instance_id, project_path.clone())
            .await
            .expect("passive config reload preserves service stop intent");
        assert!(
            manager
                .get_service_controller("service-stop:web")
                .await
                .is_none(),
            "watcher reload must not reactivate an explicitly stopped service"
        );
        assert!(
            manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "service-stop:web".to_owned()))
        );
        {
            let services = manager.services.lock().await;
            let stopped = services
                .get("service-stop:web")
                .expect("stopped service projection remains available");
            assert!(matches!(&stopped.runtime_state, ServiceRuntime::None));
            assert_eq!(stopped.config, reloaded_config);
            assert_eq!(stopped.service_config, reloaded_config.services["web"]);
        }
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("service-stop-reloaded.localhost")
                .is_some()
        );
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("service-stop.localhost")
                .is_none()
        );
        let event = tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, service_events.recv())
            .await
            .expect("stopped projection update is broadcast")
            .expect("stopped projection event channel remains open");
        let Event::ServiceUpdate(status) = event else {
            panic!("stopped projection publishes a service update");
        };
        assert_eq!(status.name, "service-stop:web");
        assert_eq!(status.status, ServiceState::Stopped);
        assert_eq!(
            status.domain.as_deref(),
            Some("service-stop-reloaded.localhost")
        );
        assert!(Arc::ptr_eq(
            &sibling,
            &manager
                .get_service_controller("service-stop:worker")
                .await
                .expect("sibling remains running after watcher reload")
        ));

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("explicit project start clears service stop suppression");
        assert!(
            manager
                .get_service_controller("service-stop:web")
                .await
                .is_some()
        );
        assert!(
            !manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "service-stop:web".to_owned()))
        );

        let stop_entered = Arc::new(tokio::sync::Notify::new());
        let release_stop = Arc::new(tokio::sync::Notify::new());
        {
            let mut services = manager.services.lock().await;
            let service = services
                .get_mut("service-stop:web")
                .expect("replace running web controller");
            service.runtime_state = ServiceRuntime::Controller(Arc::new(Mutex::new(
                BlockingSuccessfulStopController {
                    id: "service-stop:web".to_owned(),
                    state: RuntimeState {
                        pid: Some(42),
                        port: None,
                        status: ServiceState::Running,
                        health_status: HealthStatus::Healthy,
                    },
                    stop_entered: stop_entered.clone(),
                    release_stop: release_stop.clone(),
                },
            )));
            service.health_status = HealthStatus::Healthy;
        }

        let stop_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.stop("service-stop:web").await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, stop_entered.notified())
            .await
            .expect("service stop enters its controller");
        let start_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.start(project_path, None, false).await }
        });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(
            !start_task.is_finished(),
            "later explicit start waits for the in-flight service stop"
        );

        release_stop.notify_one();
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, stop_task)
            .await
            .expect("service stop completes")
            .expect("service stop task joins")
            .expect("service stop succeeds");
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, start_task)
            .await
            .expect("explicit start completes")
            .expect("explicit start task joins")
            .expect("later explicit start succeeds");
        assert!(
            manager
                .get_service_controller("service-stop:web")
                .await
                .is_some()
        );
        assert!(
            !manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "service-stop:web".to_owned()))
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up service-stop project");
    }

    #[tokio::test]
    async fn removed_service_retires_its_stop_override_after_publication() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("removed-service-stop-project");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "removed-service-stop").await;
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "removed-service-stop",
            "removed-service-stop.localhost",
            &["web", "worker"],
        );

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start two-service project");
        manager
            .stop("removed-service-stop:web")
            .await
            .expect("stop service before removing it");
        manager
            .stop("removed-service-stop:worker")
            .await
            .expect("stop sibling independently");

        write_availability_worker_config(
            &project_path,
            "removed-service-stop",
            "removed-service-stop.localhost",
            &["worker"],
        );
        manager
            .reload_catalogued_instance(instance_id, project_path.clone())
            .await
            .expect("publish stopped service removal");
        {
            let suppressions = manager.service_stop_suppressions.lock().await;
            assert!(!suppressions.contains(&(instance_id, "removed-service-stop:web".to_owned())));
            assert!(
                suppressions.contains(&(instance_id, "removed-service-stop:worker".to_owned()))
            );
        }
        assert!(
            manager
                .services
                .lock()
                .await
                .get("removed-service-stop:web")
                .is_none()
        );

        write_availability_worker_config(
            &project_path,
            "removed-service-stop",
            "removed-service-stop.localhost",
            &["web", "worker"],
        );
        manager
            .reload_catalogued_instance(instance_id, project_path.clone())
            .await
            .expect("re-add service after its retired stop override");
        assert!(
            manager
                .get_service_controller("removed-service-stop:web")
                .await
                .is_some(),
            "a re-added service starts under the project's live demand"
        );
        assert!(
            manager
                .get_service_controller("removed-service-stop:worker")
                .await
                .is_none(),
            "an unrelated service stop remains in force"
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up removed-service-stop project");
    }

    #[tokio::test]
    async fn service_start_route_resumes_only_the_selected_stopped_service() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("selective-service-start-project");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "selective-service-start").await;
        write_availability_worker_config(
            &project_path,
            "selective-service-start",
            "selective-service-start.localhost",
            &["web", "worker"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start demanded two-service project");
        manager
            .stop("selective-service-start:web")
            .await
            .expect("stop selected web service");
        manager
            .stop("selective-service-start:worker")
            .await
            .expect("stop independent worker service");

        let response = crate::api::router(manager.clone())
            .oneshot(
                Request::post("/services/selective-service-start:web/start")
                    .body(Body::empty())
                    .expect("build service-start request"),
            )
            .await
            .expect("service-start response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            manager
                .get_service_controller("selective-service-start:web")
                .await
                .is_some()
        );
        assert!(
            manager
                .get_service_controller("selective-service-start:worker")
                .await
                .is_none(),
            "starting web must preserve worker's independent stop override"
        );
        let suppressions = manager.service_stop_suppressions.lock().await;
        assert!(!suppressions.contains(&(instance_id, "selective-service-start:web".to_owned())));
        assert!(suppressions.contains(&(instance_id, "selective-service-start:worker".to_owned())));
        drop(suppressions);

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up selective service-start project");
    }

    #[tokio::test]
    async fn service_start_route_semantically_resumes_a_paused_project() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("paused-service-start-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "paused-service-start").await;
        write_availability_worker_config(
            &project_path,
            "paused-service-start",
            "paused-service-start.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start demanded project");
        manager
            .project_pause_availability(&project_path)
            .await
            .expect("pause project before service-start action");
        assert!(
            manager
                .get_service_controller("paused-service-start:web")
                .await
                .is_none()
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load paused availability");
        let paused_generation = availability
            .snapshot()
            .await
            .expect("read paused availability")
            .activity_generation();

        let response = crate::api::router(manager.clone())
            .oneshot(
                Request::post("/services/paused-service-start:web/start")
                    .body(Body::empty())
                    .expect("build paused service-start request"),
            )
            .await
            .expect("paused service-start response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            manager
                .get_service_controller("paused-service-start:web")
                .await
                .is_some()
        );

        let resumed = availability
            .snapshot()
            .await
            .expect("read resumed availability");
        assert!(!resumed.is_paused());
        assert_eq!(resumed.activity_generation(), paused_generation + 1);
        assert!(
            resumed.demands().iter().any(|demand| {
                demand.key().kind() == locald_core::DemandKind::StoppedPageResume
            })
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up resumed service-start project");
    }

    #[tokio::test]
    async fn service_start_route_resumes_required_dependencies_only() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("dependency-service-start-project");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "dependency-service-start").await;
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "dependency-service-start"
domain = "dependency-service-start.localhost"

[services.db]
type = "worker"
command = "unused-by-test-factory"

[services.web]
type = "worker"
command = "unused-by-test-factory"
depends_on = ["api"]

[services.api]
type = "worker"
command = "unused-by-test-factory"
depends_on = ["db"]

[services.worker]
type = "worker"
command = "unused-by-test-factory"
"#,
        )
        .expect("write dependency service-start config");
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start dependency project");
        for service in ["web", "api", "db", "worker"] {
            manager
                .stop(&format!("dependency-service-start:{service}"))
                .await
                .expect("stop service before selective dependency start");
        }

        let response = crate::api::router(manager.clone())
            .oneshot(
                Request::post("/services/dependency-service-start:web/start")
                    .body(Body::empty())
                    .expect("build dependency service-start request"),
            )
            .await
            .expect("dependency service-start response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            manager
                .get_service_controller("dependency-service-start:db")
                .await
                .is_some(),
            "a selected service must restore its dependency closure"
        );
        assert!(
            manager
                .get_service_controller("dependency-service-start:api")
                .await
                .is_some(),
            "dependency activation must include transitive prerequisites"
        );
        assert!(
            manager
                .get_service_controller("dependency-service-start:web")
                .await
                .is_some()
        );
        assert!(
            manager
                .get_service_controller("dependency-service-start:worker")
                .await
                .is_none(),
            "an unrelated stopped service remains suppressed"
        );
        let suppressions = manager.service_stop_suppressions.lock().await;
        assert!(!suppressions.contains(&(instance_id, "dependency-service-start:db".to_owned())));
        assert!(!suppressions.contains(&(instance_id, "dependency-service-start:api".to_owned())));
        assert!(!suppressions.contains(&(instance_id, "dependency-service-start:web".to_owned())));
        assert!(
            suppressions.contains(&(instance_id, "dependency-service-start:worker".to_owned()))
        );
        drop(suppressions);

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up dependency service-start project");
    }

    #[tokio::test]
    async fn later_project_pause_supersedes_service_start_at_config_publication() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("pause-service-start-race-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "pause-service-start-race").await;
        write_availability_worker_config(
            &project_path,
            "pause-service-start-race",
            "pause-service-start-race.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start project before pause-vs-service-start race");
        manager
            .stop("pause-service-start-race:web")
            .await
            .expect("stop service before pause-vs-service-start race");

        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        manager.set_config_publication_hook(ConfigPublicationHook {
            reached: reached.clone(),
            resume: resume.clone(),
        });
        let start_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.start_service("pause-service-start-race:web").await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, reached.notified())
            .await
            .expect("service start reaches config publication boundary");

        let pause_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_pause_availability(&project_path).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            loop {
                let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
                    .await
                    .expect("load availability while waiting for pause publication");
                if availability
                    .snapshot()
                    .await
                    .expect("read availability while waiting for pause publication")
                    .is_paused()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("later pause publishes while service start is held");

        resume.notify_one();
        let start_error = tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, start_task)
            .await
            .expect("superseded service start returns")
            .expect("service start task joins")
            .expect_err("later pause supersedes service start");
        assert!(start_error.is::<AvailabilityStartSuperseded>());
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, pause_task)
            .await
            .expect("pause convergence completes")
            .expect("pause task joins")
            .expect("pause succeeds");
        assert!(
            manager
                .get_service_controller("pause-service-start-race:web")
                .await
                .is_none()
        );
        assert!(
            manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "pause-service-start-race:web".to_owned()))
        );
    }

    #[tokio::test]
    async fn new_attachment_resumes_a_service_stop_while_owner_renewal_preserves_it() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("attachment-service-stop-project");
        let (mut manager, instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "attachment-service-stop").await;
        write_availability_worker_config(
            &project_path,
            "attachment-service-stop",
            "attachment-service-stop.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let first_owner = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "window-a".to_owned(),
            pid: None,
        };

        manager
            .project_attach(project_path.clone(), first_owner.clone())
            .await
            .expect("attach first owner");
        manager
            .stop("attachment-service-stop:web")
            .await
            .expect("stop service under first owner");
        manager
            .project_attach(project_path.clone(), first_owner.clone())
            .await
            .expect("renew existing owner");
        assert!(
            manager
                .get_service_controller("attachment-service-stop:web")
                .await
                .is_none(),
            "passive owner renewal preserves the one-off service stop"
        );
        assert!(
            manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "attachment-service-stop:web".to_owned()))
        );

        manager
            .project_detach(project_path.clone(), Some(first_owner))
            .await
            .expect("detach first owner");
        manager
            .project_attach(
                project_path.clone(),
                AttachmentSource::Editor {
                    name: "Code".to_owned(),
                    id: "window-b".to_owned(),
                    pid: None,
                },
            )
            .await
            .expect("new owner resumes automatic service management");
        assert!(
            manager
                .get_service_controller("attachment-service-stop:web")
                .await
                .is_some()
        );
        assert!(
            !manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "attachment-service-stop:web".to_owned()))
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up attachment service-stop project");
    }

    #[tokio::test]
    async fn explicit_availability_actions_resume_a_named_service_stop() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("availability-service-stop-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "availability-service-stop").await;
        write_availability_worker_config(
            &project_path,
            "availability-service-stop",
            "availability-service-stop.localhost",
            &["web"],
        );
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start demanded project");

        manager
            .stop("availability-service-stop:web")
            .await
            .expect("stop service before explicit ensure");
        manager
            .project_ensure_availability(&project_path, DemandKey::manual_cli())
            .await
            .expect("explicit ensure resumes service management");
        assert!(
            manager
                .get_service_controller("availability-service-stop:web")
                .await
                .is_some()
        );

        manager
            .stop("availability-service-stop:web")
            .await
            .expect("stop service before Always On activation");
        manager
            .project_set_always_on(&project_path, true)
            .await
            .expect("Always On activation resumes service management");
        assert!(
            manager
                .get_service_controller("availability-service-stop:web")
                .await
                .is_some()
        );

        manager
            .project_set_always_on(&project_path, false)
            .await
            .expect("disable direct Always On policy");
        manager
            .stop("availability-service-stop:web")
            .await
            .expect("stop service before compatibility pin");
        manager
            .registry_pin(&project_path)
            .await
            .expect("compatibility pin resumes service management");
        assert!(
            manager
                .get_service_controller("availability-service-stop:web")
                .await
                .is_some()
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up explicit availability project");
    }

    #[tokio::test]
    async fn remove_clears_service_stop_before_stable_identity_reregistration() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("remove-service-stop-project");
        std::fs::create_dir(&project_path).expect("create Git project");
        git(&project_path, &["init", "-b", "main"]);
        write_availability_worker_config(
            &project_path,
            "remove-service-stop",
            "remove-service-stop.localhost",
            &["web"],
        );
        let availability_data_dir = dir.path().join("availability-data");
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(Registry::with_path(
                dir.path().join("catalog.json"),
            ))),
            Arc::new(Mutex::new(AttachmentStore::new(
                dir.path().join("attachments.json"),
            ))),
            None,
            availability_data_dir,
        )
        .expect("create remove/re-register manager");
        manager.set_host_syncer(Arc::new(NoopHostSyncer));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        manager
            .start(project_path.clone(), None, false)
            .await
            .expect("start stable-identity project");
        let (instance_id, _) = manager
            .required_availability_instance_for_path(&project_path)
            .await
            .expect("resolve stable project instance");
        manager
            .stop("remove-service-stop:web")
            .await
            .expect("stop named service before removal");
        manager
            .remove_project(&project_path)
            .await
            .expect("remove project");
        assert!(
            !manager
                .service_stop_suppressions
                .lock()
                .await
                .iter()
                .any(|(owner, _)| *owner == instance_id)
        );

        manager
            .project_attach(
                project_path.clone(),
                AttachmentSource::Editor {
                    name: "Code".to_owned(),
                    id: "window-after-remove".to_owned(),
                    pid: None,
                },
            )
            .await
            .expect("re-register removed project through a new owner");
        let (reregistered, _) = manager
            .required_availability_instance_for_path(&project_path)
            .await
            .expect("resolve re-registered stable identity");
        assert_eq!(reregistered, instance_id);
        assert!(
            manager
                .get_service_controller("remove-service-stop:web")
                .await
                .is_some()
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up re-registered project");
    }

    #[tokio::test]
    async fn availability_convergence_first_pause_cancels_in_flight_legacy_start() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("pause-race-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "pause-race").await;
        write_unready_availability_worker_config(
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
                release: None,
                stop_count: stop_count.clone(),
            }),
        );

        let start_manager = manager.clone();
        let start_path = project_path.clone();
        let start = tokio::spawn(async move { start_manager.start(start_path, None, false).await });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, entered.notified())
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
    async fn legacy_force_stop_publishes_pause_during_in_flight_start() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("force-stop-race-project");
        let (mut manager, _instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "force-stop-race").await;
        write_unready_availability_worker_config(
            &project_path,
            "force-stop-race",
            "force-stop-race.localhost",
            &["web"],
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(UnreadyStartFactory {
                entered: entered.clone(),
                release: None,
                stop_count: stop_count.clone(),
            }),
        );

        let start = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.start(project_path, None, false).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, entered.notified())
            .await
            .expect("startup reaches blocking readiness");

        let stop = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_force_stop(project_path).await }
        });
        let instance_id = manager
            .availability_instance_for_path(&project_path)
            .await
            .expect("resolve force-stop project")
            .0;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load force-stop availability");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if availability
                    .snapshot()
                    .await
                    .expect("observe force-stop availability")
                    .is_paused()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("legacy force stop publishes pause promptly");

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let (start_result, stop_result) = tokio::join!(start, stop);
            start_result
                .expect("start task joins")
                .expect("superseded start converges to stop");
            stop_result
                .expect("stop task joins")
                .expect("legacy force stop converges");
        })
        .await
        .expect("force stop cancels startup promptly");

        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(manager.attachments.lock().await.is_stopped(&project_path));
        assert!(
            manager
                .get_service_controller("force-stop-race:web")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn legacy_attach_renews_its_owner_during_prepare() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("attach-renew-prepare-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "attach-renew-prepare").await;
        write_availability_worker_config(
            &project_path,
            "attach-renew-prepare",
            "attach-renew-prepare.localhost",
            &["web"],
        );
        let source = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "window-a".to_owned(),
            pid: None,
        };
        let demand = availability_demand_for_attachment_source(&source)
            .expect("derive editor demand")
            .expect("editor owns an availability demand");
        let original_attachment_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        {
            let mut attachments = manager.attachments.lock().await;
            attachments.replace_project(
                &project_path,
                vec![Attachment {
                    project_path: project_path.clone(),
                    source: source.clone(),
                    created_at: original_attachment_time,
                }],
                false,
            );
            attachments.set_instance_owner(&project_path, instance_id);
            attachments
                .save()
                .await
                .expect("persist original editor owner");
        }
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load attach renewal availability");
        availability
            .ensure_demand(demand.clone())
            .await
            .expect("seed editor demand");
        let original_expiry = availability
            .snapshot()
            .await
            .expect("read original editor demand")
            .demands()
            .iter()
            .find(|lease| lease.key() == &demand)
            .and_then(locald_core::DemandLease::expires_at)
            .expect("editor demand has an expiry");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let prepare_entered = Arc::new(tokio::sync::Notify::new());
        let release_prepare = Arc::new(tokio::sync::Notify::new());
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(BlockingPrepareFactory {
                entered: prepare_entered.clone(),
                release: release_prepare.clone(),
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
            }),
        );
        let convergence = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .converge_managed_instance(instance_id, None, false, true)
                    .await
            }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            prepare_entered.notified(),
        )
        .await
        .expect("editor-owned startup reaches blocking prepare");

        let renewal = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            let source = source.clone();
            async move { manager.project_attach(project_path, source).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            loop {
                let renewed_expiry = availability
                    .snapshot()
                    .await
                    .expect("observe renewed editor demand")
                    .demands()
                    .iter()
                    .find(|lease| lease.key() == &demand)
                    .and_then(locald_core::DemandLease::expires_at);
                let renewed_attachment = manager
                    .attachments
                    .lock()
                    .await
                    .attachments_for(&project_path)
                    .iter()
                    .find(|attachment| attachment.source == source)
                    .map(|attachment| attachment.created_at);
                if renewed_expiry.is_some_and(|expires_at| expires_at > original_expiry)
                    && renewed_attachment
                        .is_some_and(|created_at| created_at > original_attachment_time)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("same-owner attachment renewal publishes while prepare is blocked");
        assert!(!renewal.is_finished());
        release_prepare.notify_one();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            convergence
                .await
                .expect("editor startup convergence task joins")
                .expect("renewed editor owner authorizes startup");
            renewal
                .await
                .expect("editor renewal task joins")
                .expect("editor renewal converges");
        })
        .await
        .expect("editor renewal and startup finish promptly");
        assert_eq!(start_count.load(Ordering::SeqCst), 1);
        assert_eq!(stop_count.load(Ordering::SeqCst), 0);
        assert!(
            manager
                .get_service_controller("attach-renew-prepare:web")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn journaled_stop_and_direct_policy_change_share_one_publication_boundary() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("publication-boundary-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "publication-boundary").await;
        manager.factories.insert(
            0,
            Arc::new(CountingStartFactory {
                creates: Arc::new(AtomicUsize::new(0)),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "publication-boundary",
            "publication-boundary.localhost",
            &["web"],
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load publication-boundary availability");
        availability
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("seed manual demand");

        let publication_guard = manager.lifecycle_publication_lock.lock().await;
        let stop = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_force_stop(project_path).await }
        });
        let policy = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_set_always_on(&project_path, true).await }
        });
        tokio::task::yield_now().await;
        assert!(!stop.is_finished());
        assert!(!policy.is_finished());
        drop(publication_guard);

        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            stop.await
                .expect("stop task joins")
                .expect("journaled stop publishes");
            policy
                .await
                .expect("policy task joins")
                .expect("direct policy publication succeeds");
        })
        .await
        .expect("serialized lifecycle publications finish");

        let snapshot = availability
            .snapshot()
            .await
            .expect("read serialized availability");
        assert!(snapshot.always_on());
        assert_eq!(
            snapshot.desired_up_at(SystemTime::now()),
            !snapshot.is_paused(),
            "the serialized winner determines whether Always On resumes or remains paused"
        );
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect lifecycle journal")
                .is_none()
        );
        assert!(
            !manager
                .lifecycle_recovery_required
                .load(AtomicOrdering::Acquire)
        );
    }

    #[tokio::test]
    async fn direct_always_on_disable_publishes_during_prepare() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("direct-unpin-prepare-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "direct-unpin-prepare").await;
        write_availability_worker_config(
            &project_path,
            "direct-unpin-prepare",
            "direct-unpin-prepare.localhost",
            &["web"],
        );
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load direct unpin availability");
        availability
            .set_always_on(true)
            .await
            .expect("seed direct Always On policy");

        let prepare_entered = Arc::new(tokio::sync::Notify::new());
        let release_prepare = Arc::new(tokio::sync::Notify::new());
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(BlockingPrepareFactory {
                entered: prepare_entered.clone(),
                release: release_prepare.clone(),
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
            }),
        );

        let convergence = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .converge_managed_instance(instance_id, None, false, true)
                    .await
            }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            prepare_entered.notified(),
        )
        .await
        .expect("direct Always On startup reaches blocking prepare");

        let disable = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.project_set_always_on(&project_path, false).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !availability
                    .snapshot()
                    .await
                    .expect("observe direct Always On policy")
                    .always_on()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("direct Always On disable publishes while prepare is blocked");
        assert!(!disable.is_finished());
        release_prepare.notify_one();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            convergence
                .await
                .expect("startup convergence task joins")
                .expect("superseded startup convergence succeeds");
            disable
                .await
                .expect("direct disable task joins")
                .expect("direct Always On disable converges");
        })
        .await
        .expect("direct disable and superseded startup finish promptly");
        assert_eq!(start_count.load(Ordering::SeqCst), 0);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("direct-unpin-prepare:web")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn registry_unpin_publishes_always_on_disable_during_prepare() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("registry-unpin-prepare-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "registry-unpin-prepare").await;
        write_availability_worker_config(
            &project_path,
            "registry-unpin-prepare",
            "registry-unpin-prepare.localhost",
            &["web"],
        );
        {
            let mut registry = manager.registry.lock().await;
            assert!(registry.pin_project(&project_path));
            registry.save().await.expect("persist seeded registry pin");
        }
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load registry unpin availability");
        availability
            .set_always_on(true)
            .await
            .expect("seed registry Always On policy");

        let prepare_entered = Arc::new(tokio::sync::Notify::new());
        let release_prepare = Arc::new(tokio::sync::Notify::new());
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(BlockingPrepareFactory {
                entered: prepare_entered.clone(),
                release: release_prepare.clone(),
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
            }),
        );

        let convergence = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .converge_managed_instance(instance_id, None, false, true)
                    .await
            }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            prepare_entered.notified(),
        )
        .await
        .expect("registry-pinned startup reaches blocking prepare");

        let unpin = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.registry_unpin(&project_path).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !availability
                    .snapshot()
                    .await
                    .expect("observe registry Always On policy")
                    .always_on()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("registry unpin publishes while prepare is blocked");
        assert!(!unpin.is_finished());
        release_prepare.notify_one();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            convergence
                .await
                .expect("registry startup convergence task joins")
                .expect("registry superseded startup convergence succeeds");
            unpin
                .await
                .expect("registry unpin task joins")
                .expect("registry unpin converges");
        })
        .await
        .expect("registry unpin and superseded startup finish promptly");
        assert_eq!(start_count.load(Ordering::SeqCst), 0);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("registry-unpin-prepare:web")
                .await
                .is_none()
        );
        assert!(
            !manager
                .registry
                .lock()
                .await
                .instances
                .get(&instance_id)
                .expect("registry-unpinned project remains catalogued")
                .pinned
        );
    }

    #[tokio::test]
    async fn released_demand_during_prepare_prevents_service_spawn() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("released-prepare-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "released-prepare").await;
        write_availability_worker_config(
            &project_path,
            "released-prepare",
            "released-prepare.localhost",
            &["web"],
        );

        let demand = DemandKey::manual_cli();
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load project availability");
        availability
            .ensure_demand(demand.clone())
            .await
            .expect("acquire demand before startup");

        let prepare_entered = Arc::new(tokio::sync::Notify::new());
        let release_prepare = Arc::new(tokio::sync::Notify::new());
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(BlockingPrepareFactory {
                entered: prepare_entered.clone(),
                release: release_prepare.clone(),
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
            }),
        );

        let start = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.start(project_path, None, false).await }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            prepare_entered.notified(),
        )
        .await
        .expect("startup reaches the prepared pre-spawn boundary");

        availability
            .release_demand(&demand)
            .await
            .expect("release demand while prepare is blocked");
        release_prepare.notify_one();

        tokio::time::timeout(std::time::Duration::from_secs(2), start)
            .await
            .expect("superseded startup returns promptly")
            .expect("startup task joins")
            .expect("startup converges to the released demand");

        assert_eq!(start_count.load(Ordering::SeqCst), 0);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("released-prepare:web")
                .await
                .is_none()
        );
        assert!(
            !availability
                .desired_up()
                .await
                .expect("reload released availability")
        );
        let snapshot = availability
            .snapshot()
            .await
            .expect("reload released cooldown");
        assert!(snapshot.shutdown_cooldown_until().is_some());
    }

    #[tokio::test]
    async fn legacy_detach_publishes_last_owner_release_during_prepare() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("detach-prepare-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "detach-prepare").await;
        write_availability_worker_config(
            &project_path,
            "detach-prepare",
            "detach-prepare.localhost",
            &["web"],
        );
        let source = AttachmentSource::Editor {
            name: "Code".to_owned(),
            id: "window-a".to_owned(),
            pid: None,
        };
        let demand = availability_demand_for_attachment_source(&source)
            .expect("derive editor demand")
            .expect("editor has an availability demand");
        {
            let mut attachments = manager.attachments.lock().await;
            attachments
                .attach(Attachment {
                    project_path: project_path.clone(),
                    source: source.clone(),
                    created_at: SystemTime::now(),
                })
                .expect("seed editor attachment");
            attachments.set_instance_owner(&project_path, instance_id);
            attachments.save().await.expect("persist editor attachment");
        }
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load detach availability");
        availability
            .ensure_demand(demand)
            .await
            .expect("seed editor demand");

        let prepare_entered = Arc::new(tokio::sync::Notify::new());
        let release_prepare = Arc::new(tokio::sync::Notify::new());
        let start_count = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(BlockingPrepareFactory {
                entered: prepare_entered.clone(),
                release: release_prepare.clone(),
                start_count: start_count.clone(),
                stop_count: stop_count.clone(),
            }),
        );

        let convergence = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .converge_managed_instance(instance_id, None, false, true)
                    .await
            }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, prepare_entered.notified())
            .await
            .expect("convergence reaches blocking prepare");

        let detach = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            let source = source.clone();
            async move { manager.project_detach(project_path, Some(source)).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            loop {
                let detached = manager
                    .attachments
                    .lock()
                    .await
                    .attachments_for(&project_path)
                    .is_empty();
                let released = availability
                    .snapshot()
                    .await
                    .expect("observe detached availability")
                    .demands()
                    .is_empty();
                if detached && released {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detach publishes attachment and demand release promptly");
        release_prepare.notify_one();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            convergence
                .await
                .expect("convergence task joins")
                .expect("released owner supersedes startup");
            detach
                .await
                .expect("detach task joins")
                .expect("detach converges released owner");
        })
        .await
        .expect("detach and convergence finish promptly");

        assert_eq!(start_count.load(Ordering::SeqCst), 0);
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            availability
                .snapshot()
                .await
                .expect("load detach cooldown")
                .shutdown_cooldown_until()
                .is_some()
        );
    }

    #[tokio::test]
    async fn started_service_ownership_is_durable_while_readiness_is_blocked() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("durable-start-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "durable-start").await;
        write_unready_availability_worker_config(
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
                release: None,
                stop_count: stop_count.clone(),
            }),
        );

        let start = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.start(project_path, None, false).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, entered.notified())
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
    async fn uncertain_spawn_ownership_publication_stops_child_and_persists_cleanup() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("uncertain-publication-project");
        let (mut manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "uncertain-publication").await;
        write_unready_availability_worker_config(
            &project_path,
            "uncertain-publication",
            "uncertain-publication.localhost",
            &["web"],
        );
        let start_entered = Arc::new(tokio::sync::Notify::new());
        let release_start = Arc::new(tokio::sync::Notify::new());
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(UnreadyStartFactory {
                entered: start_entered.clone(),
                release: Some(release_start.clone()),
                stop_count: stop_count.clone(),
            }),
        );

        let start = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move { manager.start(project_path, None, false).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, start_entered.notified())
            .await
            .expect("service reaches the controlled spawn boundary");
        manager
            .state_manager
            .inject_save_fault(StateSaveFault::ParentDirectorySync)
            .await;
        release_start.notify_one();

        let error = tokio::time::timeout(std::time::Duration::from_secs(1), start)
            .await
            .expect("failed ownership publication returns promptly")
            .expect("start task joins")
            .expect_err("uncertain ownership publication must fail startup");
        let message = format!("{error:#}");
        assert!(message.contains("failed to persist ownership"));
        assert!(message.contains("was published and its parent-directory sync failed"));
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("uncertain-publication:web")
                .await
                .is_none()
        );

        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("load cleaned runtime state");
        let service = persisted
            .services
            .iter()
            .find(|service| service.name == "uncertain-publication:web")
            .expect("cleaned service remains in the runtime projection");
        assert_eq!(service.status, ServiceState::Stopped);
        assert!(service.pid.is_none());
        assert!(service.process_identity.is_none());
    }

    #[tokio::test]
    async fn availability_convergence_pause_is_not_blocked_by_unrelated_startup() {
        let dir = tempdir().expect("create temporary directory");
        let first_path = dir.path().join("pause-now-project");
        let second_path = dir.path().join("slow-start-project");
        let (mut manager, first_id, availability_data_dir) =
            availability_manager(dir.path(), &first_path, "pause-now").await;
        std::fs::create_dir_all(&second_path).expect("create slow-start project");
        write_unready_availability_worker_config(
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
        manager.services.lock().await.insert(
            "pause-now:web".to_owned(),
            availability_test_service(first_id, "pause-now", &first_path, false),
        );

        let start_entered = Arc::new(tokio::sync::Notify::new());
        manager.factories.insert(
            0,
            Arc::new(UnreadyStartFactory {
                entered: start_entered.clone(),
                release: None,
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let slow_start = tokio::spawn({
            let manager = manager.clone();
            let second_path = second_path.clone();
            async move { manager.project_set_always_on(&second_path, true).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, start_entered.notified())
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
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), slow_start)
                .await
                .expect("slow startup observes pause")
                .expect("join slow startup")
                .expect("converge slow startup after pause")
        );
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
            .start_catalogued_instance(instance_id, project_path, None, false, None, None)
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
    async fn availability_convergence_restarts_unhealthy_runtime_before_clearing_error() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("unhealthy-retry-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "unhealthy-retry").await;
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryPrepareFactory {
                failure_consumed: Arc::new(AtomicBool::new(true)),
                stop_count: stop_count.clone(),
            }),
        );
        write_availability_worker_config(
            &project_path,
            "unhealthy-retry",
            "unhealthy-retry.localhost",
            &["web"],
        );
        let (config, _) = ConfigLoader::load_project_config(&project_path)
            .await
            .expect("load unhealthy retry config");
        let service_config = config.services["web"].clone();
        let old_controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(ScriptedController {
                id: "unhealthy-retry:web".to_owned(),
                state: RuntimeState {
                    pid: Some(42),
                    port: None,
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
                process_identity: Some(test_process_identity(
                    1_234,
                    42,
                    "/test/unhealthy-retry-worker",
                )),
                spawn_identity: None,
                prepare_entered: None,
                release_prepare: None,
                start_entered: None,
                start_release: None,
                start_count: None,
                fail_prepare: false,
                stop_count: stop_count.clone(),
            }));
        let mut service = test_service(
            config,
            service_config,
            ServiceRuntime::Controller(old_controller.clone()),
            std::fs::canonicalize(&project_path).expect("canonical unhealthy retry path"),
        );
        service.instance_id = instance_id;
        service.health_status = HealthStatus::Unhealthy;
        manager
            .services
            .lock()
            .await
            .insert("unhealthy-retry:web".to_owned(), service);

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load unhealthy retry availability");
        availability
            .set_always_on(true)
            .await
            .expect("enable unhealthy retry Always On");
        availability
            .record_convergence_error("injected unhealthy runtime".to_owned())
            .await
            .expect("record unhealthy convergence error");

        assert!(!manager.project_runtime_is_ready(instance_id).await);
        assert_eq!(
            manager
                .converge_project_availability(&project_path)
                .await
                .expect("repair unhealthy runtime"),
            Some(ConvergenceDecision::EnsureUp)
        );

        let replacement = manager
            .get_service_controller("unhealthy-retry:web")
            .await
            .expect("replacement controller is running");
        assert!(!Arc::ptr_eq(&old_controller, &replacement));
        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(manager.project_runtime_is_ready(instance_id).await);
        assert_eq!(
            availability
                .snapshot()
                .await
                .expect("reload repaired availability")
                .last_convergence_error(),
            None
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up repaired runtime");
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
    async fn shutdown_drains_an_admitted_ensure_across_post_gate_checks() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("shutdown-admitted-ensure-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "shutdown-admitted-ensure").await;
        write_unready_availability_worker_config(
            &project_path,
            "shutdown-admitted-ensure",
            "shutdown-admitted-ensure.localhost",
            &["web"],
        );
        let start_entered = Arc::new(tokio::sync::Notify::new());
        let stop_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(UnreadyStartFactory {
                entered: start_entered.clone(),
                release: None,
                stop_count: stop_count.clone(),
            }),
        );

        let ensure_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move {
                manager
                    .project_ensure_availability(&project_path, DemandKey::manual_cli())
                    .await
            }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, start_entered.notified())
            .await
            .expect("admitted ensure reaches its readiness wait");

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
        assert!(!shutdown_task.is_finished());

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load admitted ensure availability");
        availability
            .pause_project()
            .await
            .expect("supersede the admitted ensure after shutdown begins");

        let (ensure_result, shutdown_result) =
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(ensure_task, shutdown_task)
            })
            .await
            .expect("admitted ensure drains before shutdown teardown");
        ensure_result
            .expect("join admitted ensure")
            .expect("admitted ensure crosses post-shutdown convergence checks");
        shutdown_result
            .expect("join shutdown")
            .expect("finish shutdown after admitted ensure");

        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("shutdown-admitted-ensure:web")
                .await
                .is_none()
        );
        assert!(
            availability
                .snapshot()
                .await
                .expect("reload admitted ensure availability")
                .is_paused()
        );
    }

    #[tokio::test]
    async fn availability_transition_admission_is_manager_scoped_and_not_inherited() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("admission-scope-project");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "admission-scope").await;
        let other_dir = tempdir().expect("create other temporary directory");
        let other_path = other_dir.path().join("other-admission-scope-project");
        let (other, _other_instance_id, _other_availability_data_dir) =
            availability_manager(other_dir.path(), &other_path, "other-admission-scope").await;

        let shutdown_task = manager
            .run_admitted_availability_transition(|| async {
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
                .expect("shutdown queues behind the admitted transition");
                assert!(!shutdown_task.is_finished());

                other.shutting_down.store(true, AtomicOrdering::Release);
                manager
                    .ensure_accepting_lifecycle_requests()
                    .expect("the admitted manager continues");

                let other_error = other
                    .ensure_accepting_lifecycle_requests()
                    .expect_err("admission does not authorize another manager");
                assert!(other_error.downcast_ref::<DaemonShuttingDown>().is_some());

                let nested_error = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    manager.project_set_always_on(&project_path, true),
                )
                .await
                .expect("nested admission rejects without waiting for the shutdown writer")
                .expect_err("a nested availability transition is rejected");
                assert!(
                    nested_error
                        .downcast_ref::<ReentrantAvailabilityTransition>()
                        .is_some()
                );

                let child_error = tokio::spawn({
                    let manager = manager.clone();
                    async move { manager.ensure_accepting_lifecycle_requests() }
                })
                .await
                .expect("join unadmitted child")
                .expect_err("spawned tasks do not inherit availability admission");
                assert!(child_error.downcast_ref::<DaemonShuttingDown>().is_some());
                Ok(shutdown_task)
            })
            .await
            .expect("exercise scoped availability admission");
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown_task)
            .await
            .expect("shutdown proceeds after the admitted scope exits")
            .expect("join shutdown")
            .expect("finish shutdown");
    }

    #[tokio::test]
    async fn cancelling_an_admitted_transition_releases_shutdown() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("cancelled-admission-project");
        let (manager, _instance_id, _availability_data_dir) =
            availability_manager(dir.path(), &project_path, "cancelled-admission").await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let transition_task = tokio::spawn({
            let manager = manager.clone();
            let entered = entered.clone();
            let release = release.clone();
            async move {
                manager
                    .run_admitted_availability_transition(|| async {
                        entered.notify_one();
                        release.notified().await;
                        Ok(())
                    })
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("transition enters its admitted scope");

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
        assert!(!shutdown_task.is_finished());

        transition_task.abort();
        assert!(
            transition_task
                .await
                .expect_err("join cancelled transition")
                .is_cancelled()
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown_task)
            .await
            .expect("cancelled admission releases the shutdown writer")
            .expect("join shutdown")
            .expect("finish shutdown after cancellation");
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

    #[derive(Clone, Copy)]
    enum QueuedAvailabilityTransition {
        Pause,
        Ensure,
        AlwaysOn,
    }

    async fn assert_shutdown_rejects_queued_availability_transition(
        transition: QueuedAvailabilityTransition,
    ) {
        let dir = tempdir().expect("create temporary directory");
        let label = match transition {
            QueuedAvailabilityTransition::Pause => "pause",
            QueuedAvailabilityTransition::Ensure => "ensure",
            QueuedAvailabilityTransition::AlwaysOn => "always-on",
        };
        let project_path = dir.path().join(format!("shutdown-{label}-project"));
        let project_name = format!("shutdown-{label}");
        let (manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, &project_name).await;
        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load queued-transition availability");
        match transition {
            QueuedAvailabilityTransition::Pause => {
                availability
                    .set_always_on(true)
                    .await
                    .expect("establish running availability baseline");
            }
            QueuedAvailabilityTransition::Ensure | QueuedAvailabilityTransition::AlwaysOn => {
                availability
                    .pause_project()
                    .await
                    .expect("establish paused availability baseline");
            }
        }
        let baseline = availability
            .snapshot()
            .await
            .expect("load queued-transition baseline");

        let transition_guard = manager.availability_transition_gate.write().await;
        let mut transition_task = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move {
                match transition {
                    QueuedAvailabilityTransition::Pause => manager
                        .project_pause_availability(&project_path)
                        .await
                        .map(|_| ()),
                    QueuedAvailabilityTransition::Ensure => manager
                        .project_ensure_availability(&project_path, DemandKey::manual_cli())
                        .await
                        .map(|_| ()),
                    QueuedAvailabilityTransition::AlwaysOn => manager
                        .project_set_always_on(&project_path, true)
                        .await
                        .map(|_| ()),
                }
            }
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut transition_task,)
                .await
                .is_err(),
            "availability transition waits for its admission gate"
        );

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
        drop(transition_guard);

        let (transition_result, shutdown_result) =
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                tokio::join!(transition_task, shutdown_task)
            })
            .await
            .expect("queued transition and shutdown finish without a lock inversion");
        let error = transition_result
            .expect("join queued availability transition")
            .expect_err("queued transition is rejected after shutdown begins");
        assert!(error.downcast_ref::<DaemonShuttingDown>().is_some());
        shutdown_result
            .expect("join shutdown")
            .expect("finish shutdown");

        let snapshot = availability
            .snapshot()
            .await
            .expect("reload availability after shutdown");
        assert_eq!(snapshot, baseline);
    }

    #[tokio::test]
    async fn shutdown_gate_rejects_a_pause_queued_before_availability_publication() {
        assert_shutdown_rejects_queued_availability_transition(QueuedAvailabilityTransition::Pause)
            .await;
    }

    #[tokio::test]
    async fn shutdown_gate_rejects_an_ensure_queued_before_availability_publication() {
        assert_shutdown_rejects_queued_availability_transition(
            QueuedAvailabilityTransition::Ensure,
        )
        .await;
    }

    #[tokio::test]
    async fn shutdown_gate_rejects_always_on_queued_before_availability_publication() {
        assert_shutdown_rejects_queued_availability_transition(
            QueuedAvailabilityTransition::AlwaysOn,
        )
        .await;
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
    async fn stale_runtime_reconciliation_does_not_restore_without_availability_policy() {
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
                .is_none()
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
    async fn service_stop_ignores_runtime_snapshot_that_has_not_materialized() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("legacy-service-stop-project");
        let (mut manager, _instance_id, _availability_data_dir) =
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

        manager
            .stop("legacy-service-stop:web")
            .await
            .expect("a stale runtime snapshot is not a live service");
        assert!(manager.services.lock().await.is_empty());

        manager.restore_policy_owned_projects(restore_plan).await;
        assert!(
            manager
                .get_service_controller("legacy-service-stop:web")
                .await
                .is_none()
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
    async fn unresolved_project_reaper_keeps_owner_evidence_until_stop_succeeds() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("unresolved-reaper-project");
        std::fs::create_dir(&project_path).expect("create unresolved project directory");
        let canonical =
            std::fs::canonicalize(&project_path).expect("canonical unresolved project path");
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(Registry::with_path(
                dir.path().join("catalog.json"),
            ))),
            attachments.clone(),
            None,
        )
        .expect("create unresolved reaper manager");
        attachments
            .lock()
            .await
            .attach(Attachment {
                project_path: project_path.clone(),
                source: AttachmentSource::CLI { pid: u32::MAX },
                created_at: SystemTime::now(),
            })
            .expect("attach expired legacy CLI owner");
        let stop_attempts = Arc::new(AtomicUsize::new(0));
        let first_stop_entered = Arc::new(tokio::sync::Notify::new());
        let release_first_stop = Arc::new(tokio::sync::Notify::new());
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingFailOnceStopController {
                id: "unresolved-reaper:web".to_owned(),
                state: RuntimeState {
                    pid: Some(42),
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
                stop_attempts: stop_attempts.clone(),
                first_stop_entered: first_stop_entered.clone(),
                release_first_stop: release_first_stop.clone(),
            }));
        manager.services.lock().await.insert(
            "unresolved-reaper:web".to_owned(),
            Service {
                instance_id: test_instance_id(),
                controller_generation: 1,
                projection_generation: 1,
                config: LocaldConfig::default(),
                service_config: ServiceConfig::Legacy(ExecServiceConfig::default()),
                resolved_env: HashMap::new(),
                runtime_state: ServiceRuntime::Controller(controller.clone()),
                sticky_port: None,
                path: canonical.clone(),
                health_status: HealthStatus::Healthy,
                health_source: HealthSource::None,
                warnings: Vec::new(),
            },
        );

        let first_reap = tokio::spawn({
            let manager = manager.clone();
            let canonical = canonical.clone();
            async move { manager.reconcile_legacy_attachment_project(canonical).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, first_stop_entered.notified())
            .await
            .expect("first unresolved stop reaches its controller");

        assert!(
            !attachments
                .lock()
                .await
                .attachments_for(&project_path)
                .is_empty(),
            "a daemon exit while stop is in flight leaves retry evidence durable"
        );

        release_first_stop.notify_one();
        let first_error = tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, first_reap)
            .await
            .expect("first unresolved reap completes")
            .expect("first unresolved reap task joins")
            .expect_err("first unresolved stop failure surfaces");
        assert!(format!("{first_error:#}").contains("injected first stop failure"));
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 1);
        assert!(
            !attachments
                .lock()
                .await
                .attachments_for(&project_path)
                .is_empty(),
            "failed stop preserves the owner evidence for the next sweep"
        );

        manager
            .reconcile_legacy_attachment_project(canonical)
            .await
            .expect("later sweep retries and completes the unresolved stop");

        assert!(
            attachments
                .lock()
                .await
                .attachments_for(&project_path)
                .is_empty()
        );
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            controller.lock().await.read_state().await.status,
            ServiceState::Stopped
        );
        assert_eq!(
            manager.services.lock().await["unresolved-reaper:web"].health_status,
            HealthStatus::Unknown
        );
    }

    #[tokio::test]
    async fn unresolved_owner_cleanup_rebases_unrelated_catalog_and_attachment_publication() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("unresolved-publication-race-project");
        std::fs::create_dir(&project_path).expect("create unresolved project directory");
        let canonical =
            std::fs::canonicalize(&project_path).expect("canonical unresolved project path");
        let unrelated_path = dir.path().join("unrelated-registration-project");
        std::fs::create_dir(&unrelated_path).expect("create unrelated project directory");
        git(&unrelated_path, &["init", "-b", "main"]);
        let unrelated_discovery = Registry::discover(unrelated_path.clone())
            .await
            .expect("discover unrelated project");
        let attachments = Arc::new(Mutex::new(AttachmentStore::new(
            dir.path().join("attachments.json"),
        )));
        let manager = ProcessManager::new(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            Arc::new(Mutex::new(Registry::with_path(
                dir.path().join("catalog.json"),
            ))),
            attachments.clone(),
            None,
        )
        .expect("create unresolved publication-race manager");
        attachments
            .lock()
            .await
            .attach(Attachment {
                project_path: project_path.clone(),
                source: AttachmentSource::CLI { pid: u32::MAX },
                created_at: SystemTime::now(),
            })
            .expect("attach expired legacy CLI owner");
        let stop_entered = Arc::new(tokio::sync::Notify::new());
        let release_stop = Arc::new(tokio::sync::Notify::new());
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingSuccessfulStopController {
                id: "unresolved-publication-race:web".to_owned(),
                state: RuntimeState {
                    pid: Some(42),
                    port: Some(3000),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
                stop_entered: stop_entered.clone(),
                release_stop: release_stop.clone(),
            }));
        manager.services.lock().await.insert(
            "unresolved-publication-race:web".to_owned(),
            Service {
                instance_id: test_instance_id(),
                controller_generation: 1,
                projection_generation: 1,
                config: LocaldConfig::default(),
                service_config: ServiceConfig::Legacy(ExecServiceConfig::default()),
                resolved_env: HashMap::new(),
                runtime_state: ServiceRuntime::Controller(controller.clone()),
                sticky_port: None,
                path: canonical.clone(),
                health_status: HealthStatus::Healthy,
                health_source: HealthSource::None,
                warnings: Vec::new(),
            },
        );

        // Model an unrelated first registration that already owns the global
        // runtime transition while this cleanup releases publication and waits
        // to stop its unresolved service.
        let runtime_projection_guard = manager.runtime_projection_lock.lock().await;
        let (_, project_transition) = manager.transition_lock_for_path(&canonical).await;
        let reap = tokio::spawn({
            let manager = manager.clone();
            let canonical = canonical.clone();
            async move { manager.reconcile_legacy_attachment_project(canonical).await }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            loop {
                if project_transition.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("unresolved cleanup reaches the runtime publication boundary");

        let publication_guard = manager.lifecycle_publication_lock.lock().await;
        let unrelated_instance = {
            let mut registry = manager.registry.lock().await;
            let mut candidate = registry.clone();
            let instance_id = candidate
                .register_project(
                    unrelated_discovery,
                    Some("unrelated-registration".to_owned()),
                )
                .expect("register unrelated project candidate");
            registry
                .commit_candidate(candidate)
                .await
                .expect("publish unrelated catalog mutation");
            manager.domain_index.store(registry.domain_index().clone());
            instance_id
        };
        {
            let mut store = attachments.lock().await;
            let mut snapshot = store.snapshot();
            snapshot.replace_project(
                &unrelated_path,
                vec![Attachment {
                    project_path: unrelated_path.clone(),
                    source: AttachmentSource::Editor {
                        name: "Code".to_owned(),
                        id: "unrelated-window".to_owned(),
                        pid: Some(std::process::id()),
                    },
                    created_at: SystemTime::now(),
                }],
                false,
            );
            snapshot.set_instance_owner(&unrelated_path, unrelated_instance);
            store
                .replace_snapshot(snapshot)
                .await
                .expect("publish unrelated compatibility mutation");
        }
        drop(publication_guard);
        drop(runtime_projection_guard);
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, stop_entered.notified())
            .await
            .expect("rebased cleanup reaches the unresolved service stop");
        release_stop.notify_one();

        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, reap)
            .await
            .expect("unresolved cleanup completes after unrelated publication")
            .expect("unresolved cleanup task joins")
            .expect("unresolved cleanup rebases its complete lifecycle transaction");

        let snapshot = attachments.lock().await.snapshot();
        assert!(snapshot.project(&project_path).attachments.is_empty());
        let unrelated = snapshot.project(&unrelated_path);
        assert_eq!(unrelated.instance_owner, Some(unrelated_instance));
        assert_eq!(unrelated.attachments.len(), 1);
        assert_eq!(
            controller.lock().await.read_state().await.status,
            ServiceState::Stopped
        );
        assert!(
            manager
                .registry
                .lock()
                .await
                .instances
                .contains_key(&unrelated_instance)
        );
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect lifecycle journal")
                .is_none()
        );
        assert!(
            !manager
                .lifecycle_recovery_required
                .load(AtomicOrdering::Acquire)
        );
    }

    #[tokio::test]
    async fn first_registration_syncs_hosts_after_catalog_publish_before_later_failure() {
        let dir = tempdir().expect("create temporary directory");
        let project_path = dir.path().join("partial-registration-project");
        std::fs::create_dir(&project_path).expect("create registration project");
        git(&project_path, &["init", "-b", "main"]);
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "partial-registration"
domain = "partial-registration.localhost"

[services.web]
type = "worker"
command = "sleep 30"
"#,
        )
        .expect("write registration config");

        let availability_data_dir = dir.path().join("availability-data");
        let attachments_path = dir.path().join("attachments.json");
        std::fs::create_dir(&attachments_path)
            .expect("block compatibility publication after catalog commit");

        let registry = Arc::new(Mutex::new(Registry::with_path(
            dir.path().join("catalog.json"),
        )));
        let mut manager = ProcessManager::new_with_availability_data_dir(
            dir.path().join("notify.sock"),
            Arc::new(StateManager::with_path(dir.path().join("state.json"))),
            registry.clone(),
            Arc::new(Mutex::new(AttachmentStore::new(attachments_path))),
            None,
            availability_data_dir,
        )
        .expect("create partial registration manager");
        let host_sync_calls = Arc::new(StdMutex::new(Vec::new()));
        manager.set_host_syncer(Arc::new(RecordingHostSyncer {
            calls: host_sync_calls.clone(),
        }));

        manager
            .start(project_path, None, false)
            .await
            .expect_err("blocked compatibility publication surfaces after catalog commit");

        let registry = registry.lock().await;
        assert_eq!(registry.instances.len(), 1);
        assert!(
            registry
                .domain_index()
                .resolve("partial-registration.localhost")
                .is_some()
        );
        assert!(
            manager
                .domain_index()
                .snapshot()
                .resolve("partial-registration.localhost")
                .is_some()
        );
        assert_eq!(
            host_sync_calls
                .lock()
                .expect("recording host sync mutex poisoned")
                .as_slice(),
            &[expected_hosts(&["partial-registration.localhost"])]
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
        manager
            .service_stop_suppressions
            .lock()
            .await
            .insert((instance_id, "reload:a".to_owned()));

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
            manager
                .service_stop_suppressions
                .lock()
                .await
                .contains(&(instance_id, "reload:a".to_owned())),
            "failed publication preserves existing service stop intent"
        );
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
    async fn attachment_persistence_failure_leaves_a_replayable_removal() {
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
        let mut attachment_store = AttachmentStore::new(attachment_path.clone());
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
            .lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("publish completed migration marker");

        manager
            .remove_project(&project_path)
            .await
            .expect_err("attachment persistence failure remains replayable");

        assert!(registry.lock().await.get_project(&project_path).is_none());
        let stored = attachments.lock().await;
        assert_eq!(stored.attachments_for(&project_path).len(), 1);
        assert!(matches!(
            stored.attachments_for(&project_path)[0].source,
            AttachmentSource::Pin
        ));
        drop(stored);
        let transaction = manager
            .lifecycle_journal
            .load()
            .await
            .expect("load replayable removal")
            .expect("removal journal remains");
        assert_eq!(
            transaction.phase(),
            LifecycleTransactionPhase::AvailabilityPublished
        );
        assert!(
            manager
                .lifecycle_recovery_required
                .load(AtomicOrdering::Acquire)
        );

        std::fs::remove_dir(&attachment_path).expect("remove attachment failure fixture");
        manager
            .recover_and_migrate_lifecycle_state()
            .await
            .expect("replay removal after attachment storage is repaired");

        assert!(
            attachments
                .lock()
                .await
                .attachments_for(&project_path)
                .is_empty()
        );
        assert!(
            manager
                .lifecycle_journal
                .load()
                .await
                .expect("inspect cleared removal journal")
                .is_none()
        );
        assert!(
            !manager
                .lifecycle_recovery_required
                .load(AtomicOrdering::Acquire)
        );
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
type = "worker"
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

    fn readiness_test_manager(root: &Path) -> ProcessManager {
        ProcessManager::new(
            root.join("notify.sock"),
            Arc::new(StateManager::with_path(root.join("state.json"))),
            Arc::new(Mutex::new(Registry::default())),
            Arc::new(Mutex::new(AttachmentStore::new(
                root.join("attachments.json"),
            ))),
            None,
        )
        .expect("create readiness test manager")
    }

    #[tokio::test(start_paused = true)]
    async fn portful_controller_liveness_does_not_bypass_the_exact_readiness_deadline() {
        let dir = tempdir().expect("create readiness directory");
        let manager = readiness_test_manager(dir.path());
        let instance_id = test_instance_id();
        let assigned_port = 41_237;
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::new(
                "readiness:web",
                RuntimeState {
                    pid: Some(42),
                    port: Some(assigned_port),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            )));
        let mut service = test_service(
            test_config_with_domain("readiness", "readiness.localhost"),
            ServiceConfig::Legacy(ExecServiceConfig::default()),
            ServiceRuntime::Controller(controller),
            dir.path().to_path_buf(),
        );
        service.sticky_port = Some(assigned_port);
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:web".to_owned(), service);

        let readiness = tokio::spawn({
            let manager = manager.clone();
            async move { manager.wait_for_health("readiness:web", instance_id).await }
        });
        tokio::task::yield_now().await;
        {
            let mut services = manager.services.lock().await;
            let service = services
                .get_mut("readiness:web")
                .expect("readiness service remains present");
            service.health_status = HealthStatus::Unhealthy;
            service.health_source = HealthSource::Tcp;
        }
        tokio::time::advance(
            SERVICE_READINESS_TIMEOUT
                .checked_sub(Duration::from_millis(1))
                .expect("readiness timeout exceeds one millisecond"),
        )
        .await;
        assert!(
            !readiness.is_finished(),
            "a failed probe does not end readiness before the overall deadline"
        );

        tokio::time::advance(Duration::from_millis(1)).await;
        let error = readiness
            .await
            .expect("readiness task joins")
            .expect_err("assigned TCP port never became ready");
        let message = format!("{error:#}");
        assert!(message.contains("timed out after 30s"));
        assert!(message.contains("TCP probe on the assigned endpoint"));
        assert!(message.contains("last runtime was running"));
        assert!(!message.contains("41237"));
        assert_eq!(
            manager.services.lock().await["readiness:web"].health_status,
            HealthStatus::Unhealthy
        );
        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("load persisted readiness failure");
        assert_eq!(persisted.services.len(), 1);
        assert_eq!(persisted.services[0].health_status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn readiness_releases_the_service_registry_while_reading_controller_state() {
        let dir = tempdir().expect("create readiness directory");
        let manager = readiness_test_manager(dir.path());
        let instance_id = test_instance_id();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingStatusController {
                entered: entered.clone(),
                release: release.clone(),
                state: RuntimeState {
                    pid: None,
                    port: Some(41_236),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            }));
        let mut service = test_service(
            LocaldConfig::default(),
            ServiceConfig::Typed(TypedServiceConfig::Site(
                locald_core::config::SiteServiceConfig::default(),
            )),
            ServiceRuntime::Controller(controller),
            dir.path().to_path_buf(),
        );
        service.sticky_port = Some(41_236);
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:site".to_owned(), service);

        let readiness = tokio::spawn({
            let manager = manager.clone();
            async move { manager.wait_for_health("readiness:site", instance_id).await }
        });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("readiness begins reading controller state");
        let mut services = tokio::time::timeout(Duration::from_secs(1), manager.services.lock())
            .await
            .expect("controller state read does not hold the service registry");
        services
            .get_mut("readiness:site")
            .expect("readiness service remains present")
            .health_status = HealthStatus::Healthy;
        drop(services);
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("ready status projection reads the current controller");
        release.notify_one();

        readiness
            .await
            .expect("readiness task joins")
            .expect("current controller becomes ready");
    }

    #[tokio::test]
    async fn portless_worker_uses_owned_process_liveness() {
        let dir = tempdir().expect("create worker readiness directory");
        let manager = readiness_test_manager(dir.path());
        let instance_id = test_instance_id();
        let mut controller = TestController::new(
            "readiness:worker",
            RuntimeState {
                pid: Some(42),
                port: None,
                status: ServiceState::Running,
                health_status: HealthStatus::Unknown,
            },
        );
        controller.owned_process_id = Some(42);
        let controller: Arc<Mutex<dyn ServiceController>> = Arc::new(Mutex::new(controller));
        let mut service = test_service(
            LocaldConfig::default(),
            ServiceConfig::Typed(TypedServiceConfig::Worker(
                locald_core::config::WorkerServiceConfig::default(),
            )),
            ServiceRuntime::Controller(controller),
            dir.path().to_path_buf(),
        );
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:worker".to_owned(), service);

        manager
            .wait_for_health("readiness:worker", instance_id)
            .await
            .expect("live worker becomes ready");
        let services = manager.services.lock().await;
        assert_eq!(
            services["readiness:worker"].health_status,
            HealthStatus::Healthy
        );
        assert_eq!(
            services["readiness:worker"].health_source,
            HealthSource::Explicit
        );
    }

    #[tokio::test(start_paused = true)]
    async fn exact_deadline_rechecks_controller_readiness() {
        let dir = tempdir().expect("create worker readiness directory");
        let manager = readiness_test_manager(dir.path());
        let instance_id = test_instance_id();
        let controller = Arc::new(Mutex::new(TestController::new(
            "readiness:worker",
            RuntimeState {
                pid: Some(42),
                port: None,
                status: ServiceState::Running,
                health_status: HealthStatus::Unknown,
            },
        )));
        let runtime_controller: Arc<Mutex<dyn ServiceController>> = controller.clone();
        let mut service = test_service(
            LocaldConfig::default(),
            ServiceConfig::Typed(TypedServiceConfig::Worker(
                locald_core::config::WorkerServiceConfig::default(),
            )),
            ServiceRuntime::Controller(runtime_controller),
            dir.path().to_path_buf(),
        );
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:worker".to_owned(), service);

        let readiness = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .wait_for_health("readiness:worker", instance_id)
                    .await
            }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(
            SERVICE_READINESS_TIMEOUT
                .checked_sub(Duration::from_millis(250))
                .expect("readiness timeout exceeds the final interval"),
        )
        .await;
        assert!(
            !readiness.is_finished(),
            "unowned process remains pending before the deadline"
        );
        controller.lock().await.owned_process_id = Some(42);

        tokio::time::advance(Duration::from_millis(250)).await;
        readiness
            .await
            .expect("readiness task joins")
            .expect("final controller observation satisfies readiness");
        assert_eq!(
            manager.services.lock().await["readiness:worker"].health_status,
            HealthStatus::Healthy
        );
    }

    #[tokio::test(start_paused = true)]
    async fn portless_worker_requires_owned_process_identity() {
        let dir = tempdir().expect("create worker readiness directory");
        let manager = readiness_test_manager(dir.path());
        let instance_id = test_instance_id();
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::new(
                "readiness:worker",
                RuntimeState {
                    pid: Some(42),
                    port: None,
                    status: ServiceState::Running,
                    health_status: HealthStatus::Unknown,
                },
            )));
        let mut service = test_service(
            LocaldConfig::default(),
            ServiceConfig::Typed(TypedServiceConfig::Worker(
                locald_core::config::WorkerServiceConfig::default(),
            )),
            ServiceRuntime::Controller(controller),
            dir.path().to_path_buf(),
        );
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:worker".to_owned(), service);

        let readiness = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .wait_for_health("readiness:worker", instance_id)
                    .await
            }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(SERVICE_READINESS_TIMEOUT).await;
        let error = readiness
            .await
            .expect("worker readiness task joins")
            .expect_err("unowned PID evidence cannot satisfy worker readiness");
        assert!(format!("{error:#}").contains("owned process liveness"));
        assert_eq!(
            manager.services.lock().await["readiness:worker"].health_status,
            HealthStatus::Unhealthy
        );
        assert_eq!(
            manager.services.lock().await["readiness:worker"].health_source,
            HealthSource::Explicit,
            "terminal worker readiness records its owned-process contract"
        );
        let persisted = manager
            .state_manager
            .load()
            .await
            .expect("load persisted worker readiness failure");
        assert_eq!(persisted.services[0].health_source, HealthSource::Explicit);
    }

    #[tokio::test]
    async fn site_controller_failure_is_an_immediate_readiness_failure() {
        let dir = tempdir().expect("create site readiness directory");
        let manager = readiness_test_manager(dir.path());
        let instance_id = test_instance_id();
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::new(
                "readiness:site",
                RuntimeState {
                    pid: None,
                    port: Some(41_238),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Unhealthy,
                },
            )));
        let mut service = test_service(
            LocaldConfig::default(),
            ServiceConfig::Typed(TypedServiceConfig::Site(
                locald_core::config::SiteServiceConfig::default(),
            )),
            ServiceRuntime::Controller(controller),
            dir.path().to_path_buf(),
        );
        service.sticky_port = Some(41_238);
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:site".to_owned(), service);

        let error = manager
            .wait_for_health("readiness:site", instance_id)
            .await
            .expect_err("failed site build must not become ready");
        assert!(format!("{error:#}").contains("controller reported unhealthy"));
        assert_eq!(
            manager.services.lock().await["readiness:site"].health_status,
            HealthStatus::Unhealthy
        );
        assert_eq!(
            manager.services.lock().await["readiness:site"].health_source,
            HealthSource::Tcp,
            "terminal combined readiness records its endpoint contract"
        );
    }

    #[tokio::test]
    async fn notify_releases_the_service_registry_while_reading_controller_state() {
        let dir = tempdir().expect("create notify readiness directory");
        let manager = readiness_test_manager(dir.path());
        let pid = std::process::id();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(BlockingStatusController {
                entered: entered.clone(),
                release: release.clone(),
                state: RuntimeState {
                    pid: Some(pid),
                    port: Some(41_239),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            }));
        let mut service = test_service(
            LocaldConfig::default(),
            ServiceConfig::Legacy(ExecServiceConfig::default()),
            ServiceRuntime::Controller(controller),
            dir.path().to_path_buf(),
        );
        service.sticky_port = Some(41_239);
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:web".to_owned(), service);

        let notify = tokio::spawn({
            let manager = manager.clone();
            async move { manager.handle_notify(pid).await }
        });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("notify begins reading controller state");
        let services = tokio::time::timeout(Duration::from_secs(1), manager.services.lock())
            .await
            .expect("controller state read does not hold the service registry");
        drop(services);
        release.notify_one();
        notify.await.expect("notify task joins");

        let services = manager.services.lock().await;
        assert_eq!(
            services["readiness:web"].health_status,
            HealthStatus::Starting
        );
        assert_eq!(services["readiness:web"].health_source, HealthSource::None);
    }

    #[tokio::test]
    async fn notify_cannot_bypass_authoritative_readiness() {
        let dir = tempdir().expect("create notify readiness directory");
        let manager = readiness_test_manager(dir.path());
        let pid = std::process::id();
        let controller: Arc<Mutex<dyn ServiceController>> =
            Arc::new(Mutex::new(TestController::new(
                "readiness:web",
                RuntimeState {
                    pid: Some(pid),
                    port: Some(41_239),
                    status: ServiceState::Running,
                    health_status: HealthStatus::Healthy,
                },
            )));
        let explicit = ServiceConfig::Legacy(ExecServiceConfig {
            common: locald_core::config::CommonServiceConfig {
                health_check: Some(locald_core::config::HealthCheckConfig::Command(
                    "true".to_owned(),
                )),
                ..locald_core::config::CommonServiceConfig::default()
            },
            ..ExecServiceConfig::default()
        });
        let mut service = test_service(
            LocaldConfig::default(),
            explicit,
            ServiceRuntime::Controller(controller),
            dir.path().to_path_buf(),
        );
        service.sticky_port = Some(41_239);
        service.health_status = HealthStatus::Starting;
        manager
            .services
            .lock()
            .await
            .insert("readiness:web".to_owned(), service);

        manager.handle_notify(pid).await;
        assert_eq!(
            manager.services.lock().await["readiness:web"].health_status,
            HealthStatus::Starting,
            "notify cannot override an explicit readiness check"
        );

        manager
            .services
            .lock()
            .await
            .get_mut("readiness:web")
            .expect("notify readiness service")
            .service_config = ServiceConfig::Legacy(ExecServiceConfig::default());
        manager.handle_notify(pid).await;
        let services = manager.services.lock().await;
        assert_eq!(
            services["readiness:web"].health_status,
            HealthStatus::Starting
        );
        assert_eq!(services["readiness:web"].health_source, HealthSource::None);
        drop(services);

        manager
            .services
            .lock()
            .await
            .get_mut("readiness:web")
            .expect("notify readiness service remains present")
            .health_status = HealthStatus::Unhealthy;
        manager.handle_notify(pid).await;
        assert_eq!(
            manager.services.lock().await["readiness:web"].health_status,
            HealthStatus::Unhealthy,
            "notify cannot revive terminal readiness"
        );
    }

    #[tokio::test]
    async fn ensure_project_registers_then_returns_ready_semantic_urls() {
        let dir = tempdir().expect("create EnsureProject directory");
        let project_path = dir.path().join("ensure-project");
        std::fs::create_dir(&project_path).expect("create EnsureProject project");
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "ensure-project"
domain = "ensure-project.localhost"

[services.web]
command = "unused-by-test-factory"
"#,
        )
        .expect("write EnsureProject config");
        let mut manager = unregistered_availability_manager(dir.path());
        let creates = Arc::new(AtomicUsize::new(1));
        manager.factories.insert(
            0,
            Arc::new(RetryingTcpReadinessFactory {
                creates: creates.clone(),
                stops: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let malformed_demand: DemandKey = serde_json::from_value(serde_json::json!({
            "kind": "agent_conversation",
            "owner": "not-a-canonical-digest"
        }))
        .expect("deserialize malformed demand fixture");
        let malformed_error = manager
            .ensure_project(project_path.clone(), malformed_demand)
            .await
            .expect_err("malformed demand is rejected before registration");
        assert!(format!("{malformed_error:#}").contains("invalid EnsureProject demand"));
        assert!(manager.registry.lock().await.instances.is_empty());

        let legacy_error = manager
            .ensure_project(
                project_path.clone(),
                DemandKey::legacy_process_attachment("untrusted-pid")
                    .expect("construct compatibility demand"),
            )
            .await
            .expect_err("semantic ensure rejects compatibility ownership");
        assert!(
            format!("{legacy_error:#}")
                .contains("does not accept legacy process-attachment demands")
        );
        assert!(manager.registry.lock().await.instances.is_empty());

        let mut ensure = tokio::spawn({
            let manager = manager.clone();
            let config_path = project_path.join("locald.toml");
            async move {
                manager
                    .ensure_project(config_path, DemandKey::manual_cli())
                    .await
            }
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut ensure)
                .await
                .is_err(),
            "EnsureProject waits for proxy listeners"
        );
        assert!(manager.registry.lock().await.instances.is_empty());
        manager.set_http_port(Some(80)).await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut ensure)
                .await
                .is_err(),
            "EnsureProject waits for both proxy listeners"
        );
        assert!(manager.registry.lock().await.instances.is_empty());
        manager.set_https_port(Some(8443)).await;

        let result = ensure
            .await
            .expect("join first project registration")
            .expect("ensure first project registration");
        assert_eq!(result.state, EnsureProjectState::Ready);
        assert_eq!(
            result.project_path,
            std::fs::canonicalize(&project_path).expect("canonical EnsureProject path")
        );
        assert_eq!(result.project_name.as_deref(), Some("ensure-project"));
        assert_eq!(result.urls, vec!["https://ensure-project.localhost"]);
        assert_eq!(result.services.len(), 1);
        assert_eq!(result.services[0].name, "ensure-project:web");
        assert_eq!(result.services[0].status, ServiceState::Running);
        assert_eq!(result.services[0].health_status, HealthStatus::Healthy);
        assert_eq!(
            result.services[0].url.as_deref(),
            Some("https://ensure-project.localhost")
        );
        assert_eq!(creates.load(Ordering::SeqCst), 2);

        let (instance_id, _) = manager
            .required_availability_instance_for_path(&project_path)
            .await
            .expect("resolve newly registered EnsureProject instance");
        let snapshot = manager
            .load_availability(instance_id)
            .await
            .expect("load EnsureProject availability")
            .snapshot()
            .await
            .expect("read EnsureProject availability");
        assert!(
            snapshot
                .demands()
                .iter()
                .any(|lease| lease.kind() == DemandKind::ManualCli)
        );
    }

    #[tokio::test]
    async fn concurrent_ensure_project_calls_share_one_runtime_transition() {
        let dir = tempdir().expect("create concurrent EnsureProject directory");
        let project_path = dir.path().join("concurrent-ensure-project");
        std::fs::create_dir(&project_path).expect("create concurrent EnsureProject project");
        write_availability_worker_config(
            &project_path,
            "concurrent-ensure",
            "concurrent-ensure.localhost",
            &["web"],
        );
        let mut manager = unregistered_availability_manager(dir.path());
        manager.set_http_port(Some(80)).await;
        manager.set_https_port(Some(443)).await;
        let publication_reached = Arc::new(tokio::sync::Notify::new());
        let resume_publication = Arc::new(tokio::sync::Notify::new());
        manager.set_config_publication_hook(ConfigPublicationHook {
            reached: publication_reached.clone(),
            resume: resume_publication.clone(),
        });
        let prepare_entered = Arc::new(tokio::sync::Notify::new());
        let release_prepare = Arc::new(tokio::sync::Notify::new());
        let start_count = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(BlockingPrepareFactory {
                entered: prepare_entered.clone(),
                release: release_prepare.clone(),
                start_count: start_count.clone(),
                stop_count: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let first = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move {
                manager
                    .ensure_project(project_path, DemandKey::manual_cli())
                    .await
            }
        });
        tokio::time::timeout(
            TEST_STARTUP_BOUNDARY_TIMEOUT,
            publication_reached.notified(),
        )
        .await
        .expect("first ensure reaches initial catalog publication");
        let second = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move {
                manager
                    .ensure_project(project_path, DemandKey::manual_cli())
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!second.is_finished());
        resume_publication.notify_one();
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, prepare_entered.notified())
            .await
            .expect("first ensure reaches service preparation");
        assert!(!second.is_finished());
        release_prepare.notify_one();

        let (first, second) = tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            tokio::join!(first, second)
        })
        .await
        .expect("concurrent ensures finish after readiness");
        let first = first
            .expect("join first ensure")
            .expect("first ensure succeeds");
        let second = second
            .expect("join second ensure")
            .expect("second ensure succeeds");
        assert_eq!(first.state, EnsureProjectState::Ready);
        assert_eq!(second.state, EnsureProjectState::Ready);
        assert_eq!(start_count.load(Ordering::SeqCst), 1);
        assert_eq!(manager.registry.lock().await.instances.len(), 1);
    }

    #[tokio::test]
    async fn ensure_project_surfaces_the_exact_readiness_timeout() {
        let dir = tempdir().expect("create timed EnsureProject directory");
        let project_path = dir.path().join("timed-ensure-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "timed-ensure").await;
        manager.set_http_port(Some(80)).await;
        manager.set_https_port(Some(443)).await;
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "timed-ensure"
domain = "timed-ensure.localhost"

[services.web]
command = "unused-by-test-factory"
"#,
        )
        .expect("write timed EnsureProject config");
        let creates = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryingTcpReadinessFactory {
                creates: creates.clone(),
                stops: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let ensure = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move {
                manager
                    .ensure_project(project_path, DemandKey::manual_cli())
                    .await
            }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            loop {
                if creates.load(Ordering::SeqCst) == 1
                    && manager
                        .services
                        .lock()
                        .await
                        .get("timed-ensure:web")
                        .is_some_and(|service| service.health_status == HealthStatus::Starting)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("EnsureProject reaches readiness polling");
        tokio::time::pause();
        tokio::time::advance(SERVICE_READINESS_TIMEOUT).await;
        let error = ensure
            .await
            .expect("join timed EnsureProject")
            .expect_err("unbound assigned endpoint times out");
        assert!(format!("{error:#}").contains("timed out after 30s"));

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load timed EnsureProject availability");
        assert!(
            availability
                .desired_up()
                .await
                .expect("timed EnsureProject remains desired up")
        );
        assert!(
            availability
                .snapshot()
                .await
                .expect("read timed EnsureProject failure")
                .last_convergence_error()
                .is_some_and(|message| message.contains("timed out after 30s"))
        );
    }

    #[tokio::test]
    async fn readiness_timeout_preserves_demand_and_retry_clears_the_convergence_error() {
        let dir = tempdir().expect("create retry readiness directory");
        let project_path = dir.path().join("retry-readiness-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "retry-readiness").await;
        tokio::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "retry-readiness"
domain = "retry-readiness.localhost"

[services.db]
command = "ignored by readiness fixture"

[services.web]
command = "ignored by readiness fixture"
depends_on = ["db"]
"#,
        )
        .await
        .expect("write retry readiness config");
        let creates = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryingTcpReadinessFactory {
                creates: creates.clone(),
                stops: stops.clone(),
            }),
        );

        let first_ensure = tokio::spawn({
            let manager = manager.clone();
            let project_path = project_path.clone();
            async move {
                manager
                    .project_ensure_availability(&project_path, DemandKey::manual_cli())
                    .await
            }
        });
        tokio::time::timeout(TEST_STARTUP_BOUNDARY_TIMEOUT, async {
            loop {
                if creates.load(Ordering::SeqCst) == 1
                    && manager
                        .services
                        .lock()
                        .await
                        .get("retry-readiness:db")
                        .is_some_and(|service| service.health_status == HealthStatus::Starting)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first ensure reaches its readiness wait");
        assert!(
            !first_ensure.is_finished(),
            "first ensure remains blocked on readiness after creating its controller"
        );
        assert_eq!(creates.load(Ordering::SeqCst), 1);
        assert!(
            manager
                .get_service_controller("retry-readiness:web")
                .await
                .is_none(),
            "dependent service cannot start before its dependency is ready"
        );
        tokio::task::yield_now().await;

        tokio::time::pause();
        tokio::time::advance(SERVICE_READINESS_TIMEOUT).await;
        let error = first_ensure
            .await
            .expect("first ensure task joins")
            .expect_err("first controller never binds its assigned port");
        assert!(format!("{error:#}").contains("timed out after 30s"));

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load readiness availability after timeout");
        let failed = availability
            .snapshot()
            .await
            .expect("read failed readiness state");
        assert!(
            availability
                .desired_up()
                .await
                .expect("failed readiness remains desired up")
        );
        assert!(!failed.demands().is_empty());
        assert!(
            failed
                .last_convergence_error()
                .is_some_and(|message| message.contains("timed out after 30s"))
        );
        assert_eq!(
            manager.services.lock().await["retry-readiness:db"].health_status,
            HealthStatus::Unhealthy
        );

        manager
            .project_ensure_availability(&project_path, DemandKey::manual_cli())
            .await
            .expect("retry binds the assigned port and becomes ready");
        assert_eq!(creates.load(Ordering::SeqCst), 3);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(manager.project_runtime_is_ready(instance_id).await);
        assert_eq!(
            availability
                .snapshot()
                .await
                .expect("read successful readiness retry")
                .last_convergence_error(),
            None
        );

        manager
            .project_pause_availability(&project_path)
            .await
            .expect("clean up retry readiness runtime");
    }

    #[tokio::test]
    async fn invalid_readiness_contract_fails_before_controller_creation() {
        let dir = tempdir().expect("create invalid readiness directory");
        let project_path = dir.path().join("invalid-readiness-project");
        let (mut manager, instance_id, availability_data_dir) =
            availability_manager(dir.path(), &project_path, "invalid-readiness").await;
        tokio::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "invalid-readiness"
domain = "invalid-readiness.localhost"

[services.web]
command = "ignored by readiness fixture"
health_check = { type = "command" }
"#,
        )
        .await
        .expect("write invalid readiness config");
        let creates = Arc::new(AtomicUsize::new(0));
        manager.factories.insert(
            0,
            Arc::new(RetryingTcpReadinessFactory {
                creates: creates.clone(),
                stops: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let error = manager
            .project_ensure_availability(&project_path, DemandKey::manual_cli())
            .await
            .expect_err("incomplete command readiness must fail");
        assert!(format!("{error:#}").contains("requires `command`"));
        assert_eq!(creates.load(Ordering::SeqCst), 0);
        assert!(
            manager
                .get_service_controller("invalid-readiness:web")
                .await
                .is_none(),
            "invalid readiness cannot create or start a controller"
        );

        let mut availability = AvailabilityStore::load(&availability_data_dir, instance_id)
            .await
            .expect("load invalid readiness availability");
        assert!(
            availability
                .desired_up()
                .await
                .expect("invalid readiness preserves desired availability")
        );
        assert!(
            availability
                .snapshot()
                .await
                .expect("read invalid readiness convergence error")
                .last_convergence_error()
                .is_some_and(|message| message.contains("requires `command`"))
        );
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
