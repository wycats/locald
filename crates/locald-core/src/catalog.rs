//! Versioned, daemon-owned project identity catalog.
//!
//! The catalog makes opaque identities authoritative while retaining paths as
//! current or historical locators. It deliberately excludes availability,
//! readiness, and runtime state; those change at a different cadence and have
//! their own persistence model.

use crate::agent::AgentConversationKey;
use crate::identity::{
    IdentityError, ProjectId, ProjectIdentity, ProjectInstanceId, RepositoryId,
    ResolvedProjectIdentity, WorktreeId, derive_project_id, derive_project_instance_id,
    inspect_git_project_identity, inspect_repository_id, resolve_git_project_identity,
};
use crate::{DomainClaim, DomainError, DomainIndex, DomainName, DomainTarget};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// The current identity, exact-domain, and ambient-agent binding schema.
pub const CATALOG_VERSION: u32 = 4;
const PREVIOUS_CATALOG_VERSION: u32 = 3;
const LEGACY_CATALOG_VERSION: u32 = 2;

/// Paths used to initialize a catalog and collect legacy locator evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPaths {
    pub catalog: PathBuf,
    pub legacy_registry: PathBuf,
    pub legacy_attachments: PathBuf,
    pub legacy_runtime_state: PathBuf,
}

impl CatalogPaths {
    /// Build the standard file layout beneath a locald data directory.
    #[must_use]
    pub fn for_data_dir(data_dir: &Path) -> Self {
        Self {
            catalog: data_dir.join("catalog.json"),
            legacy_registry: data_dir.join("registry.json"),
            legacy_attachments: data_dir.join("attachments.json"),
            legacy_runtime_state: data_dir.join("state.json"),
        }
    }
}

/// Coarse filesystem presence, distinct from service availability.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogPresence {
    Active,
    Missing,
}

/// How a logical project received its identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectOrigin {
    Git {
        repository_id: RepositoryId,
        repository_relative_root: PathBuf,
    },
    NonGit,
}

/// How a physical project instance received its identity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectInstanceOrigin {
    Git { worktree_id: WorktreeId },
    NonGit,
}

/// Durable metadata for one physical Git clone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryRecord {
    pub id: RepositoryId,
    pub current_git_dir: Option<PathBuf>,
    pub last_known_git_dir: PathBuf,
    pub display_name: Option<String>,
    pub presence: CatalogPresence,
}

/// Durable metadata for one physical Git worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRecord {
    pub id: WorktreeId,
    pub repository_id: RepositoryId,
    pub current_path: Option<PathBuf>,
    pub last_known_path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub display_name: Option<String>,
    pub presence: CatalogPresence,
}

/// Durable metadata for one logical locald project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub origin: ProjectOrigin,
    pub display_name: Option<String>,
}

/// Durable metadata for a logical project in one physical workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectInstanceRecord {
    pub id: ProjectInstanceId,
    pub project_id: ProjectId,
    pub origin: ProjectInstanceOrigin,
    pub current_path: Option<PathBuf>,
    pub last_known_path: PathBuf,
    pub presence: CatalogPresence,
    pub display_name: Option<String>,
    pub pinned: bool,
    pub last_seen: SystemTime,
    #[serde(default)]
    pub domain_slug: Option<String>,
    #[serde(default)]
    pub domain_claims: BTreeSet<String>,
}

/// Legacy stores that supplied a path before it could be assigned an identity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum LegacyLocatorSource {
    Registry,
    Attachment,
    ManualStop,
    RuntimeState,
}

/// Preserved evidence for a legacy path whose identity cannot currently be read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UnresolvedLegacyProject {
    pub path: PathBuf,
    pub display_name: Option<String>,
    pub pinned: bool,
    pub last_seen: Option<SystemTime>,
    pub sources: BTreeSet<LegacyLocatorSource>,
}

/// Path-oriented compatibility projection used by the existing IPC surface.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "SystemTime::now")]
    pub last_seen: SystemTime,
}

/// A discovered project before it is reconciled with persistent catalog state.
#[derive(Debug, Clone)]
pub enum ProjectDiscovery {
    Git {
        resolved: Box<ResolvedProjectIdentity>,
        branch: Option<String>,
        head: Option<String>,
    },
    NonGit {
        project_root: PathBuf,
    },
}

impl ProjectDiscovery {
    /// Return the canonical project root associated with this discovery.
    #[must_use]
    pub fn project_root(&self) -> &Path {
        match self {
            Self::Git { resolved, .. } => &resolved.project_root,
            Self::NonGit { project_root } => project_root,
        }
    }

    /// Return the stable physical instance identity when Git supplies it.
    #[must_use]
    pub fn git_project_instance_id(&self) -> Option<ProjectInstanceId> {
        match self {
            Self::Git { resolved, .. } => Some(resolved.identity.project_instance_id),
            Self::NonGit { .. } => None,
        }
    }

    /// Whether this project lives in a linked Git worktree rather than the
    /// repository's primary checkout.
    #[must_use]
    pub fn is_linked_worktree(&self) -> bool {
        matches!(
            self,
            Self::Git { resolved, .. } if resolved.common_git_dir != resolved.worktree_git_dir
        )
    }
}

/// A catalog load, validation, discovery, or persistence failure.
#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("failed to {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("catalog `{path}` uses unsupported schema version {found}; expected {expected}")]
    UnsupportedVersion {
        path: PathBuf,
        found: u64,
        expected: u32,
    },

    #[error("invalid catalog or legacy state `{path}`: {reason}")]
    InvalidData { path: PathBuf, reason: String },

    #[error(transparent)]
    Identity(#[from] IdentityError),

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("catalog {operation} task failed: {reason}")]
    TaskJoin {
        operation: &'static str,
        reason: String,
    },

    #[error(
        "{kind} identity `{identity}` is simultaneously live at `{existing}` and `{discovered}`; copied locald identity markers must be removed before either location can be managed"
    )]
    LiveIdentityConflict {
        kind: &'static str,
        identity: String,
        existing: PathBuf,
        discovered: PathBuf,
    },

    #[error("catalog invariant failed: {0}")]
    Invariant(String),

    #[error("catalog `{path}` was published and its parent-directory sync failed: {reason}")]
    PublishedNotDurable { path: PathBuf, reason: String },

    #[error("agent conversation is already bound to another project instance")]
    AgentBindingConflict,
}

/// Versioned durable identity catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectCatalog {
    version: u32,
    pub repositories: BTreeMap<RepositoryId, RepositoryRecord>,
    pub worktrees: BTreeMap<WorktreeId, WorktreeRecord>,
    pub projects: BTreeMap<ProjectId, ProjectRecord>,
    pub instances: BTreeMap<ProjectInstanceId, ProjectInstanceRecord>,
    pub legacy_paths: BTreeMap<PathBuf, ProjectInstanceId>,
    pub unresolved_legacy: BTreeMap<PathBuf, UnresolvedLegacyProject>,
    pub domain_index: DomainIndex,
    #[serde(default)]
    agent_bindings: BTreeMap<AgentConversationKey, ProjectInstanceId>,
    #[serde(skip, default = "ProjectCatalog::path")]
    storage_path: PathBuf,
}

impl Default for ProjectCatalog {
    fn default() -> Self {
        Self::empty_at(Self::path())
    }
}

impl ProjectCatalog {
    fn empty_at(storage_path: PathBuf) -> Self {
        Self {
            version: CATALOG_VERSION,
            repositories: BTreeMap::new(),
            worktrees: BTreeMap::new(),
            projects: BTreeMap::new(),
            instances: BTreeMap::new(),
            legacy_paths: BTreeMap::new(),
            unresolved_legacy: BTreeMap::new(),
            domain_index: DomainIndex::default(),
            agent_bindings: BTreeMap::new(),
            storage_path,
        }
    }

    /// Create an empty catalog with an explicit persistence path.
    #[must_use]
    pub fn with_path(storage_path: PathBuf) -> Self {
        Self::empty_at(storage_path)
    }

    /// Get the standard identity catalog path.
    #[must_use]
    pub fn path() -> PathBuf {
        data_dir().join("catalog.json")
    }

    /// Return the persistence path owned by this in-memory catalog image.
    ///
    /// Lifecycle transaction journals intentionally omit this locator from
    /// their serialized catalog images. Recovery restores the live writer's
    /// path before comparing or publishing a prepared image.
    #[must_use]
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// Rebind a deserialized catalog image to the live writer's persistence
    /// path before validation and publication.
    pub fn set_storage_path(&mut self, storage_path: PathBuf) {
        self.storage_path = storage_path;
    }

    /// Get the predecessor path-keyed registry path.
    #[must_use]
    pub fn legacy_registry_path() -> PathBuf {
        data_dir().join("registry.json")
    }

    /// Load the catalog, importing locator evidence if no catalog exists yet.
    ///
    /// Existing catalog state is authoritative. Supported v2 state is migrated
    /// atomically to v3; malformed or unsupported state is never replaced from
    /// legacy inputs.
    pub async fn load() -> Result<Self, CatalogError> {
        Self::load_from_paths(CatalogPaths::for_data_dir(&data_dir())).await
    }

    /// Load the exact durable catalog image before lifecycle-journal recovery.
    ///
    /// Existing state is intentionally not reconciled with the filesystem:
    /// doing so could publish a third image while a lifecycle transaction is
    /// waiting to replay its recorded base or target. Once recovery completes,
    /// the manager reconciles presence through the same journaled publication
    /// boundary as every other catalog mutation.
    pub async fn load_for_lifecycle_recovery(
        allow_legacy_bootstrap: bool,
    ) -> Result<Self, CatalogError> {
        Self::load_from_paths_for_lifecycle_recovery(
            CatalogPaths::for_data_dir(&data_dir()),
            allow_legacy_bootstrap,
        )
        .await
    }

    /// Explicit-path variant of [`Self::load_for_lifecycle_recovery`].
    pub async fn load_from_paths_for_lifecycle_recovery(
        paths: CatalogPaths,
        allow_legacy_bootstrap: bool,
    ) -> Result<Self, CatalogError> {
        if file_exists(&paths.catalog).await? {
            return Self::load_existing_for_lifecycle_recovery(&paths.catalog).await;
        }
        if allow_legacy_bootstrap {
            // Recovery needs the complete imported image in memory, but the
            // lifecycle migration journal owns its first durable publication
            // after the raw v1 inputs have been backed up.
            Self::build_legacy_candidate(paths).await
        } else {
            Err(CatalogError::InvalidData {
                path: paths.catalog,
                reason: "authoritative catalog is missing after lifecycle-v2 state was published"
                    .to_owned(),
            })
        }
    }

    /// Load from explicit catalog and legacy paths.
    pub async fn load_from_paths(paths: CatalogPaths) -> Result<Self, CatalogError> {
        if file_exists(&paths.catalog).await? {
            let mut catalog = Self::load_existing(&paths.catalog).await?;
            if catalog.reconcile_missing()? > 0 {
                catalog.save().await?;
            }
            return Ok(catalog);
        }

        let catalog_path = paths.catalog.clone();
        let candidate = Self::build_legacy_candidate(paths).await?;

        if publish_new_catalog(&candidate, &catalog_path).await? {
            Ok(candidate)
        } else {
            Self::load_existing(&catalog_path).await
        }
    }

    async fn build_legacy_candidate(paths: CatalogPaths) -> Result<Self, CatalogError> {
        let evidence = normalize_legacy_evidence(collect_legacy_evidence(&paths).await?)?;
        let mut candidate = Self::empty_at(paths.catalog);

        for (path, legacy) in evidence {
            match tokio::fs::canonicalize(&path).await {
                Ok(_) => match Self::discover(path.clone()).await {
                    Ok(discovery) => {
                        candidate.apply_discovery(
                            discovery,
                            legacy.display_name,
                            legacy.pinned,
                            legacy.last_seen.unwrap_or(UNIX_EPOCH),
                            Some(path),
                        )?;
                    }
                    Err(CatalogError::Io { .. }) => {
                        candidate.unresolved_legacy.insert(path, legacy);
                    }
                    Err(CatalogError::Identity(error)) if unavailable_git_locator(&error) => {
                        candidate.unresolved_legacy.insert(path, legacy);
                    }
                    Err(error) => return Err(error),
                },
                Err(_) => {
                    candidate.unresolved_legacy.insert(path, legacy);
                }
            }
        }

        candidate.validate()?;
        Ok(candidate)
    }

    async fn load_existing(path: &Path) -> Result<Self, CatalogError> {
        Self::load_existing_with_parent_sync(path, |path| async move { sync_parent(&path).await })
            .await
    }

    async fn load_existing_for_lifecycle_recovery(path: &Path) -> Result<Self, CatalogError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|source| CatalogError::Io {
                operation: "read catalog for lifecycle recovery",
                path: path.to_path_buf(),
                source,
            })?;
        if content.trim().is_empty() {
            return Err(CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: "existing catalog is empty".to_owned(),
            });
        }
        let value: Value =
            serde_json::from_str(&content).map_err(|source| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: source.to_string(),
            })?;
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: "missing unsigned `version`".to_owned(),
            })?;
        match version {
            version if version == u64::from(CATALOG_VERSION) => {
                Self::deserialize_current(value, path)
            }
            version if version == u64::from(PREVIOUS_CATALOG_VERSION) => {
                Self::migrate_v3(value, path)
            }
            version if version == u64::from(LEGACY_CATALOG_VERSION) => {
                Self::migrate_v2(value, path)
            }
            found => Err(CatalogError::UnsupportedVersion {
                path: path.to_path_buf(),
                found,
                expected: CATALOG_VERSION,
            }),
        }
    }

    async fn load_existing_with_parent_sync<Sync, SyncFuture>(
        path: &Path,
        parent_sync: Sync,
    ) -> Result<Self, CatalogError>
    where
        Sync: FnOnce(PathBuf) -> SyncFuture,
        SyncFuture: std::future::Future<Output = Result<(), CatalogError>>,
    {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|source| CatalogError::Io {
                operation: "read catalog",
                path: path.to_path_buf(),
                source,
            })?;
        if content.trim().is_empty() {
            return Err(CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: "existing catalog is empty".to_owned(),
            });
        }

        let value: Value =
            serde_json::from_str(&content).map_err(|source| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: source.to_string(),
            })?;
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: "missing unsigned `version`".to_owned(),
            })?;
        match version {
            version if version == u64::from(CATALOG_VERSION) => {
                Self::deserialize_current(value, path)
            }
            version if version == u64::from(PREVIOUS_CATALOG_VERSION) => {
                let catalog = Self::migrate_v3(value, path)?;
                match replace_catalog_with_parent_sync(&catalog, path, parent_sync).await {
                    Ok(()) | Err(CatalogError::PublishedNotDurable { .. }) => Ok(catalog),
                    Err(error) => Err(error),
                }
            }
            version if version == u64::from(LEGACY_CATALOG_VERSION) => {
                let catalog = Self::migrate_v2(value, path)?;
                match replace_catalog_with_parent_sync(&catalog, path, parent_sync).await {
                    Ok(()) | Err(CatalogError::PublishedNotDurable { .. }) => Ok(catalog),
                    Err(error) => Err(error),
                }
            }
            found => Err(CatalogError::UnsupportedVersion {
                path: path.to_path_buf(),
                found,
                expected: CATALOG_VERSION,
            }),
        }
    }

    fn deserialize_current(value: Value, path: &Path) -> Result<Self, CatalogError> {
        if value.get("agent_bindings").is_none() {
            return Err(CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: "current catalog is missing `agent_bindings`".to_owned(),
            });
        }
        let mut catalog: Self =
            serde_json::from_value(value).map_err(|source| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: source.to_string(),
            })?;
        catalog.storage_path = path.to_path_buf();
        catalog.validate()?;
        Ok(catalog)
    }

    fn migrate_v3(mut value: Value, path: &Path) -> Result<Self, CatalogError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: "catalog must be an object".to_owned(),
            })?;
        object.insert("version".to_owned(), Value::from(CATALOG_VERSION));
        object.insert(
            "agent_bindings".to_owned(),
            Value::Object(serde_json::Map::new()),
        );
        Self::deserialize_current(value, path)
    }

    fn migrate_v2(mut value: Value, path: &Path) -> Result<Self, CatalogError> {
        let had_domain_index = value.get("domain_index").is_some();
        let object = value
            .as_object_mut()
            .ok_or_else(|| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: "catalog must be an object".to_owned(),
            })?;
        object.insert("version".to_owned(), Value::from(CATALOG_VERSION));
        object.insert(
            "agent_bindings".to_owned(),
            Value::Object(serde_json::Map::new()),
        );
        if !had_domain_index {
            object.insert(
                "domain_index".to_owned(),
                serde_json::to_value(DomainIndex::default()).map_err(|source| {
                    CatalogError::InvalidData {
                        path: path.to_path_buf(),
                        reason: source.to_string(),
                    }
                })?,
            );
        }

        let mut catalog: Self =
            serde_json::from_value(value).map_err(|source| CatalogError::InvalidData {
                path: path.to_path_buf(),
                reason: source.to_string(),
            })?;
        catalog.storage_path = path.to_path_buf();
        if !had_domain_index {
            catalog.hydrate_legacy_domain_index()?;
        }
        catalog.validate()?;
        Ok(catalog)
    }

    /// Persist the catalog with file sync, atomic replacement, and directory sync.
    pub async fn save(&self) -> Result<(), CatalogError> {
        self.validate()?;
        replace_catalog(self, &self.storage_path).await
    }

    /// Resolve project identity and mutable Git display metadata off the async executor.
    pub async fn discover(project_root: PathBuf) -> Result<ProjectDiscovery, CatalogError> {
        tokio::task::spawn_blocking(move || discover_project(&project_root))
            .await
            .map_err(|source| CatalogError::TaskJoin {
                operation: "discovery",
                reason: source.to_string(),
            })?
    }

    /// Reconcile a discovered project and return its stable project-instance ID.
    pub fn register_project(
        &mut self,
        discovery: ProjectDiscovery,
        display_name: Option<String>,
    ) -> Result<ProjectInstanceId, CatalogError> {
        let path = discovery.project_root().to_path_buf();
        let mut legacy_names = Vec::new();
        for legacy_path in self.matching_unresolved_paths(&path) {
            if let Some(record) = self.unresolved_legacy.get(&legacy_path) {
                legacy_names.push((
                    legacy_path,
                    record.display_name.clone(),
                    record.pinned,
                    record.last_seen,
                ));
            }
        }

        let inherited_name =
            display_name.or_else(|| legacy_names.iter().find_map(|(_, name, _, _)| name.clone()));
        let inherited_pin = legacy_names.iter().any(|(_, _, pinned, _)| *pinned);
        let last_seen = SystemTime::now();

        let mut candidate = self.clone();
        let instance_id =
            candidate.apply_discovery(discovery, inherited_name, inherited_pin, last_seen, None)?;
        for (legacy_path, _, _, _) in legacy_names {
            candidate.unresolved_legacy.remove(&legacy_path);
            candidate.legacy_paths.insert(legacy_path, instance_id);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(instance_id)
    }

    /// Reconcile and persist a project as one in-memory transaction.
    pub async fn register_project_and_save(
        &mut self,
        discovery: ProjectDiscovery,
        display_name: Option<String>,
    ) -> Result<ProjectInstanceId, CatalogError> {
        let mut working = self.clone();
        let (candidate, instance_id) = tokio::task::spawn_blocking(move || {
            let instance_id = working.register_project(discovery, display_name)?;
            Ok::<_, CatalogError>((working, instance_id))
        })
        .await
        .map_err(|source| CatalogError::TaskJoin {
            operation: "registration",
            reason: source.to_string(),
        })??;
        self.commit_candidate(candidate).await?;
        Ok(instance_id)
    }

    /// Publish a complete candidate and keep memory aligned with the atomic
    /// replacement commit point.
    pub async fn commit_candidate(&mut self, candidate: Self) -> Result<(), CatalogError> {
        self.commit_candidate_with_parent_sync(
            candidate,
            |path| async move { sync_parent(&path).await },
        )
        .await
    }

    /// Return the complete persistent exact-domain ownership index.
    #[must_use]
    pub const fn domain_index(&self) -> &DomainIndex {
        &self.domain_index
    }

    /// Return the durable project-instance binding for one opaque conversation.
    #[must_use]
    pub fn agent_binding(&self, conversation: &AgentConversationKey) -> Option<ProjectInstanceId> {
        self.agent_bindings.get(conversation).copied()
    }

    /// Upgrade one catalog image embedded in a replayable lifecycle journal.
    ///
    /// Catalog files migrate before use, but a prepared transaction may retain
    /// exact version-3 before/after images across the daemon upgrade.
    pub fn upgrade_embedded_schema(&mut self) -> Result<bool, CatalogError> {
        let upgraded = match self.version {
            CATALOG_VERSION => false,
            PREVIOUS_CATALOG_VERSION => {
                self.version = CATALOG_VERSION;
                self.agent_bindings.clear();
                true
            }
            version => {
                return Err(CatalogError::Invariant(format!(
                    "embedded catalog schema version {version} cannot be upgraded to {CATALOG_VERSION}"
                )));
            }
        };
        self.validate()?;
        Ok(upgraded)
    }

    /// Bind one opaque conversation to its first resolved project instance.
    ///
    /// Repeating the same binding is idempotent. A different target fails
    /// closed so mutable workspace labels cannot silently retarget a chat.
    pub fn bind_agent_conversation(
        &mut self,
        conversation: AgentConversationKey,
        instance_id: ProjectInstanceId,
    ) -> Result<bool, CatalogError> {
        conversation.validate().map_err(|error| {
            CatalogError::Invariant(format!("invalid agent conversation key: {error}"))
        })?;
        if !self.instances.contains_key(&instance_id) {
            return Err(CatalogError::Invariant(
                "agent conversation binding references a missing project instance".to_owned(),
            ));
        }
        match self.agent_bindings.get(&conversation) {
            Some(bound) if *bound == instance_id => Ok(false),
            Some(_) => Err(CatalogError::AgentBindingConflict),
            None => {
                self.agent_bindings.insert(conversation, instance_id);
                self.validate()?;
                Ok(true)
            }
        }
    }

    /// Replace one instance's complete exact claim set in memory.
    ///
    /// Callers commit the resulting catalog candidate through [`Self::commit_candidate`]
    /// so identity reconciliation and domain ownership publish together.
    pub fn replace_domain_claims(
        &mut self,
        instance_id: ProjectInstanceId,
        claims: impl IntoIterator<Item = DomainClaim>,
    ) -> Result<(), CatalogError> {
        if !self.instances.contains_key(&instance_id) {
            return Err(CatalogError::Invariant(format!(
                "domain claims reference missing project instance {instance_id}"
            )));
        }
        let replacement = self.domain_index.replacing_instance(instance_id, claims)?;
        let domains = replacement.domains_for_instance(instance_id);
        let record = self.instances.get_mut(&instance_id).ok_or_else(|| {
            CatalogError::Invariant(format!(
                "domain claims reference missing project instance {instance_id}"
            ))
        })?;
        record.domain_claims = domains;
        self.domain_index = replacement;
        self.validate()?;
        Ok(())
    }

    /// Allocate and persist the stable DNS label for a linked worktree.
    ///
    /// Mutable task, branch, and path labels are allocation hints only. Once
    /// assigned, a slug remains reserved until the project instance is
    /// explicitly forgotten.
    pub fn ensure_worktree_domain_slug(
        &mut self,
        instance_id: ProjectInstanceId,
        is_linked_worktree: bool,
        trusted_label: Option<&str>,
    ) -> Result<Option<String>, CatalogError> {
        let record = self.instances.get(&instance_id).ok_or_else(|| {
            CatalogError::Invariant(format!(
                "domain slug references missing project instance {instance_id}"
            ))
        })?;
        if let Some(slug) = &record.domain_slug {
            return Ok(Some(slug.clone()));
        }
        if !is_linked_worktree {
            return Ok(None);
        }

        let project_id = record.project_id;
        let ProjectInstanceOrigin::Git { worktree_id } = record.origin else {
            return Ok(None);
        };
        let worktree = self.worktrees.get(&worktree_id).ok_or_else(|| {
            CatalogError::Invariant(format!(
                "project instance {instance_id} references missing worktree {worktree_id}"
            ))
        })?;
        let branch_hint = worktree
            .branch
            .as_deref()
            .map(crate::worktree::branch_last_segment);
        let path_hint = worktree.display_name.as_deref();
        let fallback = format!("wt-{}", &instance_id.to_string()[..8]);
        let base = trusted_label
            .into_iter()
            .chain(branch_hint)
            .chain(path_hint)
            .find_map(crate::worktree::sanitize_slug_hint)
            .unwrap_or(fallback);
        let reserved = self
            .instances
            .values()
            .filter(|candidate| candidate.project_id == project_id)
            .filter_map(|candidate| candidate.domain_slug.as_deref())
            .collect::<BTreeSet<_>>();
        let slug = allocate_unique_domain_slug(&base, &reserved);

        let mutable_record = self.instances.get_mut(&instance_id).ok_or_else(|| {
            CatalogError::Invariant(format!(
                "domain slug allocation lost project instance {instance_id}"
            ))
        })?;
        mutable_record.domain_slug = Some(slug.clone());
        self.validate()?;
        Ok(Some(slug))
    }

    fn hydrate_legacy_domain_index(&mut self) -> Result<(), CatalogError> {
        let instances = self
            .instances
            .iter()
            .map(|(instance_id, record)| (*instance_id, record.domain_claims.clone()))
            .collect::<Vec<_>>();
        let mut index = DomainIndex::default();
        for (instance_id, domains) in instances {
            let claims = domains
                .into_iter()
                .map(|domain| {
                    Ok(DomainClaim::legacy(
                        domain.parse::<DomainName>()?,
                        instance_id,
                    ))
                })
                .collect::<Result<Vec<_>, DomainError>>()?;
            index = index.replacing_instance(instance_id, claims)?;
        }
        for (instance_id, record) in &mut self.instances {
            record.domain_claims = index.domains_for_instance(*instance_id);
        }
        self.domain_index = index;
        Ok(())
    }

    async fn commit_candidate_with_parent_sync<Sync, SyncFuture>(
        &mut self,
        candidate: Self,
        parent_sync: Sync,
    ) -> Result<(), CatalogError>
    where
        Sync: FnOnce(PathBuf) -> SyncFuture,
        SyncFuture: std::future::Future<Output = Result<(), CatalogError>>,
    {
        candidate.validate()?;
        let result =
            replace_catalog_with_parent_sync(&candidate, &candidate.storage_path, parent_sync)
                .await;
        if result.is_ok() || matches!(&result, Err(CatalogError::PublishedNotDurable { .. })) {
            *self = candidate;
        }
        result
    }

    fn apply_discovery(
        &mut self,
        discovery: ProjectDiscovery,
        display_name: Option<String>,
        pinned: bool,
        last_seen: SystemTime,
        legacy_path: Option<PathBuf>,
    ) -> Result<ProjectInstanceId, CatalogError> {
        let mut candidate = self.clone();
        let instance_id = match discovery {
            ProjectDiscovery::Git {
                resolved,
                branch,
                head,
            } => candidate.apply_git_discovery(
                *resolved,
                branch,
                head,
                display_name,
                pinned,
                last_seen,
            )?,
            ProjectDiscovery::NonGit { project_root } => {
                candidate.apply_non_git_discovery(project_root, display_name, pinned, last_seen)?
            }
        };

        if let Some(path) = legacy_path {
            candidate.legacy_paths.insert(path, instance_id);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(instance_id)
    }

    fn apply_git_discovery(
        &mut self,
        resolved: ResolvedProjectIdentity,
        branch: Option<String>,
        head: Option<String>,
        display_name: Option<String>,
        pinned: bool,
        last_seen: SystemTime,
    ) -> Result<ProjectInstanceId, CatalogError> {
        let ProjectIdentity {
            repository_id,
            worktree_id,
            project_id,
            project_instance_id,
        } = resolved.identity;

        self.retire_repository_path_claims(&resolved.common_git_dir, repository_id);
        self.retire_worktree_path_claims(&resolved.worktree_root, worktree_id);
        self.retire_path_claims(&resolved.project_root, project_instance_id);

        let repository_conflict = match self.repositories.get(&repository_id) {
            Some(record) => repository_conflicting_locator(
                record.current_git_dir.as_deref(),
                &record.last_known_git_dir,
                &resolved.common_git_dir,
                repository_id,
            )?,
            None => None,
        };
        let worktree_conflict = match self.worktrees.get(&worktree_id) {
            Some(record) => git_conflicting_locator(
                record.current_path.as_deref(),
                &record.last_known_path,
                &resolved.worktree_root,
                |identity| identity.worktree_id == worktree_id,
            )?,
            None => None,
        };
        let instance_conflict = match self.instances.get(&project_instance_id) {
            Some(record) => git_conflicting_locator(
                record.current_path.as_deref(),
                &record.last_known_path,
                &resolved.project_root,
                |identity| identity.project_instance_id == project_instance_id,
            )?,
            None => None,
        };

        if let Some(record) = self.repositories.get_mut(&repository_id) {
            reconcile_locator(
                "repository",
                repository_id.to_string(),
                &mut record.current_git_dir,
                &mut record.last_known_git_dir,
                &resolved.common_git_dir,
                repository_conflict,
            )?;
            record.display_name = repository_display_name(&resolved.common_git_dir);
            record.presence = CatalogPresence::Active;
        } else {
            self.repositories.insert(
                repository_id,
                RepositoryRecord {
                    id: repository_id,
                    current_git_dir: Some(resolved.common_git_dir.clone()),
                    last_known_git_dir: resolved.common_git_dir.clone(),
                    display_name: repository_display_name(&resolved.common_git_dir),
                    presence: CatalogPresence::Active,
                },
            );
        }

        if let Some(record) = self.worktrees.get_mut(&worktree_id) {
            if record.repository_id != repository_id {
                return Err(CatalogError::Invariant(format!(
                    "worktree {worktree_id} changed repository identity"
                )));
            }
            reconcile_locator(
                "worktree",
                worktree_id.to_string(),
                &mut record.current_path,
                &mut record.last_known_path,
                &resolved.worktree_root,
                worktree_conflict,
            )?;
            record.branch = branch;
            record.head = head;
            record.display_name = path_display_name(&resolved.worktree_root);
            record.presence = CatalogPresence::Active;
        } else {
            self.worktrees.insert(
                worktree_id,
                WorktreeRecord {
                    id: worktree_id,
                    repository_id,
                    current_path: Some(resolved.worktree_root.clone()),
                    last_known_path: resolved.worktree_root.clone(),
                    branch,
                    head,
                    display_name: path_display_name(&resolved.worktree_root),
                    presence: CatalogPresence::Active,
                },
            );
        }

        let project_origin = ProjectOrigin::Git {
            repository_id,
            repository_relative_root: resolved.repository_relative_project_root.clone(),
        };
        if let Some(record) = self.projects.get_mut(&project_id) {
            if record.origin != project_origin {
                return Err(CatalogError::Invariant(format!(
                    "project {project_id} changed its Git origin"
                )));
            }
            if display_name.is_some() {
                record.display_name.clone_from(&display_name);
            }
        } else {
            self.projects.insert(
                project_id,
                ProjectRecord {
                    id: project_id,
                    origin: project_origin,
                    display_name: display_name.clone(),
                },
            );
        }

        let instance_origin = ProjectInstanceOrigin::Git { worktree_id };
        if let Some(record) = self.instances.get_mut(&project_instance_id) {
            if record.project_id != project_id || record.origin != instance_origin {
                return Err(CatalogError::Invariant(format!(
                    "project instance {project_instance_id} changed identity relationships"
                )));
            }
            reconcile_locator(
                "project instance",
                project_instance_id.to_string(),
                &mut record.current_path,
                &mut record.last_known_path,
                &resolved.project_root,
                instance_conflict,
            )?;
            record.presence = CatalogPresence::Active;
            if display_name.is_some() {
                record.display_name = display_name;
            }
            record.pinned |= pinned;
            record.last_seen = last_seen;
        } else {
            self.instances.insert(
                project_instance_id,
                ProjectInstanceRecord {
                    id: project_instance_id,
                    project_id,
                    origin: instance_origin,
                    current_path: Some(resolved.project_root.clone()),
                    last_known_path: resolved.project_root.clone(),
                    presence: CatalogPresence::Active,
                    display_name,
                    pinned,
                    last_seen,
                    domain_slug: None,
                    domain_claims: BTreeSet::new(),
                },
            );
        }
        self.legacy_paths
            .insert(resolved.project_root, project_instance_id);
        Ok(project_instance_id)
    }

    fn apply_non_git_discovery(
        &mut self,
        project_root: PathBuf,
        display_name: Option<String>,
        pinned: bool,
        last_seen: SystemTime,
    ) -> Result<ProjectInstanceId, CatalogError> {
        let current_non_git = self.current_instance_at(&project_root).filter(|id| {
            matches!(
                self.instances.get(id).map(|record| record.origin),
                Some(ProjectInstanceOrigin::NonGit)
            )
        });
        let aliased_non_git = self.legacy_paths.get(&project_root).copied().filter(|id| {
            self.instances.get(id).is_some_and(|record| {
                matches!(record.origin, ProjectInstanceOrigin::NonGit)
                    && record.current_path.is_none()
                    && record.last_known_path == project_root
            })
        });
        let existing_id = current_non_git.or(aliased_non_git).or_else(|| {
            self.instances
                .iter()
                .filter(|(_, record)| {
                    matches!(record.origin, ProjectInstanceOrigin::NonGit)
                        && record.current_path.is_none()
                        && record.last_known_path == project_root
                })
                .max_by_key(|(id, record)| (record.last_seen, **id))
                .map(|(id, _)| *id)
        });
        if let Some(existing_id) = existing_id {
            self.retire_path_claims(&project_root, existing_id);
            let record = self
                .instances
                .get_mut(&existing_id)
                .ok_or_else(|| CatalogError::Invariant("missing non-Git instance".to_owned()))?;
            if display_name.is_some() {
                record.display_name.clone_from(&display_name);
            }
            record.pinned |= pinned;
            record.last_seen = last_seen;
            record.current_path = Some(project_root.clone());
            record.presence = CatalogPresence::Active;
            self.legacy_paths.insert(project_root, existing_id);
            return Ok(existing_id);
        }

        let project_id = ProjectId::random();
        let instance_id = ProjectInstanceId::random();
        self.retire_path_claims(&project_root, instance_id);
        self.projects.insert(
            project_id,
            ProjectRecord {
                id: project_id,
                origin: ProjectOrigin::NonGit,
                display_name: display_name.clone(),
            },
        );
        self.instances.insert(
            instance_id,
            ProjectInstanceRecord {
                id: instance_id,
                project_id,
                origin: ProjectInstanceOrigin::NonGit,
                current_path: Some(project_root.clone()),
                last_known_path: project_root.clone(),
                presence: CatalogPresence::Active,
                display_name,
                pinned,
                last_seen,
                domain_slug: None,
                domain_claims: BTreeSet::new(),
            },
        );
        self.legacy_paths.insert(project_root, instance_id);
        Ok(instance_id)
    }

    fn retire_path_claims(&mut self, path: &Path, replacement: ProjectInstanceId) {
        for (id, record) in &mut self.instances {
            if *id != replacement && record.current_path.as_deref() == Some(path) {
                record.current_path = None;
                record.last_known_path = path.to_path_buf();
                record.presence = CatalogPresence::Missing;
            }
        }
        self.legacy_paths.insert(path.to_path_buf(), replacement);
        self.unresolved_legacy.remove(path);
    }

    fn retire_repository_path_claims(&mut self, path: &Path, replacement: RepositoryId) {
        for (id, record) in &mut self.repositories {
            if *id != replacement && record.current_git_dir.as_deref() == Some(path) {
                record.current_git_dir = None;
                record.last_known_git_dir = path.to_path_buf();
                record.presence = CatalogPresence::Missing;
            }
        }
    }

    fn retire_worktree_path_claims(&mut self, path: &Path, replacement: WorktreeId) {
        for (id, record) in &mut self.worktrees {
            if *id != replacement && record.current_path.as_deref() == Some(path) {
                record.current_path = None;
                record.last_known_path = path.to_path_buf();
                record.presence = CatalogPresence::Missing;
            }
        }
    }

    fn matching_unresolved_paths(&self, path: &Path) -> Vec<PathBuf> {
        let normalized =
            crate::normalize_project_locator(path).unwrap_or_else(|_| path.to_path_buf());
        self.unresolved_legacy
            .keys()
            .filter(|candidate| {
                *candidate == path
                    || candidate.as_path() == normalized.as_path()
                    || crate::normalize_project_locator(candidate)
                        .is_ok_and(|resolved| resolved == normalized)
            })
            .cloned()
            .collect()
    }

    fn current_instance_at(&self, path: &Path) -> Option<ProjectInstanceId> {
        self.instances
            .iter()
            .find_map(|(id, record)| (record.current_path.as_deref() == Some(path)).then_some(*id))
    }

    fn instance_for_path(&self, path: &Path) -> Option<ProjectInstanceId> {
        let canonical =
            crate::normalize_project_locator(path).unwrap_or_else(|_| path.to_path_buf());
        self.current_instance_at(&canonical)
            .or_else(|| self.legacy_paths.get(&canonical).copied())
            .or_else(|| self.legacy_paths.get(path).copied())
            .or_else(|| {
                self.instances
                    .iter()
                    .filter(|(_, record)| {
                        record.last_known_path == canonical || record.last_known_path == path
                    })
                    .max_by_key(|(id, record)| (record.last_seen, **id))
                    .map(|(id, _)| *id)
            })
    }

    /// Resolve a current or historical path locator to its catalogued instance.
    #[must_use]
    pub fn project_instance_for_path(&self, path: &Path) -> Option<ProjectInstanceId> {
        self.instance_for_path(path)
    }

    fn entry_for_instance(&self, instance_id: ProjectInstanceId) -> Option<ProjectEntry> {
        self.instances.get(&instance_id).map(|record| ProjectEntry {
            path: record
                .current_path
                .clone()
                .unwrap_or_else(|| record.last_known_path.clone()),
            name: record.display_name.clone(),
            pinned: record.pinned,
            last_seen: record.last_seen,
        })
    }

    /// Return one compatibility entry for a path locator or historical alias.
    #[must_use]
    pub fn get_project(&self, path: &Path) -> Option<ProjectEntry> {
        if let Some(instance_id) = self.instance_for_path(path) {
            return self.entry_for_instance(instance_id);
        }
        let canonical =
            crate::normalize_project_locator(path).unwrap_or_else(|_| path.to_path_buf());
        self.unresolved_legacy
            .get(&canonical)
            .or_else(|| self.unresolved_legacy.get(path))
            .map(legacy_entry)
    }

    /// Return the path-based compatibility projection, sorted by path.
    #[must_use]
    pub fn project_entries(&self) -> Vec<ProjectEntry> {
        let mut by_path = BTreeMap::new();
        let mut latest_missing = BTreeMap::<PathBuf, (SystemTime, ProjectInstanceId)>::new();
        for (id, record) in self
            .instances
            .iter()
            .filter(|(_, record)| record.presence == CatalogPresence::Missing)
        {
            let entry = ProjectEntry {
                path: record.last_known_path.clone(),
                name: record.display_name.clone(),
                pinned: record.pinned,
                last_seen: record.last_seen,
            };
            let path = entry.path.clone();
            let should_replace = latest_missing
                .get(&path)
                .is_none_or(|previous| (record.last_seen, *id) > *previous);
            if should_replace {
                latest_missing.insert(path.clone(), (record.last_seen, *id));
                by_path.insert(path, entry);
            }
        }
        for record in self.unresolved_legacy.values() {
            by_path
                .entry(record.path.clone())
                .or_insert_with(|| legacy_entry(record));
        }
        for record in self
            .instances
            .values()
            .filter(|record| record.presence == CatalogPresence::Active)
        {
            let entry = ProjectEntry {
                path: record
                    .current_path
                    .clone()
                    .unwrap_or_else(|| record.last_known_path.clone()),
                name: record.display_name.clone(),
                pinned: record.pinned,
                last_seen: record.last_seen,
            };
            by_path.insert(entry.path.clone(), entry);
        }
        by_path.into_values().collect()
    }

    /// Return the compatibility projection indexed by its current display path.
    #[must_use]
    pub fn project_entries_by_path(&self) -> HashMap<PathBuf, ProjectEntry> {
        self.project_entries()
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect()
    }

    /// Enable the transitional legacy pin bit for a known path.
    pub fn pin_project(&mut self, path: &Path) -> bool {
        if let Some(instance_id) = self.instance_for_path(path)
            && let Some(record) = self.instances.get_mut(&instance_id)
        {
            record.pinned = true;
            return true;
        }
        let unresolved_paths = self.matching_unresolved_paths(path);
        let mut changed = false;
        for unresolved_path in unresolved_paths {
            if let Some(record) = self.unresolved_legacy.get_mut(&unresolved_path) {
                record.pinned = true;
                changed = true;
            }
        }
        if changed {
            return true;
        }
        false
    }

    /// Disable the transitional legacy pin bit for a known path.
    pub fn unpin_project(&mut self, path: &Path) -> bool {
        if let Some(instance_id) = self.instance_for_path(path)
            && let Some(record) = self.instances.get_mut(&instance_id)
        {
            record.pinned = false;
            return true;
        }
        let unresolved_paths = self.matching_unresolved_paths(path);
        let mut changed = false;
        for unresolved_path in unresolved_paths {
            if let Some(record) = self.unresolved_legacy.get_mut(&unresolved_path) {
                record.pinned = false;
                changed = true;
            }
        }
        if changed {
            return true;
        }
        false
    }

    /// Explicitly forget a project catalog record while leaving resources alone.
    pub fn unregister_project(&mut self, path: &Path) -> Result<bool, CatalogError> {
        let unresolved_paths = self.matching_unresolved_paths(path);
        if !unresolved_paths.is_empty() {
            for unresolved_path in unresolved_paths {
                self.unresolved_legacy.remove(&unresolved_path);
            }
            return Ok(true);
        }
        let Some(instance_id) = self.instance_for_path(path) else {
            return Ok(false);
        };
        self.remove_instance(instance_id)?;
        Ok(true)
    }

    /// Refresh filesystem presence and forget missing unpinned records.
    ///
    /// Project resources remain intact. A legacy locator that has reappeared is
    /// retained for identity discovery the next time the project is opened.
    pub fn prune_missing_projects(&mut self) -> Result<usize, CatalogError> {
        self.reconcile_missing()?;
        let mut missing = Vec::new();
        for (id, record) in &self.instances {
            if record.presence != CatalogPresence::Missing
                || record.pinned
                || self
                    .agent_bindings
                    .values()
                    .any(|bound_instance| bound_instance == id)
            {
                continue;
            }
            let removable = match record.origin {
                ProjectInstanceOrigin::Git { .. } => {
                    !git_locator_matches(&record.last_known_path, |identity| {
                        identity.project_instance_id == *id
                    })?
                }
                ProjectInstanceOrigin::NonGit => true,
            };
            if removable {
                missing.push(*id);
            }
        }
        let unresolved: Vec<_> = self
            .unresolved_legacy
            .iter()
            .filter_map(|(path, record)| {
                if record.pinned {
                    return None;
                }
                match try_exists(path) {
                    Ok(false) => Some(path.clone()),
                    Ok(true) | Err(_) => None,
                }
            })
            .collect();
        for instance_id in missing.iter().copied() {
            self.remove_instance(instance_id)?;
        }
        for path in &unresolved {
            self.unresolved_legacy.remove(path);
        }
        self.validate()?;
        Ok(missing.len() + unresolved.len())
    }

    fn remove_instance(&mut self, instance_id: ProjectInstanceId) -> Result<(), CatalogError> {
        self.domain_index = self
            .domain_index
            .replacing_instance(instance_id, std::iter::empty())?;
        let Some(instance) = self.instances.remove(&instance_id) else {
            return Ok(());
        };
        self.legacy_paths.retain(|_, id| *id != instance_id);
        self.agent_bindings.retain(|_, id| *id != instance_id);

        if !self
            .instances
            .values()
            .any(|record| record.project_id == instance.project_id)
        {
            self.projects.remove(&instance.project_id);
        }

        if let ProjectInstanceOrigin::Git { worktree_id } = instance.origin
            && !self.instances.values().any(|record| {
                matches!(
                    record.origin,
                    ProjectInstanceOrigin::Git { worktree_id: candidate } if candidate == worktree_id
                )
            })
        {
            self.worktrees.remove(&worktree_id);
        }

        let used_repositories: BTreeSet<_> = self
            .projects
            .values()
            .filter_map(|record| match &record.origin {
                ProjectOrigin::Git { repository_id, .. } => Some(*repository_id),
                ProjectOrigin::NonGit => None,
            })
            .chain(self.worktrees.values().map(|record| record.repository_id))
            .collect();
        self.repositories
            .retain(|id, _| used_repositories.contains(id));
        Ok(())
    }

    /// Reconcile catalog presence with the filesystem without deleting records.
    pub fn reconcile_missing(&mut self) -> Result<usize, CatalogError> {
        let mut changed = 0;

        // First release every current locator that no longer resolves to its
        // recorded identity. A second pass can then reclaim restored locators
        // without colliding with stale current-path claims.
        for (repository_id, record) in &mut self.repositories {
            if let Some(path) = record.current_git_dir.clone()
                && !reconciliation_repository_locator_matches(&path, *repository_id)?
            {
                record.current_git_dir = None;
                record.last_known_git_dir = path;
                record.presence = CatalogPresence::Missing;
                changed += 1;
            }
        }
        for (worktree_id, record) in &mut self.worktrees {
            if let Some(path) = record.current_path.clone()
                && !reconciliation_git_locator_matches(&path, |identity| {
                    identity.repository_id == record.repository_id
                        && identity.worktree_id == *worktree_id
                })?
            {
                record.current_path = None;
                record.last_known_path = path;
                record.presence = CatalogPresence::Missing;
                changed += 1;
            }
        }
        for (instance_id, record) in &mut self.instances {
            let matches = match record.origin {
                ProjectInstanceOrigin::Git { .. } => {
                    record.current_path.as_deref().map_or(Ok(false), |path| {
                        reconciliation_git_locator_matches(path, |identity| {
                            identity.project_instance_id == *instance_id
                        })
                    })?
                }
                ProjectInstanceOrigin::NonGit => record
                    .current_path
                    .as_deref()
                    .map_or(Ok(false), try_exists)?,
            };
            if let Some(path) = record.current_path.clone()
                && !matches
            {
                record.current_path = None;
                record.last_known_path = path;
                record.presence = CatalogPresence::Missing;
                changed += 1;
            }
        }

        let mut current_repository_paths: BTreeSet<_> = self
            .repositories
            .values()
            .filter_map(|record| record.current_git_dir.clone())
            .collect();
        for (repository_id, record) in &mut self.repositories {
            if record.presence == CatalogPresence::Missing
                && !current_repository_paths.contains(&record.last_known_git_dir)
                && reconciliation_repository_locator_matches(
                    &record.last_known_git_dir,
                    *repository_id,
                )?
            {
                record.current_git_dir = Some(record.last_known_git_dir.clone());
                record.presence = CatalogPresence::Active;
                current_repository_paths.insert(record.last_known_git_dir.clone());
                changed += 1;
            }
        }

        let mut current_worktree_paths: BTreeSet<_> = self
            .worktrees
            .values()
            .filter_map(|record| record.current_path.clone())
            .collect();
        for (worktree_id, record) in &mut self.worktrees {
            if record.presence == CatalogPresence::Missing
                && !current_worktree_paths.contains(&record.last_known_path)
                && reconciliation_git_locator_matches(&record.last_known_path, |identity| {
                    identity.repository_id == record.repository_id
                        && identity.worktree_id == *worktree_id
                })?
            {
                record.current_path = Some(record.last_known_path.clone());
                record.presence = CatalogPresence::Active;
                current_worktree_paths.insert(record.last_known_path.clone());
                changed += 1;
            }
        }

        let mut current_instance_paths: BTreeSet<_> = self
            .instances
            .values()
            .filter_map(|record| record.current_path.clone())
            .collect();
        let mut missing_instances: Vec<_> = self
            .instances
            .iter()
            .filter_map(|(id, record)| (record.presence == CatalogPresence::Missing).then_some(*id))
            .collect();
        missing_instances.sort_by(|left, right| {
            let left_record = &self.instances[left];
            let right_record = &self.instances[right];
            let left_is_alias = self.legacy_paths.get(&left_record.last_known_path) == Some(left);
            let right_is_alias =
                self.legacy_paths.get(&right_record.last_known_path) == Some(right);
            right_is_alias
                .cmp(&left_is_alias)
                .then_with(|| right_record.last_seen.cmp(&left_record.last_seen))
                .then_with(|| right.cmp(left))
        });
        for instance_id in missing_instances {
            let missing_record = &self.instances[&instance_id];
            let path = missing_record.last_known_path.clone();
            if current_instance_paths.contains(&path) {
                continue;
            }
            let matches = match missing_record.origin {
                ProjectInstanceOrigin::Git { .. } => {
                    reconciliation_git_locator_matches(&path, |identity| {
                        identity.project_instance_id == instance_id
                    })?
                }
                ProjectInstanceOrigin::NonGit => try_exists(&path)?,
            };
            if matches {
                let restored_record = self.instances.get_mut(&instance_id).ok_or_else(|| {
                    CatalogError::Invariant("missing project instance".to_owned())
                })?;
                restored_record.current_path = Some(path.clone());
                restored_record.presence = CatalogPresence::Active;
                self.legacy_paths.insert(path.clone(), instance_id);
                self.unresolved_legacy.remove(&path);
                current_instance_paths.insert(path);
                changed += 1;
            }
        }

        self.validate()?;
        Ok(changed)
    }

    /// Validate all map keys, relationships, paths, and presence invariants.
    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.version != CATALOG_VERSION {
            return Err(CatalogError::Invariant(format!(
                "in-memory schema version {} is not {CATALOG_VERSION}",
                self.version
            )));
        }
        self.domain_index.validate()?;
        for instance_id in self.agent_bindings.values() {
            if !self.instances.contains_key(instance_id) {
                return Err(CatalogError::Invariant(
                    "agent conversation binding references a missing project instance".to_owned(),
                ));
            }
        }
        for (domain, target) in self.domain_index.claims() {
            if let DomainTarget::Service {
                project_instance_id,
                ..
            } = target
                && !self.instances.contains_key(project_instance_id)
            {
                return Err(CatalogError::Invariant(format!(
                    "domain `{domain}` references missing project instance {project_instance_id}"
                )));
            }
        }
        let mut current_repository_paths = BTreeMap::<&Path, RepositoryId>::new();
        for (id, record) in &self.repositories {
            if id != &record.id {
                return Err(CatalogError::Invariant(format!(
                    "repository map key {id} does not match record {}",
                    record.id
                )));
            }
            validate_uuid_version("repository", &id.to_string(), id.as_uuid(), 4)?;
            validate_presence(
                "repository",
                &id.to_string(),
                record.presence,
                record.current_git_dir.as_deref(),
            )?;
            if let Some(path) = record.current_git_dir.as_deref()
                && let Some(previous) = current_repository_paths.insert(path, *id)
            {
                return Err(CatalogError::Invariant(format!(
                    "repositories {previous} and {id} both claim current Git directory `{}`",
                    path.display()
                )));
            }
        }
        let mut current_worktree_paths = BTreeMap::<&Path, WorktreeId>::new();
        for (id, record) in &self.worktrees {
            if id != &record.id || !self.repositories.contains_key(&record.repository_id) {
                return Err(CatalogError::Invariant(format!(
                    "worktree {id} has an invalid key or repository relationship"
                )));
            }
            validate_uuid_version("worktree", &id.to_string(), id.as_uuid(), 4)?;
            validate_presence(
                "worktree",
                &id.to_string(),
                record.presence,
                record.current_path.as_deref(),
            )?;
            if let Some(path) = record.current_path.as_deref()
                && let Some(previous) = current_worktree_paths.insert(path, *id)
            {
                return Err(CatalogError::Invariant(format!(
                    "worktrees {previous} and {id} both claim current path `{}`",
                    path.display()
                )));
            }
        }
        for (id, record) in &self.projects {
            if id != &record.id {
                return Err(CatalogError::Invariant(format!(
                    "project map key {id} does not match record {}",
                    record.id
                )));
            }
            match &record.origin {
                ProjectOrigin::Git {
                    repository_id,
                    repository_relative_root,
                } => {
                    if !self.repositories.contains_key(repository_id) {
                        return Err(CatalogError::Invariant(format!(
                            "project {id} references missing repository {repository_id}"
                        )));
                    }
                    let expected = derive_project_id(
                        *repository_id,
                        repository_relative_root,
                        repository_relative_root,
                    )?;
                    if *id != expected {
                        return Err(CatalogError::Invariant(format!(
                            "Git project {id} does not match its repository and relative root"
                        )));
                    }
                }
                ProjectOrigin::NonGit => {
                    validate_uuid_version("non-Git project", &id.to_string(), id.as_uuid(), 4)?;
                }
            }
        }

        let mut current_paths = BTreeMap::<&Path, ProjectInstanceId>::new();
        let mut reserved_domain_slugs = BTreeMap::<(ProjectId, &str), ProjectInstanceId>::new();
        for (id, record) in &self.instances {
            if id != &record.id || !self.projects.contains_key(&record.project_id) {
                return Err(CatalogError::Invariant(format!(
                    "project instance {id} has an invalid key or project relationship"
                )));
            }
            validate_presence(
                "project instance",
                &id.to_string(),
                record.presence,
                record.current_path.as_deref(),
            )?;
            match (&record.origin, &self.projects[&record.project_id].origin) {
                (
                    ProjectInstanceOrigin::Git { worktree_id },
                    ProjectOrigin::Git { repository_id, .. },
                ) => {
                    let worktree = self.worktrees.get(worktree_id).ok_or_else(|| {
                        CatalogError::Invariant(format!(
                            "project instance {id} references missing worktree {worktree_id}"
                        ))
                    })?;
                    if worktree.repository_id != *repository_id {
                        return Err(CatalogError::Invariant(format!(
                            "project instance {id} crosses repository identities"
                        )));
                    }
                    let expected = derive_project_instance_id(*worktree_id, record.project_id);
                    if *id != expected {
                        return Err(CatalogError::Invariant(format!(
                            "Git project instance {id} does not match its worktree and project"
                        )));
                    }
                }
                (ProjectInstanceOrigin::NonGit, ProjectOrigin::NonGit) => {
                    validate_uuid_version(
                        "non-Git project instance",
                        &id.to_string(),
                        id.as_uuid(),
                        4,
                    )?;
                }
                _ => {
                    return Err(CatalogError::Invariant(format!(
                        "project instance {id} mixes Git and non-Git identities"
                    )));
                }
            }
            if let Some(path) = record.current_path.as_deref()
                && let Some(previous) = current_paths.insert(path, *id)
            {
                return Err(CatalogError::Invariant(format!(
                    "instances {previous} and {id} both claim current path `{}`",
                    path.display()
                )));
            }
            if let Some(slug) = record.domain_slug.as_deref() {
                if crate::worktree::sanitize_slug_hint(slug).as_deref() != Some(slug) {
                    return Err(CatalogError::Invariant(format!(
                        "project instance {id} has invalid persistent domain slug `{slug}`"
                    )));
                }
                if let Some(previous) = reserved_domain_slugs.insert((record.project_id, slug), *id)
                {
                    return Err(CatalogError::Invariant(format!(
                        "project instances {previous} and {id} both reserve domain slug `{slug}`"
                    )));
                }
            }
            let indexed_domains = self.domain_index.domains_for_instance(*id);
            if record.domain_claims != indexed_domains {
                return Err(CatalogError::Invariant(format!(
                    "project instance {id} domain claims do not match the persistent domain index"
                )));
            }
        }
        for (path, instance_id) in &self.legacy_paths {
            if !self.instances.contains_key(instance_id) {
                return Err(CatalogError::Invariant(format!(
                    "legacy path `{}` references missing instance {instance_id}",
                    path.display()
                )));
            }
            if let Some(current_instance_id) = current_paths.get(path.as_path())
                && current_instance_id != instance_id
            {
                return Err(CatalogError::Invariant(format!(
                    "legacy path `{}` references instance {instance_id} while active instance {current_instance_id} claims it",
                    path.display()
                )));
            }
            if self.unresolved_legacy.contains_key(path) {
                return Err(CatalogError::Invariant(format!(
                    "legacy path `{}` is both resolved and unresolved",
                    path.display()
                )));
            }
        }
        for (path, record) in &self.unresolved_legacy {
            if path != &record.path {
                return Err(CatalogError::Invariant(format!(
                    "unresolved legacy key `{}` does not match record `{}`",
                    path.display(),
                    record.path.display()
                )));
            }
            if current_paths.contains_key(path.as_path()) {
                return Err(CatalogError::Invariant(format!(
                    "unresolved legacy path `{}` is also claimed by an identified instance",
                    path.display()
                )));
            }
        }
        Ok(())
    }
}

fn data_dir() -> PathBuf {
    crate::storage::data_dir()
}

fn allocate_unique_domain_slug(base: &str, reserved: &BTreeSet<&str>) -> String {
    if !reserved.contains(base) {
        return base.to_owned();
    }

    for ordinal in 2_u64.. {
        let suffix = format!("-{ordinal}");
        let available = 63_usize.saturating_sub(suffix.len());
        let prefix = base[..base.len().min(available)].trim_end_matches('-');
        let candidate = format!("{prefix}{suffix}");
        if !reserved.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("the worktree slug suffix space is unbounded")
}

fn discover_project(project_root: &Path) -> Result<ProjectDiscovery, CatalogError> {
    match resolve_git_project_identity(project_root) {
        Ok(resolved) => {
            let repository = git2::Repository::open(&resolved.worktree_root).map_err(|source| {
                CatalogError::InvalidData {
                    path: resolved.worktree_root.clone(),
                    reason: format!("failed to reopen Git metadata: {source}"),
                }
            })?;
            let (branch, head) = repository.head().map_or((None, None), |reference| {
                (
                    reference
                        .shorthand()
                        .filter(|_| reference.is_branch())
                        .map(str::to_owned),
                    reference.target().map(|oid| oid.to_string()),
                )
            });
            Ok(ProjectDiscovery::Git {
                resolved: Box::new(resolved),
                branch,
                head,
            })
        }
        Err(IdentityError::NotGit { .. }) => {
            let project_root =
                std::fs::canonicalize(project_root).map_err(|source| CatalogError::Io {
                    operation: "resolve non-Git project path",
                    path: project_root.to_path_buf(),
                    source,
                })?;
            Ok(ProjectDiscovery::NonGit { project_root })
        }
        Err(error) => Err(error.into()),
    }
}

fn reconcile_locator(
    kind: &'static str,
    identity: String,
    current: &mut Option<PathBuf>,
    last_known: &mut PathBuf,
    discovered: &Path,
    conflicting_locator: Option<PathBuf>,
) -> Result<(), CatalogError> {
    if let Some(existing) = conflicting_locator {
        return Err(CatalogError::LiveIdentityConflict {
            kind,
            identity,
            existing,
            discovered: discovered.to_path_buf(),
        });
    }
    if current.as_deref() == Some(discovered) {
        return Ok(());
    }
    if let Some(previous) = current.replace(discovered.to_path_buf()) {
        *last_known = previous;
    }
    Ok(())
}

fn repository_conflicting_locator(
    current: Option<&Path>,
    last_known: &Path,
    discovered: &Path,
    expected: RepositoryId,
) -> Result<Option<PathBuf>, CatalogError> {
    for existing in locator_candidates(current, last_known, discovered) {
        if try_exists(existing)?
            && inspect_repository_id(existing)?.is_some_and(|identity| identity == expected)
        {
            return Ok(Some(existing.to_path_buf()));
        }
    }
    Ok(None)
}

fn git_conflicting_locator(
    current: Option<&Path>,
    last_known: &Path,
    discovered: &Path,
    matches: impl Fn(ProjectIdentity) -> bool,
) -> Result<Option<PathBuf>, CatalogError> {
    let candidates = locator_candidates(current, last_known, discovered);
    for existing in candidates {
        if try_exists(existing)? && inspect_git_project_identity(existing)?.is_some_and(&matches) {
            return Ok(Some(existing.to_path_buf()));
        }
    }
    Ok(None)
}

fn locator_candidates<'a>(
    current: Option<&'a Path>,
    last_known: &'a Path,
    discovered: &Path,
) -> Vec<&'a Path> {
    let mut candidates = Vec::with_capacity(2);
    if let Some(current) = current.filter(|current| *current != discovered) {
        candidates.push(current);
    }
    if last_known != discovered && !candidates.contains(&last_known) {
        candidates.push(last_known);
    }
    candidates
}

fn try_exists(path: &Path) -> Result<bool, CatalogError> {
    path.try_exists().map_err(|source| CatalogError::Io {
        operation: "inspect catalog locator",
        path: path.to_path_buf(),
        source,
    })
}

fn repository_locator_matches(path: &Path, expected: RepositoryId) -> Result<bool, CatalogError> {
    if !try_exists(path)? {
        return Ok(false);
    }
    Ok(inspect_repository_id(path)?.is_some_and(|identity| identity == expected))
}

fn reconciliation_repository_locator_matches(
    path: &Path,
    expected: RepositoryId,
) -> Result<bool, CatalogError> {
    match repository_locator_matches(path, expected) {
        Err(CatalogError::Io { .. } | CatalogError::Identity(IdentityError::MarkerIo { .. })) => {
            Ok(false)
        }
        result => result,
    }
}

fn git_locator_matches(
    path: &Path,
    matches: impl FnOnce(ProjectIdentity) -> bool,
) -> Result<bool, CatalogError> {
    if !try_exists(path)? {
        return Ok(false);
    }
    Ok(inspect_git_project_identity(path)?.is_some_and(matches))
}

fn reconciliation_git_locator_matches(
    path: &Path,
    matches: impl FnOnce(ProjectIdentity) -> bool,
) -> Result<bool, CatalogError> {
    match git_locator_matches(path, matches) {
        Err(CatalogError::Io { .. }) => Ok(false),
        Err(CatalogError::Identity(error)) if unavailable_git_locator(&error) => Ok(false),
        result => result,
    }
}

const fn unavailable_git_locator(error: &IdentityError) -> bool {
    matches!(
        error,
        IdentityError::CanonicalizeProjectRoot { .. }
            | IdentityError::BrokenWorktree { .. }
            | IdentityError::UnrepairedWorktree { .. }
            | IdentityError::BareRepository { .. }
            | IdentityError::CanonicalizeGitDirectory { .. }
            | IdentityError::ProjectOutsideWorktree { .. }
            | IdentityError::MarkerIo { .. }
    )
}

fn validate_presence(
    kind: &'static str,
    identity: &str,
    presence: CatalogPresence,
    current: Option<&Path>,
) -> Result<(), CatalogError> {
    if (presence == CatalogPresence::Active) != current.is_some() {
        return Err(CatalogError::Invariant(format!(
            "{kind} {identity} presence does not match its current locator"
        )));
    }
    Ok(())
}

fn validate_uuid_version(
    kind: &'static str,
    identity: &str,
    value: Uuid,
    expected: usize,
) -> Result<(), CatalogError> {
    if value.get_version_num() != expected {
        return Err(CatalogError::Invariant(format!(
            "{kind} {identity} must use UUID version {expected}"
        )));
    }
    Ok(())
}

fn path_display_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

fn repository_display_name(common_git_dir: &Path) -> Option<String> {
    common_git_dir.parent().and_then(path_display_name)
}

fn legacy_entry(record: &UnresolvedLegacyProject) -> ProjectEntry {
    ProjectEntry {
        path: record.path.clone(),
        name: record.display_name.clone(),
        pinned: record.pinned,
        last_seen: record.last_seen.unwrap_or(UNIX_EPOCH),
    }
}

async fn collect_legacy_evidence(
    paths: &CatalogPaths,
) -> Result<BTreeMap<PathBuf, UnresolvedLegacyProject>, CatalogError> {
    let mut evidence = BTreeMap::new();
    if let Some(value) = read_optional_json(&paths.legacy_registry).await? {
        collect_registry_evidence(&paths.legacy_registry, &value, &mut evidence)?;
    }
    collect_best_effort_compatibility_evidence(
        &paths.legacy_attachments,
        &mut evidence,
        collect_attachment_evidence,
    )
    .await;
    collect_best_effort_compatibility_evidence(
        &paths.legacy_runtime_state,
        &mut evidence,
        collect_runtime_evidence,
    )
    .await;
    Ok(evidence)
}

fn normalize_legacy_evidence(
    evidence: BTreeMap<PathBuf, UnresolvedLegacyProject>,
) -> Result<BTreeMap<PathBuf, UnresolvedLegacyProject>, CatalogError> {
    let mut normalized = BTreeMap::<PathBuf, UnresolvedLegacyProject>::new();
    for (source_path, mut record) in evidence {
        let path = crate::normalize_project_locator(&source_path)
            .or_else(|_| crate::locator::absolute_project_locator(&source_path))
            .map_err(|source| CatalogError::Io {
                operation: "normalize legacy project locator",
                path: source_path,
                source,
            })?;
        record.path.clone_from(&path);
        match normalized.entry(path) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(record);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let existing = entry.get_mut();
                if record.display_name.is_some()
                    && (existing.display_name.is_none() || record.last_seen > existing.last_seen)
                {
                    existing.display_name = record.display_name;
                }
                existing.pinned |= record.pinned;
                existing.last_seen = existing.last_seen.max(record.last_seen);
                existing.sources.extend(record.sources);
            }
        }
    }
    Ok(normalized)
}

type CompatibilityEvidenceCollector =
    fn(&Path, &Value, &mut BTreeMap<PathBuf, UnresolvedLegacyProject>) -> Result<(), CatalogError>;

async fn collect_best_effort_compatibility_evidence(
    path: &Path,
    evidence: &mut BTreeMap<PathBuf, UnresolvedLegacyProject>,
    collector: CompatibilityEvidenceCollector,
) {
    let Ok(Some(value)) = read_optional_json(path).await else {
        return;
    };
    let mut candidate = evidence.clone();
    if collector(path, &value, &mut candidate).is_ok() {
        *evidence = candidate;
    }
}

async fn read_optional_json(path: &Path) -> Result<Option<Value>, CatalogError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) if content.trim().is_empty() => Ok(None),
        Ok(content) => {
            serde_json::from_str(&content)
                .map(Some)
                .map_err(|source| CatalogError::InvalidData {
                    path: path.to_path_buf(),
                    reason: source.to_string(),
                })
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CatalogError::Io {
            operation: "read legacy state",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn collect_registry_evidence(
    source_path: &Path,
    value: &Value,
    evidence: &mut BTreeMap<PathBuf, UnresolvedLegacyProject>,
) -> Result<(), CatalogError> {
    let Some(projects) = optional_object_field(source_path, value, "projects")? else {
        return Ok(());
    };
    for (key, entry_value) in projects {
        let entry = entry_value
            .as_object()
            .ok_or_else(|| CatalogError::InvalidData {
                path: source_path.to_path_buf(),
                reason: format!("registry entry `{key}` must be an object"),
            })?;
        let path = PathBuf::from(key);
        if let Some(recorded_path) = entry.get("path") {
            let recorded_path =
                recorded_path
                    .as_str()
                    .ok_or_else(|| CatalogError::InvalidData {
                        path: source_path.to_path_buf(),
                        reason: format!("registry path for `{key}` must be a string"),
                    })?;
            if Path::new(recorded_path) != path {
                return Err(CatalogError::InvalidData {
                    path: source_path.to_path_buf(),
                    reason: format!(
                        "registry key `{key}` disagrees with entry path `{recorded_path}`"
                    ),
                });
            }
        }
        let record = evidence_record(evidence, path, LegacyLocatorSource::Registry);
        record.display_name = match entry.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) => {
                return Err(CatalogError::InvalidData {
                    path: source_path.to_path_buf(),
                    reason: format!("registry name for `{key}` must be a string or null"),
                });
            }
        };
        record.pinned |= match entry.get("pinned") {
            None => false,
            Some(Value::Bool(pinned)) => *pinned,
            Some(_) => {
                return Err(CatalogError::InvalidData {
                    path: source_path.to_path_buf(),
                    reason: format!("registry pinned flag for `{key}` must be a boolean"),
                });
            }
        };
        if let Some(last_seen) = entry.get("last_seen") {
            record.last_seen =
                Some(serde_json::from_value(last_seen.clone()).map_err(|source| {
                    CatalogError::InvalidData {
                        path: source_path.to_path_buf(),
                        reason: format!("invalid registry last_seen for `{key}`: {source}"),
                    }
                })?);
        }
    }
    Ok(())
}

fn collect_attachment_evidence(
    source_path: &Path,
    value: &Value,
    evidence: &mut BTreeMap<PathBuf, UnresolvedLegacyProject>,
) -> Result<(), CatalogError> {
    if let Some(attachments) = optional_object_field(source_path, value, "attachments")? {
        for key in attachments.keys() {
            evidence_record(
                evidence,
                PathBuf::from(key),
                LegacyLocatorSource::Attachment,
            );
        }
    }
    if let Some(stopped) = value.get("manually_stopped") {
        let stopped = stopped
            .as_array()
            .ok_or_else(|| CatalogError::InvalidData {
                path: source_path.to_path_buf(),
                reason: "`manually_stopped` must be an array".to_owned(),
            })?;
        for path in stopped {
            let path = path.as_str().ok_or_else(|| CatalogError::InvalidData {
                path: source_path.to_path_buf(),
                reason: "manual-stop project path must be a string".to_owned(),
            })?;
            evidence_record(
                evidence,
                PathBuf::from(path),
                LegacyLocatorSource::ManualStop,
            );
        }
    }
    Ok(())
}

fn collect_runtime_evidence(
    source_path: &Path,
    value: &Value,
    evidence: &mut BTreeMap<PathBuf, UnresolvedLegacyProject>,
) -> Result<(), CatalogError> {
    let Some(services_value) = value.get("services") else {
        ensure_object(source_path, value)?;
        return Ok(());
    };
    let services = services_value
        .as_array()
        .ok_or_else(|| CatalogError::InvalidData {
            path: source_path.to_path_buf(),
            reason: "`services` must be an array".to_owned(),
        })?;
    for service in services {
        let path = service.get("path").and_then(Value::as_str).ok_or_else(|| {
            CatalogError::InvalidData {
                path: source_path.to_path_buf(),
                reason: "runtime service path must be a string".to_owned(),
            }
        })?;
        evidence_record(
            evidence,
            PathBuf::from(path),
            LegacyLocatorSource::RuntimeState,
        );
    }
    Ok(())
}

fn optional_object_field<'a>(
    source_path: &Path,
    value: &'a Value,
    field: &str,
) -> Result<Option<&'a serde_json::Map<String, Value>>, CatalogError> {
    ensure_object(source_path, value)?;
    value.get(field).map_or(Ok(None), |field_value| {
        field_value
            .as_object()
            .map(Some)
            .ok_or_else(|| CatalogError::InvalidData {
                path: source_path.to_path_buf(),
                reason: format!("`{field}` must be an object"),
            })
    })
}

fn ensure_object(source_path: &Path, value: &Value) -> Result<(), CatalogError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(CatalogError::InvalidData {
            path: source_path.to_path_buf(),
            reason: "top-level legacy state must be an object".to_owned(),
        })
    }
}

fn evidence_record(
    evidence: &mut BTreeMap<PathBuf, UnresolvedLegacyProject>,
    path: PathBuf,
    source: LegacyLocatorSource,
) -> &mut UnresolvedLegacyProject {
    let record = evidence
        .entry(path.clone())
        .or_insert_with(|| UnresolvedLegacyProject {
            path,
            display_name: None,
            pinned: false,
            last_seen: None,
            sources: BTreeSet::new(),
        });
    record.sources.insert(source);
    record
}

async fn file_exists(path: &Path) -> Result<bool, CatalogError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(CatalogError::Io {
            operation: "inspect catalog path",
            path: path.to_path_buf(),
            source,
        }),
    }
}

async fn publish_new_catalog(catalog: &ProjectCatalog, path: &Path) -> Result<bool, CatalogError> {
    publish_new_catalog_with_parent_sync(
        catalog,
        path,
        |path| async move { sync_parent(&path).await },
    )
    .await
}

async fn publish_new_catalog_with_parent_sync<Sync, SyncFuture>(
    catalog: &ProjectCatalog,
    path: &Path,
    parent_sync: Sync,
) -> Result<bool, CatalogError>
where
    Sync: FnOnce(PathBuf) -> SyncFuture,
    SyncFuture: std::future::Future<Output = Result<(), CatalogError>>,
{
    let temporary = write_temporary_catalog(catalog, path).await?;
    match tokio::fs::hard_link(&temporary, path).await {
        Ok(()) => {
            let sync_result = parent_sync(path.to_path_buf()).await;
            let cleanup_result = remove_temporary(&temporary).await;
            if let Err(error) = sync_result {
                let reason = match cleanup_result {
                    Ok(()) => error.to_string(),
                    Err(cleanup_error) => {
                        format!("{error}; temporary catalog cleanup also failed: {cleanup_error}")
                    }
                };
                return Err(CatalogError::PublishedNotDurable {
                    path: path.to_path_buf(),
                    reason,
                });
            }
            cleanup_result?;
            Ok(true)
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            remove_temporary(&temporary).await?;
            Ok(false)
        }
        Err(source) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            Err(CatalogError::Io {
                operation: "publish catalog",
                path: path.to_path_buf(),
                source,
            })
        }
    }
}

async fn replace_catalog(catalog: &ProjectCatalog, path: &Path) -> Result<(), CatalogError> {
    replace_catalog_with_parent_sync(
        catalog,
        path,
        |path| async move { sync_parent(&path).await },
    )
    .await
}

async fn replace_catalog_with_parent_sync<Sync, SyncFuture>(
    catalog: &ProjectCatalog,
    path: &Path,
    parent_sync: Sync,
) -> Result<(), CatalogError>
where
    Sync: FnOnce(PathBuf) -> SyncFuture,
    SyncFuture: std::future::Future<Output = Result<(), CatalogError>>,
{
    let temporary = write_temporary_catalog(catalog, path).await?;
    if let Err(source) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(CatalogError::Io {
            operation: "replace catalog",
            path: path.to_path_buf(),
            source,
        });
    }
    parent_sync(path.to_path_buf())
        .await
        .map_err(|error| CatalogError::PublishedNotDurable {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })
}

async fn write_temporary_catalog(
    catalog: &ProjectCatalog,
    path: &Path,
) -> Result<PathBuf, CatalogError> {
    let parent = path.parent().ok_or_else(|| CatalogError::InvalidData {
        path: path.to_path_buf(),
        reason: "catalog path has no parent directory".to_owned(),
    })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|source| CatalogError::Io {
            operation: "create catalog directory",
            path: parent.to_path_buf(),
            source,
        })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("catalog.json");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut content =
        serde_json::to_vec_pretty(catalog).map_err(|source| CatalogError::InvalidData {
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
    content.push(b'\n');

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| CatalogError::Io {
            operation: "create temporary catalog",
            path: temporary.clone(),
            source,
        })?;
    file.write_all(&content)
        .await
        .map_err(|source| CatalogError::Io {
            operation: "write and sync temporary catalog",
            path: temporary.clone(),
            source,
        })?;
    file.sync_all().await.map_err(|source| CatalogError::Io {
        operation: "write and sync temporary catalog",
        path: temporary.clone(),
        source,
    })?;
    Ok(temporary)
}

async fn remove_temporary(path: &Path) -> Result<(), CatalogError> {
    tokio::fs::remove_file(path)
        .await
        .map_err(|source| CatalogError::Io {
            operation: "remove temporary catalog",
            path: path.to_path_buf(),
            source,
        })
}

async fn sync_parent(path: &Path) -> Result<(), CatalogError> {
    let parent = path.parent().ok_or_else(|| CatalogError::InvalidData {
        path: path.to_path_buf(),
        reason: "catalog path has no parent directory".to_owned(),
    })?;
    let directory = tokio::fs::File::open(parent)
        .await
        .map_err(|source| CatalogError::Io {
            operation: "open catalog directory for sync",
            path: parent.to_path_buf(),
            source,
        })?;
    directory
        .sync_all()
        .await
        .map_err(|source| CatalogError::Io {
            operation: "sync catalog directory",
            path: parent.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    struct Fixture {
        _temp: TempDir,
        data: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("create fixture directory");
            let data = temp.path().join("data");
            std::fs::create_dir(&data).expect("create data directory");
            Self { _temp: temp, data }
        }

        fn paths(&self) -> CatalogPaths {
            CatalogPaths::for_data_dir(&self.data)
        }

        fn project(&self, name: &str) -> PathBuf {
            let path = self._temp.path().join(name);
            std::fs::create_dir(&path).expect("create project directory");
            path
        }

        fn git_project(&self, name: &str) -> PathBuf {
            let path = self.project(name);
            git(&path, &["init", "-b", "main"]);
            git(&path, &["config", "user.name", "locald tests"]);
            git(&path, &["config", "user.email", "locald@example.test"]);
            std::fs::write(path.join("locald.toml"), "[project]\nname = \"test\"\n")
                .expect("write project config");
            git(&path, &["add", "locald.toml"]);
            git(&path, &["commit", "-m", "initial"]);
            path
        }

        fn linked_worktree(&self, repository: &Path, name: &str, branch: &str) -> PathBuf {
            let path = self._temp.path().join(name);
            git(
                repository,
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    path.to_str().expect("UTF-8 worktree path"),
                ],
            );
            path
        }
    }

    fn catalog_fixture_bytes(
        catalog: &ProjectCatalog,
        version: u32,
        include_domain_index: bool,
    ) -> Vec<u8> {
        let mut value = serde_json::to_value(catalog).expect("serialize catalog fixture");
        value["version"] = Value::from(version);
        if !include_domain_index {
            value
                .as_object_mut()
                .expect("catalog fixture object")
                .remove("domain_index");
        }
        let mut bytes = serde_json::to_vec_pretty(&value).expect("encode catalog fixture");
        bytes.push(b'\n');
        bytes
    }

    fn catalog_fixture_version(bytes: &[u8]) -> u64 {
        serde_json::from_slice::<Value>(bytes)
            .expect("parse catalog fixture")
            .get("version")
            .and_then(Value::as_u64)
            .expect("unsigned catalog fixture version")
    }

    #[tokio::test]
    async fn empty_initialization_writes_a_deterministic_versioned_catalog() {
        let fixture = Fixture::new();
        let paths = fixture.paths();

        let first = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let first_bytes = tokio::fs::read(&paths.catalog)
            .await
            .expect("read first catalog");
        let second = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("reopen catalog");
        let second_bytes = tokio::fs::read(&paths.catalog)
            .await
            .expect("read second catalog");

        assert_eq!(first, second);
        assert_eq!(first_bytes, second_bytes);
        assert_eq!(
            catalog_fixture_version(&first_bytes),
            u64::from(CATALOG_VERSION)
        );
    }

    #[tokio::test]
    async fn lifecycle_recovery_builds_legacy_catalog_without_publishing_it() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let missing = fixture._temp.path().join("legacy-only-project");
        let registry = serde_json::json!({
            "projects": {
                missing.to_string_lossy().as_ref(): {
                    "path": missing,
                    "name": "legacy-only",
                    "pinned": true,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize legacy registry"),
        )
        .await
        .expect("write legacy registry");

        let candidate = ProjectCatalog::load_from_paths_for_lifecycle_recovery(paths.clone(), true)
            .await
            .expect("build read-only legacy candidate");

        assert_eq!(candidate.storage_path(), paths.catalog);
        assert_eq!(candidate.unresolved_legacy.len(), 1);
        assert!(!paths.catalog.exists());
    }

    #[tokio::test]
    async fn lifecycle_recovery_does_not_reimport_legacy_state_when_catalog_is_missing() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        tokio::fs::write(&paths.legacy_registry, br#"{"projects":{}}"#)
            .await
            .expect("write stale legacy registry");

        let error = ProjectCatalog::load_from_paths_for_lifecycle_recovery(paths.clone(), false)
            .await
            .expect_err("v2 recovery must require its authoritative catalog");

        assert!(matches!(error, CatalogError::InvalidData { .. }));
        assert!(error.to_string().contains("lifecycle-v2"));
        assert!(!paths.catalog.exists());
    }

    #[tokio::test]
    async fn legacy_locator_sources_are_unioned_without_parsing_attachment_variants() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let missing = fixture._temp.path().join("missing");
        let missing_text = missing.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                missing_text.as_ref(): {
                    "path": missing,
                    "name": "legacy",
                    "pinned": true,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        let attachments = serde_json::json!({
            "attachments": {
                missing_text.as_ref(): [{
                    "project_path": missing,
                    "source": { "Runtime": { "token": "opaque" } },
                    "created_at": UNIX_EPOCH
                }]
            },
            "manually_stopped": [missing]
        });
        let runtime = serde_json::json!({
            "services": [{ "path": missing }]
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");
        tokio::fs::write(
            &paths.legacy_attachments,
            serde_json::to_vec_pretty(&attachments).expect("serialize attachments"),
        )
        .await
        .expect("write attachments");
        tokio::fs::write(
            &paths.legacy_runtime_state,
            serde_json::to_vec_pretty(&runtime).expect("serialize runtime"),
        )
        .await
        .expect("write runtime");

        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("import locator evidence");
        let record = catalog
            .unresolved_legacy
            .get(&crate::normalize_project_locator(&missing).expect("normalize missing locator"))
            .expect("missing path is unresolved");

        assert_eq!(record.display_name.as_deref(), Some("legacy"));
        assert!(record.pinned);
        assert_eq!(
            record.sources,
            BTreeSet::from([
                LegacyLocatorSource::Registry,
                LegacyLocatorSource::Attachment,
                LegacyLocatorSource::ManualStop,
                LegacyLocatorSource::RuntimeState,
            ])
        );
        assert!(catalog.instances.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_legacy_symlink_remains_unresolved_evidence() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let missing_target = fixture._temp.path().join("missing-target");
        let dangling = fixture._temp.path().join("dangling-project");
        std::os::unix::fs::symlink(&missing_target, &dangling).expect("create dangling symlink");
        let dangling_text = dangling.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                dangling_text.as_ref(): {
                    "path": dangling,
                    "name": "dangling",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");

        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("preserve dangling legacy evidence");

        assert!(catalog.unresolved_legacy.contains_key(
            &crate::normalize_project_locator(&dangling).expect("normalize dangling locator")
        ));
        assert!(catalog.instances.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn current_v4_unresolved_alias_mutations_match_the_normalized_locator() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let real_parent = fixture._temp.path().join("real-parent");
        let alias_parent = fixture._temp.path().join("alias-parent");
        std::fs::create_dir(&real_parent).expect("create real locator parent");
        std::os::unix::fs::symlink(&real_parent, &alias_parent).expect("create locator alias");
        let raw_alias = alias_parent.join("missing-project");
        let normalized = crate::normalize_project_locator(&raw_alias)
            .expect("normalize missing locator through alias");
        assert_ne!(raw_alias, normalized);

        let mut catalog = ProjectCatalog::with_path(paths.catalog.clone());
        catalog.unresolved_legacy.insert(
            raw_alias.clone(),
            UnresolvedLegacyProject {
                path: raw_alias.clone(),
                display_name: Some("aliased legacy project".to_owned()),
                pinned: false,
                last_seen: Some(UNIX_EPOCH),
                sources: BTreeSet::from([LegacyLocatorSource::Registry]),
            },
        );
        catalog.save().await.expect("persist current v4 catalog");
        let before_reload = tokio::fs::read(&paths.catalog)
            .await
            .expect("read current v4 catalog before reload");

        let mut reopened = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("reload current v4 catalog");

        assert_eq!(
            tokio::fs::read(&paths.catalog)
                .await
                .expect("read current v4 catalog after reload"),
            before_reload
        );
        assert!(reopened.unresolved_legacy.contains_key(&raw_alias));
        assert!(!reopened.unresolved_legacy.contains_key(&normalized));

        assert!(reopened.pin_project(&normalized));
        assert!(reopened.unresolved_legacy[&raw_alias].pinned);
        assert!(reopened.unpin_project(&normalized));
        assert!(!reopened.unresolved_legacy[&raw_alias].pinned);
        assert!(
            reopened
                .unregister_project(&normalized)
                .expect("unregister normalized unresolved alias")
        );
        assert!(!reopened.unresolved_legacy.contains_key(&raw_alias));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_locator_resolution_errors_remain_unresolved_evidence() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let looped = fixture._temp.path().join("looped-project");
        std::os::unix::fs::symlink(&looped, &looped).expect("create symlink loop");
        let resolution_error = tokio::fs::canonicalize(&looped)
            .await
            .expect_err("symlink loop must not resolve");
        assert_ne!(resolution_error.kind(), io::ErrorKind::NotFound);

        let looped_text = looped.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                looped_text.as_ref(): {
                    "path": looped,
                    "name": "looped",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");

        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("preserve unresolvable legacy evidence");

        assert!(catalog.unresolved_legacy.contains_key(&looped));
        assert!(catalog.instances.is_empty());
    }

    #[tokio::test]
    async fn unavailable_git_identity_during_legacy_import_remains_unresolved_evidence() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let root = fixture.git_project("legacy-linked-root");
        let linked = fixture._temp.path().join("legacy-linked-broken");
        let linked_text = linked.to_str().expect("UTF-8 linked worktree path");
        git(
            &root,
            &["worktree", "add", "-b", "legacy-linked", linked_text],
        );
        let discovery = ProjectCatalog::discover(linked.clone())
            .await
            .expect("discover linked worktree before breaking it");
        let resolved = match discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved,
            ProjectDiscovery::NonGit { .. } => panic!("expected Git discovery"),
        };
        let linked_text = linked.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                linked_text.as_ref(): {
                    "path": linked,
                    "name": "legacy-linked",
                    "pinned": true,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");
        let unavailable_admin = fixture._temp.path().join("unavailable-legacy-admin");
        std::fs::rename(&resolved.worktree_git_dir, &unavailable_admin)
            .expect("make linked Git metadata unavailable");

        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("preserve unavailable Git identity as legacy evidence");
        let record = catalog
            .unresolved_legacy
            .get(&crate::normalize_project_locator(&linked).expect("normalize linked locator"))
            .expect("unavailable linked worktree remains unresolved");

        assert_eq!(record.display_name.as_deref(), Some("legacy-linked"));
        assert!(record.pinned);
        assert_eq!(
            record.sources,
            BTreeSet::from([LegacyLocatorSource::Registry])
        );
        assert!(catalog.instances.is_empty());
        let error = ProjectCatalog::discover(linked)
            .await
            .expect_err("direct discovery still reports the broken worktree");
        assert!(matches!(
            error,
            CatalogError::Identity(IdentityError::BrokenWorktree { .. })
        ));
        assert!(error.to_string().contains("git worktree repair"));
    }

    #[tokio::test]
    async fn invalid_marker_still_blocks_initial_legacy_import() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("invalid-legacy-marker");
        let resolved = resolve_git_project_identity(&project).expect("create identity markers");
        let marker = resolved.worktree_git_dir.join("locald/worktree-id");
        std::fs::write(&marker, "v1 definitely-not-a-uuid\n").expect("corrupt worktree marker");
        let project_text = project.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                project_text.as_ref(): {
                    "path": project,
                    "name": "invalid-marker",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("invalid marker must block initial import");

        assert!(matches!(
            error,
            CatalogError::Identity(IdentityError::InvalidMarker { .. })
        ));
        assert!(!paths.catalog.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clean_preserves_uninspectable_legacy_evidence_and_forgets_missing_peer() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let looped = fixture._temp.path().join("looped-during-clean");
        let missing = fixture._temp.path().join("missing-during-clean");
        std::os::unix::fs::symlink(&looped, &looped).expect("create symlink loop");
        let looped_text = looped.to_string_lossy();
        let missing_text = missing.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                looped_text.as_ref(): {
                    "path": looped,
                    "name": "looped",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                },
                missing_text.as_ref(): {
                    "path": missing,
                    "name": "missing",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("import unresolved legacy evidence");

        assert_eq!(
            catalog
                .prune_missing_projects()
                .expect("clean other records while preserving uninspectable evidence"),
            1
        );
        assert!(catalog.unresolved_legacy.contains_key(&looped));
        assert!(!catalog.unresolved_legacy.contains_key(&missing));
    }

    #[tokio::test]
    async fn present_git_legacy_path_receives_its_git_backed_identity() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("git-project");
        let project_text = project.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                project_text.as_ref(): {
                    "path": project,
                    "name": "git-project",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");

        let expected = resolve_git_project_identity(&project).expect("resolve expected identity");
        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("import Git project");

        assert!(
            catalog
                .instances
                .contains_key(&expected.identity.project_instance_id)
        );
        assert_eq!(
            catalog
                .legacy_paths
                .get(&crate::normalize_project_locator(&project).expect("normalize Git locator")),
            Some(&expected.identity.project_instance_id)
        );
    }

    #[tokio::test]
    async fn present_non_git_project_keeps_its_random_identity_after_reopen() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.project("plain-project");
        let project_text = project.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                project_text.as_ref(): {
                    "path": project,
                    "name": "plain",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");

        let first = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("import non-Git project");
        let first_id = first.instances.keys().next().copied().expect("instance ID");
        let second = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen non-Git project");

        assert!(second.instances.contains_key(&first_id));
        assert!(matches!(
            &second.instances[&first_id].origin,
            ProjectInstanceOrigin::NonGit
        ));
    }

    #[tokio::test]
    async fn reappearing_non_git_project_reuses_its_identity_and_metadata() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.project("returning-plain-project");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover non-Git project");
        let instance_id = catalog
            .register_project_and_save(discovery, Some("plain".to_owned()))
            .await
            .expect("register non-Git project");
        let project_id = catalog.instances[&instance_id].project_id;
        let record = catalog
            .instances
            .get_mut(&instance_id)
            .expect("non-Git instance");
        record.pinned = true;
        record.domain_slug = Some("plain".to_owned());
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::legacy(
                    "plain.localhost".parse().expect("valid domain"),
                    instance_id,
                )],
            )
            .expect("record domain claims");
        catalog.save().await.expect("save non-Git metadata");

        std::fs::remove_dir_all(&project).expect("remove non-Git project");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("persist missing non-Git project");
        assert_eq!(
            catalog.instances[&instance_id].presence,
            CatalogPresence::Missing
        );
        std::fs::create_dir(&project).expect("restore non-Git project");
        let discovery = ProjectCatalog::discover(project.clone())
            .await
            .expect("rediscover non-Git project");
        let rediscovered_id = catalog
            .register_project_and_save(discovery, Some("plain again".to_owned()))
            .await
            .expect("reconcile restored non-Git project");
        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen restored non-Git project");

        assert_eq!(rediscovered_id, instance_id);
        assert_eq!(catalog.instances.len(), 1);
        assert_eq!(catalog.projects.len(), 1);
        let record = &catalog.instances[&instance_id];
        assert_eq!(record.project_id, project_id);
        assert_eq!(record.presence, CatalogPresence::Active);
        assert_eq!(
            record.current_path,
            Some(std::fs::canonicalize(&project).expect("canonical restored project"))
        );
        assert!(record.pinned);
        assert_eq!(record.domain_slug.as_deref(), Some("plain"));
        assert!(record.domain_claims.contains("plain.localhost"));
    }

    #[tokio::test]
    async fn malformed_existing_catalog_blocks_without_reimport_or_rewrite() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let malformed = b"{\"version\":3,\"instances\":";
        tokio::fs::write(&paths.catalog, malformed)
            .await
            .expect("write malformed catalog");
        tokio::fs::write(&paths.legacy_registry, b"{\"projects\":{}}")
            .await
            .expect("write legacy registry");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("malformed catalog must fail");

        assert!(matches!(error, CatalogError::InvalidData { .. }));
        assert_eq!(
            tokio::fs::read(paths.catalog)
                .await
                .expect("read malformed catalog"),
            malformed
        );
    }

    #[tokio::test]
    async fn unsupported_catalog_version_is_not_replaced() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let unsupported = b"{\"version\":99}";
        tokio::fs::write(&paths.catalog, unsupported)
            .await
            .expect("write unsupported catalog");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("unsupported catalog must fail");

        assert!(matches!(error, CatalogError::UnsupportedVersion { .. }));
        assert_eq!(
            tokio::fs::read(paths.catalog)
                .await
                .expect("read unsupported catalog"),
            unsupported
        );
    }

    #[tokio::test]
    async fn registration_reconciles_a_moved_git_project_across_reopen() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let original = fixture.git_project("original");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let first = ProjectCatalog::discover(original.clone())
            .await
            .expect("discover original");
        let instance_id = catalog
            .register_project_and_save(first, Some("project".to_owned()))
            .await
            .expect("register original");
        let canonical_original = std::fs::canonicalize(&original).expect("canonical original path");
        let moved = fixture._temp.path().join("moved");
        std::fs::rename(&original, &moved).expect("move repository");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen catalog after move");
        let second = ProjectCatalog::discover(moved.clone())
            .await
            .expect("discover moved");

        let moved_id = catalog
            .register_project(second, Some("project".to_owned()))
            .expect("reconcile moved project");

        assert_eq!(instance_id, moved_id);
        let canonical_moved = std::fs::canonicalize(&moved).expect("canonical moved path");
        assert_eq!(
            catalog.instances[&instance_id].current_path,
            Some(canonical_moved)
        );
        assert_eq!(
            catalog.instances[&instance_id].last_known_path,
            canonical_original
        );
    }

    #[tokio::test]
    async fn clean_reactivates_a_restored_git_project_before_pruning() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("restored-before-clean");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover Git project");
        let identity = match &discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.identity,
            ProjectDiscovery::NonGit { .. } => panic!("expected Git discovery"),
        };
        catalog
            .register_project_and_save(discovery, Some("restored".to_owned()))
            .await
            .expect("register Git project");
        let record = catalog
            .instances
            .get_mut(&identity.project_instance_id)
            .expect("Git instance");
        record.domain_slug = Some("restored".to_owned());
        catalog
            .replace_domain_claims(
                identity.project_instance_id,
                [DomainClaim::legacy(
                    "restored.localhost".parse().expect("valid domain"),
                    identity.project_instance_id,
                )],
            )
            .expect("record domain claims");
        catalog.save().await.expect("save Git project metadata");

        let parked = fixture._temp.path().join("temporarily-parked");
        std::fs::rename(&project, &parked).expect("temporarily move Git project");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("persist missing Git project");
        assert_eq!(
            catalog.repositories[&identity.repository_id].presence,
            CatalogPresence::Missing
        );
        assert_eq!(
            catalog.worktrees[&identity.worktree_id].presence,
            CatalogPresence::Missing
        );
        assert_eq!(
            catalog.instances[&identity.project_instance_id].presence,
            CatalogPresence::Missing
        );
        std::fs::rename(&parked, &project).expect("restore Git project");

        assert_eq!(
            catalog
                .prune_missing_projects()
                .expect("clean restored Git project"),
            0
        );
        catalog.save().await.expect("save restored Git project");
        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen restored Git project");
        assert_eq!(
            catalog.repositories[&identity.repository_id].presence,
            CatalogPresence::Active
        );
        assert_eq!(
            catalog.worktrees[&identity.worktree_id].presence,
            CatalogPresence::Active
        );
        let record = &catalog.instances[&identity.project_instance_id];
        assert_eq!(record.presence, CatalogPresence::Active);
        assert_eq!(
            record.current_path,
            Some(std::fs::canonicalize(&project).expect("canonical restored project"))
        );
        assert_eq!(record.domain_slug.as_deref(), Some("restored"));
        assert!(record.domain_claims.contains("restored.localhost"));
        assert_eq!(
            catalog.instance_for_path(&project),
            Some(identity.project_instance_id)
        );
    }

    #[tokio::test]
    async fn restored_historical_git_identity_reclaims_its_compatibility_alias() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("historical-return");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let original_discovery = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover original checkout");
        let original_identity = match &original_discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.identity,
            ProjectDiscovery::NonGit { .. } => panic!("expected Git discovery"),
        };
        catalog
            .register_project_and_save(original_discovery, Some("original".to_owned()))
            .await
            .expect("register original checkout");
        let original_record = catalog
            .instances
            .get_mut(&original_identity.project_instance_id)
            .expect("original instance");
        original_record.domain_slug = Some("historical".to_owned());
        catalog
            .replace_domain_claims(
                original_identity.project_instance_id,
                [DomainClaim::legacy(
                    "historical.localhost".parse().expect("valid domain"),
                    original_identity.project_instance_id,
                )],
            )
            .expect("record domain claims");
        catalog.save().await.expect("save original metadata");

        let parked = fixture._temp.path().join("parked-original");
        std::fs::rename(&project, &parked).expect("park original checkout");
        let replacement = fixture.git_project("historical-return");
        let replacement_discovery = ProjectCatalog::discover(replacement.clone())
            .await
            .expect("discover replacement checkout");
        let replacement_identity = match &replacement_discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.identity,
            ProjectDiscovery::NonGit { .. } => panic!("expected Git discovery"),
        };
        catalog
            .register_project_and_save(replacement_discovery, Some("replacement".to_owned()))
            .await
            .expect("register replacement checkout");
        let canonical = std::fs::canonicalize(&replacement).expect("canonical replacement path");
        assert_ne!(
            original_identity.project_instance_id,
            replacement_identity.project_instance_id
        );
        assert_eq!(
            catalog.legacy_paths.get(&canonical),
            Some(&replacement_identity.project_instance_id)
        );

        std::fs::remove_dir_all(&replacement).expect("remove replacement checkout");
        std::fs::rename(&parked, &project).expect("restore original checkout");
        catalog
            .reconcile_missing()
            .expect("reconcile restored original checkout");
        catalog.save().await.expect("save restored catalog");
        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen restored catalog");

        assert_eq!(
            catalog.legacy_paths.get(&canonical),
            Some(&original_identity.project_instance_id)
        );
        assert_eq!(
            catalog.instance_for_path(&project),
            Some(original_identity.project_instance_id)
        );
        let original_record = &catalog.instances[&original_identity.project_instance_id];
        assert_eq!(original_record.presence, CatalogPresence::Active);
        assert_eq!(original_record.domain_slug.as_deref(), Some("historical"));
        assert!(
            original_record
                .domain_claims
                .contains("historical.localhost")
        );
        assert_eq!(
            catalog.instances[&replacement_identity.project_instance_id].presence,
            CatalogPresence::Missing
        );
    }

    #[tokio::test]
    async fn unreadable_existing_git_locator_is_reconciled_as_missing() {
        let fixture = Fixture::new();
        let root = fixture.git_project("linked-root");
        let linked = fixture._temp.path().join("linked-broken");
        let linked_text = linked.to_str().expect("UTF-8 linked worktree path");
        git(
            &root,
            &["worktree", "add", "-b", "linked-broken", linked_text],
        );
        let mut catalog = ProjectCatalog::load_from_paths(fixture.paths().clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(linked.clone())
            .await
            .expect("discover linked worktree");
        let resolved = match &discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.clone(),
            ProjectDiscovery::NonGit { .. } => panic!("expected Git discovery"),
        };
        catalog
            .register_project_and_save(discovery, Some("linked".to_owned()))
            .await
            .expect("register linked worktree");
        let unavailable_admin = fixture._temp.path().join("unavailable-worktree-admin");
        std::fs::rename(&resolved.worktree_git_dir, &unavailable_admin)
            .expect("make linked Git locator unreadable");

        let mut reopened = ProjectCatalog::load_from_paths(fixture.paths())
            .await
            .expect("reconcile unreadable Git locator");

        assert_eq!(
            reopened.repositories[&resolved.identity.repository_id].presence,
            CatalogPresence::Active
        );
        assert_eq!(
            reopened.worktrees[&resolved.identity.worktree_id].presence,
            CatalogPresence::Missing
        );
        assert!(
            reopened.worktrees[&resolved.identity.worktree_id]
                .current_path
                .is_none()
        );
        assert_eq!(
            reopened.worktrees[&resolved.identity.worktree_id].last_known_path,
            resolved.worktree_root
        );
        assert_eq!(
            reopened.instances[&resolved.identity.project_instance_id].presence,
            CatalogPresence::Missing
        );
        assert!(
            reopened.instances[&resolved.identity.project_instance_id]
                .current_path
                .is_none()
        );
        assert_eq!(
            reopened.instances[&resolved.identity.project_instance_id].last_known_path,
            resolved.project_root
        );
        let clean_error = reopened
            .prune_missing_projects()
            .expect_err("cleanup must retain an unreadable Git locator");
        assert!(matches!(
            clean_error,
            CatalogError::Identity(IdentityError::BrokenWorktree { .. })
        ));
        assert!(clean_error.to_string().contains("git worktree repair"));
        assert!(
            reopened
                .instances
                .contains_key(&resolved.identity.project_instance_id)
        );
        let error = ProjectCatalog::discover(linked)
            .await
            .expect_err("active discovery still reports the broken worktree");
        assert!(matches!(
            error,
            CatalogError::Identity(IdentityError::BrokenWorktree { .. })
        ));
        assert!(error.to_string().contains("git worktree repair"));
    }

    #[tokio::test]
    async fn malformed_git_marker_still_blocks_catalog_reconciliation() {
        let fixture = Fixture::new();
        let project = fixture.git_project("malformed-reconciliation-marker");
        let mut catalog = ProjectCatalog::load_from_paths(fixture.paths().clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(project)
            .await
            .expect("discover Git project");
        let resolved = match &discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.clone(),
            ProjectDiscovery::NonGit { .. } => panic!("expected Git discovery"),
        };
        catalog
            .register_project_and_save(discovery, Some("malformed".to_owned()))
            .await
            .expect("register Git project");
        let marker = resolved.worktree_git_dir.join("locald/worktree-id");
        let malformed = "v1 definitely-not-a-uuid\n";
        std::fs::write(&marker, malformed).expect("corrupt worktree marker");

        let error = ProjectCatalog::load_from_paths(fixture.paths())
            .await
            .expect_err("malformed marker must block reconciliation");

        assert!(matches!(
            error,
            CatalogError::Identity(IdentityError::InvalidMarker { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(marker).expect("read malformed marker"),
            malformed
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconciliation_treats_locator_io_as_missing() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let looped = fixture._temp.path().join("unavailable-repository-locator");
        symlink(&looped, &looped).expect("create self-referential locator");
        let expected = "4b032f75-5c3f-486a-9dc8-ae4501e12843"
            .parse()
            .expect("parse repository ID");

        assert!(
            !reconciliation_repository_locator_matches(&looped, expected)
                .expect("reconciliation should classify locator I/O as missing")
        );
        assert!(repository_locator_matches(&looped, expected).is_err());
    }

    #[tokio::test]
    async fn moved_git_project_accepts_a_different_checkout_at_its_former_path() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let original = fixture.git_project("original");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let first = ProjectCatalog::discover(original.clone())
            .await
            .expect("discover original");
        let original_id = catalog
            .register_project_and_save(first, Some("original".to_owned()))
            .await
            .expect("register original");
        let moved = fixture._temp.path().join("moved");
        std::fs::rename(&original, &moved).expect("move original checkout");
        let replacement = fixture.git_project("original");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen catalog with reused locator");

        let moved_discovery = ProjectCatalog::discover(moved.clone())
            .await
            .expect("discover moved checkout");
        let moved_id = catalog
            .register_project(moved_discovery, Some("original".to_owned()))
            .expect("reconcile moved checkout");
        let replacement_discovery = ProjectCatalog::discover(replacement.clone())
            .await
            .expect("discover replacement checkout");
        let replacement_id = catalog
            .register_project(replacement_discovery, Some("replacement".to_owned()))
            .expect("register replacement checkout");

        assert_eq!(moved_id, original_id);
        assert_ne!(replacement_id, original_id);
        assert_eq!(
            catalog.instances[&original_id].current_path,
            Some(std::fs::canonicalize(moved).expect("canonical moved path"))
        );
        assert_eq!(
            catalog.instances[&replacement_id].current_path,
            Some(std::fs::canonicalize(replacement).expect("canonical replacement path"))
        );
    }

    #[tokio::test]
    async fn reused_primary_path_is_marked_missing_before_rediscovery() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let original = fixture.git_project("primary");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(original.clone())
            .await
            .expect("discover original primary checkout");
        let original_identity = match &discovery {
            ProjectDiscovery::Git { resolved, .. } => resolved.identity,
            ProjectDiscovery::NonGit { .. } => panic!("expected Git discovery"),
        };
        catalog
            .register_project_and_save(discovery, Some("primary".to_owned()))
            .await
            .expect("register primary checkout");

        let moved = fixture._temp.path().join("moved-primary");
        std::fs::rename(&original, &moved).expect("move primary checkout");
        let original_text = original.to_str().expect("UTF-8 worktree path");
        git(
            &moved,
            &["worktree", "add", "-b", "replacement", original_text],
        );

        let mut reopened = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reconcile reused primary locator");

        assert_eq!(
            reopened.instances[&original_identity.project_instance_id].presence,
            CatalogPresence::Missing
        );
        assert_eq!(
            reopened.repositories[&original_identity.repository_id].presence,
            CatalogPresence::Missing
        );

        let moved_discovery = ProjectCatalog::discover(moved.clone())
            .await
            .expect("discover moved primary checkout");
        let moved_id = reopened
            .register_project(moved_discovery, Some("primary".to_owned()))
            .expect("reconcile moved primary checkout");

        assert_eq!(moved_id, original_identity.project_instance_id);
        assert_eq!(
            reopened.repositories[&original_identity.repository_id].display_name,
            Some("moved-primary".to_owned())
        );
    }

    #[tokio::test]
    async fn remove_and_recreate_at_one_path_allocates_a_new_instance() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("recreated");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("initialize catalog");
        let first = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover first checkout");
        let first_id = catalog
            .register_project(first, Some("first".to_owned()))
            .expect("register first checkout");

        std::fs::remove_dir_all(&project).expect("remove first checkout");
        let recreated = fixture.git_project("recreated");
        let second = ProjectCatalog::discover(recreated.clone())
            .await
            .expect("discover recreated checkout");
        let second_id = catalog
            .register_project(second, Some("second".to_owned()))
            .expect("register recreated checkout");

        assert_ne!(first_id, second_id);
        assert_eq!(
            catalog.instances[&first_id].presence,
            CatalogPresence::Missing
        );
        assert!(catalog.instances[&first_id].current_path.is_none());
        assert_eq!(
            catalog.instances[&second_id].presence,
            CatalogPresence::Active
        );
        assert_eq!(
            catalog.instances[&second_id].current_path,
            Some(std::fs::canonicalize(&recreated).expect("canonical recreated path"))
        );
        assert_eq!(
            catalog
                .project_entries_by_path()
                .get(&std::fs::canonicalize(recreated).expect("canonical projection path"))
                .and_then(|entry| entry.name.as_deref()),
            Some("second")
        );
    }

    #[tokio::test]
    async fn latest_missing_instance_owns_the_legacy_path_projection() {
        let fixture = Fixture::new();
        let project = fixture.git_project("missing-history");
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);
        let first_discovery = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover first checkout");
        let first_id = catalog
            .register_project(first_discovery, Some("first".to_owned()))
            .expect("register first checkout");

        std::fs::remove_dir_all(&project).expect("remove first checkout");
        let recreated = fixture.git_project("missing-history");
        let second_discovery = ProjectCatalog::discover(recreated.clone())
            .await
            .expect("discover recreated checkout");
        let second_id = catalog
            .register_project(second_discovery, Some("second".to_owned()))
            .expect("register recreated checkout");
        catalog
            .instances
            .get_mut(&first_id)
            .expect("first record")
            .last_seen = UNIX_EPOCH + std::time::Duration::from_secs(1);
        catalog
            .instances
            .get_mut(&second_id)
            .expect("second record")
            .last_seen = UNIX_EPOCH + std::time::Duration::from_secs(2);
        let canonical = std::fs::canonicalize(&recreated).expect("canonical recreated path");
        std::fs::remove_dir_all(&recreated).expect("remove recreated checkout");
        catalog
            .reconcile_missing()
            .expect("mark both instances missing");
        catalog.legacy_paths.remove(&canonical);

        let projected = catalog
            .get_project(&canonical)
            .expect("latest historical projection");
        assert_eq!(projected.name.as_deref(), Some("second"));
        assert_eq!(
            catalog
                .project_entries_by_path()
                .get(&canonical)
                .and_then(|entry| entry.name.as_deref()),
            Some("second")
        );
        assert!(catalog.pin_project(&canonical));
        assert!(!catalog.instances[&first_id].pinned);
        assert!(catalog.instances[&second_id].pinned);
    }

    #[tokio::test]
    async fn git_display_metadata_refreshes_without_changing_identity() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("metadata");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("initialize catalog");
        let worktree_id = resolve_git_project_identity(&project)
            .expect("resolve expected Git identity")
            .identity
            .worktree_id;
        let first = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover main branch");
        let instance_id = catalog
            .register_project(first, Some("metadata".to_owned()))
            .expect("register main branch");

        git(&project, &["checkout", "-b", "feature/catalog"]);
        let branch = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover feature branch");
        let branch_id = catalog
            .register_project(branch, Some("metadata".to_owned()))
            .expect("refresh branch metadata");
        assert_eq!(branch_id, instance_id);
        assert_eq!(
            catalog.worktrees[&worktree_id].branch.as_deref(),
            Some("feature/catalog")
        );

        git(&project, &["checkout", "--detach"]);
        let detached = ProjectCatalog::discover(project)
            .await
            .expect("discover detached head");
        let detached_id = catalog
            .register_project(detached, Some("metadata".to_owned()))
            .expect("refresh detached metadata");
        assert_eq!(detached_id, instance_id);
        assert_eq!(catalog.worktrees[&worktree_id].branch, None);
        assert!(catalog.worktrees[&worktree_id].head.is_some());
    }

    #[tokio::test]
    async fn linked_worktree_slug_is_allocated_once_from_mutable_hints() {
        let fixture = Fixture::new();
        let repository = fixture.git_project("repository");
        let linked =
            fixture.linked_worktree(&repository, "feature-worktree", "feature/checkout-flow");
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);

        let primary_discovery = ProjectCatalog::discover(repository.clone())
            .await
            .expect("discover primary checkout");
        assert!(!primary_discovery.is_linked_worktree());
        let primary_id = catalog
            .register_project(primary_discovery, Some("primary".to_owned()))
            .expect("register primary checkout");
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(primary_id, false, None)
                .expect("preserve primary base domain"),
            None
        );

        let linked_discovery = ProjectCatalog::discover(linked.clone())
            .await
            .expect("discover linked worktree");
        assert!(linked_discovery.is_linked_worktree());
        let linked_id = catalog
            .register_project(linked_discovery, Some("linked".to_owned()))
            .expect("register linked worktree");
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(linked_id, true, None)
                .expect("allocate linked slug")
                .as_deref(),
            Some("checkout-flow")
        );

        git(&linked, &["checkout", "-b", "feature/renamed"]);
        let renamed = ProjectCatalog::discover(linked.clone())
            .await
            .expect("rediscover renamed branch");
        assert_eq!(
            catalog
                .register_project(renamed, Some("linked".to_owned()))
                .expect("refresh renamed branch"),
            linked_id
        );
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(linked_id, true, None)
                .expect("preserve slug after branch change")
                .as_deref(),
            Some("checkout-flow")
        );

        git(&linked, &["checkout", "--detach"]);
        let detached = ProjectCatalog::discover(linked.clone())
            .await
            .expect("rediscover detached worktree");
        catalog
            .register_project(detached, Some("linked".to_owned()))
            .expect("refresh detached worktree");

        let moved = fixture._temp.path().join("moved-worktree");
        git(
            &repository,
            &[
                "worktree",
                "move",
                linked.to_str().expect("UTF-8 source worktree"),
                moved.to_str().expect("UTF-8 moved worktree"),
            ],
        );
        let moved_discovery = ProjectCatalog::discover(moved)
            .await
            .expect("rediscover moved worktree");
        assert_eq!(
            catalog
                .register_project(moved_discovery, Some("linked".to_owned()))
                .expect("refresh moved worktree"),
            linked_id
        );
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(linked_id, true, Some("later-task-label"))
                .expect("preserve slug after detach and move")
                .as_deref(),
            Some("checkout-flow")
        );

        catalog.save().await.expect("persist stable slug");
        let reopened = ProjectCatalog::load_from_paths(fixture.paths())
            .await
            .expect("reopen stable slug");
        assert_eq!(
            reopened.instances[&linked_id].domain_slug.as_deref(),
            Some("checkout-flow")
        );
    }

    #[tokio::test]
    async fn slug_collisions_reserve_missing_instances_until_forget() {
        let fixture = Fixture::new();
        let repository = fixture.git_project("repository");
        let first_path =
            fixture.linked_worktree(&repository, "first-worktree", "feature/turn-trace");
        let second_path =
            fixture.linked_worktree(&repository, "second-worktree", "bugfix/turn-trace");
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);

        let first = catalog
            .register_project(
                ProjectCatalog::discover(first_path.clone())
                    .await
                    .expect("discover first worktree"),
                Some("first".to_owned()),
            )
            .expect("register first worktree");
        let second = catalog
            .register_project(
                ProjectCatalog::discover(second_path)
                    .await
                    .expect("discover second worktree"),
                Some("second".to_owned()),
            )
            .expect("register second worktree");
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(first, true, None)
                .expect("allocate first slug")
                .as_deref(),
            Some("turn-trace")
        );
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(second, true, None)
                .expect("allocate collision suffix")
                .as_deref(),
            Some("turn-trace-2")
        );

        git(
            &repository,
            &[
                "worktree",
                "remove",
                first_path.to_str().expect("UTF-8 first worktree"),
            ],
        );
        catalog
            .reconcile_missing()
            .expect("mark removed worktree missing");
        assert_eq!(catalog.instances[&first].presence, CatalogPresence::Missing);

        let third_path = fixture.linked_worktree(&repository, "third-worktree", "docs/turn-trace");
        let third = catalog
            .register_project(
                ProjectCatalog::discover(third_path)
                    .await
                    .expect("discover third worktree"),
                Some("third".to_owned()),
            )
            .expect("register third worktree");
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(third, true, None)
                .expect("reserve missing slug")
                .as_deref(),
            Some("turn-trace-3")
        );

        catalog
            .remove_instance(first)
            .expect("forget missing project instance");
        let fourth_path =
            fixture.linked_worktree(&repository, "fourth-worktree", "test/turn-trace");
        let fourth = catalog
            .register_project(
                ProjectCatalog::discover(fourth_path)
                    .await
                    .expect("discover fourth worktree"),
                Some("fourth".to_owned()),
            )
            .expect("register fourth worktree");
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(fourth, true, None)
                .expect("reuse explicitly released slug")
                .as_deref(),
            Some("turn-trace")
        );
    }

    #[tokio::test]
    async fn detached_worktrees_use_trusted_then_path_allocation_hints() {
        let fixture = Fixture::new();
        let repository = fixture.git_project("repository");
        let path_hint = fixture.linked_worktree(&repository, "detached-path-hint", "scratch/first");
        let trusted_hint =
            fixture.linked_worktree(&repository, "detached-trusted-hint", "scratch/second");
        git(&path_hint, &["checkout", "--detach"]);
        git(&trusted_hint, &["checkout", "--detach"]);
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);

        let path_instance = catalog
            .register_project(
                ProjectCatalog::discover(path_hint)
                    .await
                    .expect("discover path-hinted detached worktree"),
                Some("path hinted".to_owned()),
            )
            .expect("register path-hinted detached worktree");
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(path_instance, true, None)
                .expect("allocate from path hint")
                .as_deref(),
            Some("detached-path-hint")
        );

        let trusted_instance = catalog
            .register_project(
                ProjectCatalog::discover(trusted_hint)
                    .await
                    .expect("discover trusted detached worktree"),
                Some("trusted".to_owned()),
            )
            .expect("register trusted detached worktree");
        assert_eq!(
            catalog
                .ensure_worktree_domain_slug(trusted_instance, true, Some("Chat: Turn Trace"))
                .expect("allocate from trusted hint")
                .as_deref(),
            Some("chat-turn-trace")
        );
    }

    #[test]
    fn collision_suffixes_keep_the_slug_within_one_dns_label() {
        let base = "a".repeat(63);
        let reserved = BTreeSet::from([base.as_str()]);

        let allocated = allocate_unique_domain_slug(&base, &reserved);

        assert_eq!(allocated.len(), 63);
        assert!(allocated.ends_with("-2"));
    }

    #[tokio::test]
    async fn existing_catalog_does_not_reimport_later_legacy_changes() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let missing = fixture._temp.path().join("later-legacy");
        let missing_text = missing.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                missing_text.as_ref(): {
                    "path": missing,
                    "name": "later",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write later legacy state");

        let reopened = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen authoritative catalog");

        assert!(reopened.unresolved_legacy.is_empty());
        assert!(reopened.instances.is_empty());
    }

    #[tokio::test]
    async fn mismatched_legacy_registry_path_blocks_catalog_creation() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let key = fixture._temp.path().join("key");
        let recorded = fixture._temp.path().join("recorded");
        let key_text = key.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                key_text.as_ref(): {
                    "path": recorded,
                    "name": "mismatch",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write mismatched registry");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("mismatched registry must fail");

        assert!(matches!(error, CatalogError::InvalidData { .. }));
        assert!(!paths.catalog.exists());
    }

    #[tokio::test]
    async fn malformed_legacy_registry_fields_block_catalog_creation() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let missing = fixture._temp.path().join("malformed");
        let missing_text = missing.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                missing_text.as_ref(): {
                    "path": missing,
                    "name": "malformed",
                    "pinned": "yes",
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize malformed registry"),
        )
        .await
        .expect("write malformed registry");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("malformed typed registry field must fail");

        assert!(matches!(error, CatalogError::InvalidData { .. }));
        assert!(!paths.catalog.exists());
    }

    #[tokio::test]
    async fn malformed_legacy_compatibility_json_does_not_block_catalog_creation() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let registry_path = fixture._temp.path().join("registry-project");
        let registry_path_text = registry_path.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                registry_path_text.as_ref(): {
                    "path": registry_path,
                    "name": "registry project",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        let malformed_attachments = b"{\"attachments\":";
        let malformed_runtime = b"{\"services\":[";
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");
        tokio::fs::write(&paths.legacy_attachments, malformed_attachments)
            .await
            .expect("write malformed attachments");
        tokio::fs::write(&paths.legacy_runtime_state, malformed_runtime)
            .await
            .expect("write malformed runtime state");

        let catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("create catalog from valid registry evidence");

        let record = catalog
            .unresolved_legacy
            .get(
                &crate::normalize_project_locator(&registry_path)
                    .expect("normalize registry locator"),
            )
            .expect("preserve registry evidence");
        assert_eq!(
            record.sources,
            BTreeSet::from([LegacyLocatorSource::Registry])
        );
        assert_eq!(
            tokio::fs::read(&paths.legacy_attachments)
                .await
                .expect("read malformed attachments"),
            malformed_attachments
        );
        assert_eq!(
            tokio::fs::read(&paths.legacy_runtime_state)
                .await
                .expect("read malformed runtime state"),
            malformed_runtime
        );
    }

    #[tokio::test]
    async fn invalid_legacy_compatibility_schema_is_discarded_atomically() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let attachment_path = fixture._temp.path().join("partial-attachment");
        let runtime_path = fixture._temp.path().join("partial-runtime");
        let attachment_path_text = attachment_path.to_string_lossy();
        let runtime_path_text = runtime_path.to_string_lossy();
        let attachments = serde_json::json!({
            "attachments": {
                attachment_path_text.as_ref(): []
            },
            "manually_stopped": "invalid"
        });
        let runtime = serde_json::json!({
            "services": [
                { "path": runtime_path_text.as_ref() },
                { "path": false }
            ]
        });
        tokio::fs::write(
            &paths.legacy_attachments,
            serde_json::to_vec_pretty(&attachments).expect("serialize attachments"),
        )
        .await
        .expect("write attachments");
        tokio::fs::write(
            &paths.legacy_runtime_state,
            serde_json::to_vec_pretty(&runtime).expect("serialize runtime state"),
        )
        .await
        .expect("write runtime state");

        let catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("discard malformed compatibility evidence");

        assert!(!catalog.unresolved_legacy.contains_key(&attachment_path));
        assert!(!catalog.unresolved_legacy.contains_key(&runtime_path));
    }

    #[tokio::test]
    async fn clean_forgets_unpinned_missing_instances_and_preserves_project_data() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let unpinned = fixture.project("unpinned");
        let pinned = fixture.project("pinned");
        let reappeared = fixture._temp.path().join("reappeared");
        let unpinned_text = unpinned.to_string_lossy();
        let pinned_text = pinned.to_string_lossy();
        let reappeared_text = reappeared.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                unpinned_text.as_ref(): {
                    "path": unpinned,
                    "name": "unpinned",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                },
                pinned_text.as_ref(): {
                    "path": pinned,
                    "name": "pinned",
                    "pinned": true,
                    "last_seen": UNIX_EPOCH
                },
                reappeared_text.as_ref(): {
                    "path": reappeared,
                    "name": "reappeared",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");
        ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("import projects");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("open catalog for cleanup");
        let pinned_id = catalog
            .instance_for_path(&pinned)
            .expect("pinned instance identity");
        let project_data = fixture._temp.path().join("project-data");
        std::fs::create_dir(&project_data).expect("create project data");
        let sentinel = project_data.join("sentinel");
        std::fs::write(&sentinel, "preserved").expect("write resource sentinel");

        std::fs::remove_dir_all(&unpinned).expect("remove unpinned project");
        std::fs::remove_dir_all(&pinned).expect("remove pinned project");
        std::fs::create_dir(&reappeared).expect("restore unresolved project path");

        assert_eq!(
            catalog
                .prune_missing_projects()
                .expect("refresh and forget missing projects"),
            1
        );
        assert!(catalog.get_project(&unpinned).is_none());
        assert!(
            catalog
                .get_project(&pinned)
                .is_some_and(|entry| entry.pinned)
        );
        assert_eq!(
            catalog.instances[&pinned_id].presence,
            CatalogPresence::Missing
        );
        assert!(catalog.get_project(&reappeared).is_some());
        assert_eq!(
            std::fs::read_to_string(sentinel).expect("read resource sentinel"),
            "preserved"
        );
    }

    #[tokio::test]
    async fn failed_registration_save_keeps_in_memory_catalog_unchanged() {
        let fixture = Fixture::new();
        let project = fixture.project("transactional");
        let storage_path = fixture._temp.path().join("catalog-is-a-directory");
        std::fs::create_dir(&storage_path).expect("create blocking directory");
        let mut catalog = ProjectCatalog::with_path(storage_path);
        let before = catalog.clone();
        let discovery = ProjectCatalog::discover(project)
            .await
            .expect("discover project");

        catalog
            .register_project_and_save(discovery, Some("transactional".to_owned()))
            .await
            .expect_err("catalog replacement must fail");

        assert_eq!(catalog, before);
    }

    #[tokio::test]
    async fn first_publish_sync_failure_reports_published_state_and_cleans_temporary() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let catalog = ProjectCatalog::with_path(paths.catalog.clone());

        let error =
            publish_new_catalog_with_parent_sync(&catalog, &paths.catalog, |path| async move {
                Err(CatalogError::Io {
                    operation: "perform injected parent sync",
                    path,
                    source: io::Error::other("injected parent sync failure"),
                })
            })
            .await
            .expect_err("injected parent sync failure must be reported");

        let CatalogError::PublishedNotDurable { reason, .. } = error else {
            panic!("expected published-not-durable error");
        };
        assert!(reason.contains("injected parent sync failure"));
        let published = ProjectCatalog::load_existing(&paths.catalog)
            .await
            .expect("load newly published catalog");
        assert_eq!(published, catalog);

        let parent = paths.catalog.parent().expect("catalog parent");
        let temporary_files = std::fs::read_dir(parent)
            .expect("read catalog directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.starts_with(".catalog.json.") && name.ends_with(".tmp")
                })
            })
            .count();
        assert_eq!(temporary_files, 0);
    }

    #[tokio::test]
    async fn post_rename_sync_failure_keeps_memory_aligned_with_published_catalog() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let first_project = fixture.project("published-first");
        let second_project = fixture.project("published-second");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let first_discovery = ProjectCatalog::discover(first_project)
            .await
            .expect("discover first project");
        let mut candidate = catalog.clone();
        let first_id = candidate
            .register_project(first_discovery, Some("first".to_owned()))
            .expect("register first candidate");

        let error = catalog
            .commit_candidate_with_parent_sync(candidate.clone(), |path| async move {
                Err(CatalogError::Io {
                    operation: "perform injected parent sync",
                    path,
                    source: io::Error::other("injected parent sync failure"),
                })
            })
            .await
            .expect_err("injected parent sync failure must be reported");

        assert!(matches!(error, CatalogError::PublishedNotDurable { .. }));
        assert_eq!(catalog, candidate);
        let published = ProjectCatalog::load_existing(&paths.catalog)
            .await
            .expect("load atomically published catalog");
        assert_eq!(published, candidate);

        let second_discovery = ProjectCatalog::discover(second_project)
            .await
            .expect("discover second project");
        catalog
            .register_project_and_save(second_discovery, Some("second".to_owned()))
            .await
            .expect("commit a later catalog mutation");
        let final_catalog = ProjectCatalog::load_existing(&paths.catalog)
            .await
            .expect("load final catalog");
        assert!(final_catalog.instances.contains_key(&first_id));
        assert_eq!(final_catalog.instances.len(), 2);
    }

    #[tokio::test]
    async fn validation_rejects_git_project_ids_that_do_not_match_their_origin() {
        let fixture = Fixture::new();
        let project = fixture.git_project("derived-id");
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);
        let discovery = ProjectCatalog::discover(project)
            .await
            .expect("discover Git project");
        let instance_id = catalog
            .register_project(discovery, Some("derived".to_owned()))
            .expect("register Git project");
        let project_id = catalog.instances[&instance_id].project_id;
        let ProjectOrigin::Git {
            repository_relative_root,
            ..
        } = &mut catalog
            .projects
            .get_mut(&project_id)
            .expect("registered project record")
            .origin
        else {
            panic!("expected Git project origin");
        };
        *repository_relative_root = PathBuf::from("different-root");

        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::Invariant(_))
        ));
    }

    #[tokio::test]
    async fn validation_rejects_a_legacy_alias_for_another_active_instance() {
        let fixture = Fixture::new();
        let first_path = fixture.project("first-alias");
        let second_path = fixture.project("second-alias");
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);
        let first = ProjectCatalog::discover(first_path.clone())
            .await
            .expect("discover first project");
        catalog
            .register_project(first, Some("first".to_owned()))
            .expect("register first project");
        let second = ProjectCatalog::discover(second_path)
            .await
            .expect("discover second project");
        let second_id = catalog
            .register_project(second, Some("second".to_owned()))
            .expect("register second project");
        catalog.legacy_paths.insert(
            std::fs::canonicalize(first_path).expect("canonical first path"),
            second_id,
        );

        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::Invariant(_))
        ));
    }

    #[tokio::test]
    async fn copied_live_identity_is_rejected_without_catalog_mutation() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let original = fixture.git_project("original");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("initialize catalog");
        let first = ProjectCatalog::discover(original.clone())
            .await
            .expect("discover original");
        catalog
            .register_project(first, Some("project".to_owned()))
            .expect("register original");
        let before = catalog.clone();
        let copied = fixture._temp.path().join("copied");
        copy_directory(&original, &copied);
        let duplicate = ProjectCatalog::discover(copied)
            .await
            .expect("discover copied project");

        let error = catalog
            .register_project(duplicate, Some("copy".to_owned()))
            .expect_err("copied marker must fail");

        assert!(matches!(error, CatalogError::LiveIdentityConflict { .. }));
        assert_eq!(catalog, before);
    }

    #[tokio::test]
    async fn reappeared_last_known_copy_blocks_a_second_live_locator() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let original = fixture.git_project("last-known");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(original.clone())
            .await
            .expect("discover original");
        catalog
            .register_project_and_save(discovery, Some("project".to_owned()))
            .await
            .expect("register original");
        let copied = fixture._temp.path().join("copied-last-known");
        copy_directory(&original, &copied);
        std::fs::remove_dir_all(&original).expect("remove current locator");
        let mut catalog = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen missing catalog record");
        copy_directory(&copied, &original);
        let duplicate = ProjectCatalog::discover(copied)
            .await
            .expect("discover copied locator");
        let before = catalog.clone();

        let error = catalog
            .register_project(duplicate, Some("copy".to_owned()))
            .expect_err("reappeared last-known identity must block a second live copy");

        assert!(matches!(error, CatalogError::LiveIdentityConflict { .. }));
        assert_eq!(catalog, before);
    }

    #[tokio::test]
    async fn initial_catalog_publication_converges() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.project("concurrent-non-git");
        let project_text = project.to_string_lossy();
        let registry = serde_json::json!({
            "projects": {
                project_text.as_ref(): {
                    "path": project,
                    "name": "concurrent",
                    "pinned": false,
                    "last_seen": UNIX_EPOCH
                }
            }
        });
        tokio::fs::write(
            &paths.legacy_registry,
            serde_json::to_vec_pretty(&registry).expect("serialize registry"),
        )
        .await
        .expect("write registry");

        let (left, right) = tokio::join!(
            ProjectCatalog::load_from_paths(paths.clone()),
            ProjectCatalog::load_from_paths(paths.clone())
        );

        let left = left.expect("left catalog");
        let right = right.expect("right catalog");
        assert_eq!(left, right);
        assert_eq!(left.instances.len(), 1);
    }

    #[tokio::test]
    async fn legacy_inputs_remain_byte_identical_after_catalog_creation() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let registry = b"{\n  \"projects\": {}\n}\n";
        let attachments = b"{\n  \"attachments\": {}, \"manually_stopped\": []\n}\n";
        let runtime = b"{\n  \"services\": []\n}\n";
        tokio::fs::write(&paths.legacy_registry, registry)
            .await
            .expect("write registry");
        tokio::fs::write(&paths.legacy_attachments, attachments)
            .await
            .expect("write attachments");
        tokio::fs::write(&paths.legacy_runtime_state, runtime)
            .await
            .expect("write runtime");

        ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("create catalog");

        assert_eq!(
            tokio::fs::read(&paths.legacy_registry)
                .await
                .expect("read registry"),
            registry
        );
        assert_eq!(
            tokio::fs::read(&paths.legacy_attachments)
                .await
                .expect("read attachments"),
            attachments
        );
        assert_eq!(
            tokio::fs::read(&paths.legacy_runtime_state)
                .await
                .expect("read runtime"),
            runtime
        );
    }

    #[tokio::test]
    async fn v2_without_domain_index_migrates_identity_and_flat_claims_byte_stably() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("v2-flat-project");
        let mut source = ProjectCatalog::with_path(paths.catalog.clone());
        let discovery = ProjectCatalog::discover(project.clone())
            .await
            .expect("discover v2 Git project");
        let instance_id = source
            .register_project(discovery, Some("v2-flat".to_owned()))
            .expect("register v2 Git project");
        source.legacy_paths.insert(project, instance_id);
        {
            let record = source
                .instances
                .get_mut(&instance_id)
                .expect("registered v2 instance");
            record.pinned = true;
            record.domain_slug = Some("v2-flat".to_owned());
        }
        source
            .replace_domain_claims(
                instance_id,
                [DomainClaim::legacy(
                    "v2-flat.localhost".parse().expect("valid legacy domain"),
                    instance_id,
                )],
            )
            .expect("record v2 flat claim");
        let v2_bytes = catalog_fixture_bytes(&source, LEGACY_CATALOG_VERSION, false);
        tokio::fs::write(&paths.catalog, v2_bytes)
            .await
            .expect("write v2 catalog without domain index");

        let migrated = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("migrate v2 flat catalog");

        assert_eq!(migrated.repositories, source.repositories);
        assert_eq!(migrated.worktrees, source.worktrees);
        assert_eq!(migrated.projects, source.projects);
        assert_eq!(migrated.instances, source.instances);
        assert_eq!(migrated.legacy_paths, source.legacy_paths);
        assert_eq!(migrated.unresolved_legacy, source.unresolved_legacy);
        assert_eq!(
            migrated.domain_index().resolve("v2-flat.localhost"),
            Some(&DomainTarget::Service {
                project_instance_id: instance_id,
                service_name: None,
            })
        );

        let first_v4_bytes = tokio::fs::read(&paths.catalog)
            .await
            .expect("read migrated v4 catalog");
        assert_eq!(
            catalog_fixture_version(&first_v4_bytes),
            u64::from(CATALOG_VERSION)
        );
        assert!(
            serde_json::from_slice::<Value>(&first_v4_bytes)
                .expect("parse migrated v4 catalog")
                .get("domain_index")
                .is_some()
        );

        let reopened = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("reopen migrated v4 catalog");
        let second_v4_bytes = tokio::fs::read(&paths.catalog)
            .await
            .expect("read reopened v4 catalog");
        assert_eq!(reopened, migrated);
        assert_eq!(second_v4_bytes, first_v4_bytes);
    }

    #[tokio::test]
    async fn v3_migrates_with_an_empty_agent_binding_index() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.git_project("v3-agent-project");
        let mut source = ProjectCatalog::with_path(paths.catalog.clone());
        let instance_id = source
            .register_project(
                ProjectCatalog::discover(project.clone())
                    .await
                    .expect("discover v3 project"),
                Some("v3-agent".to_owned()),
            )
            .expect("register v3 project");
        let key = AgentConversationKey::digest("private conversation")
            .expect("digest private conversation");
        source
            .bind_agent_conversation(key.clone(), instance_id)
            .expect("bind current catalog");
        let mut value = serde_json::to_value(&source).expect("serialize v3 migration fixture");
        value["version"] = Value::from(PREVIOUS_CATALOG_VERSION);
        value
            .as_object_mut()
            .expect("catalog fixture object")
            .remove("agent_bindings");
        tokio::fs::write(
            &paths.catalog,
            serde_json::to_vec_pretty(&value).expect("encode v3 fixture"),
        )
        .await
        .expect("write v3 fixture");

        let migrated = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("migrate v3 catalog");

        assert_eq!(migrated.agent_binding(&key), None);
        assert_eq!(
            catalog_fixture_version(
                &tokio::fs::read(&paths.catalog)
                    .await
                    .expect("read migrated v4 catalog")
            ),
            u64::from(CATALOG_VERSION)
        );
        let stored: Value = serde_json::from_slice(
            &tokio::fs::read(&paths.catalog)
                .await
                .expect("read migrated catalog"),
        )
        .expect("parse migrated catalog");
        assert_eq!(stored["agent_bindings"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn agent_binding_is_durable_idempotent_and_released_by_forget() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let first_path = fixture.git_project("bound-project");
        let second_path = fixture.git_project("other-project");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let first = catalog
            .register_project(
                ProjectCatalog::discover(first_path.clone())
                    .await
                    .expect("discover first project"),
                Some("bound".to_owned()),
            )
            .expect("register first project");
        let second = catalog
            .register_project(
                ProjectCatalog::discover(second_path)
                    .await
                    .expect("discover second project"),
                Some("other".to_owned()),
            )
            .expect("register second project");
        let key =
            AgentConversationKey::digest("private conversation").expect("digest conversation");

        assert!(
            catalog
                .bind_agent_conversation(key.clone(), first)
                .expect("bind first instance")
        );
        assert!(
            !catalog
                .bind_agent_conversation(key.clone(), first)
                .expect("repeat binding")
        );
        assert!(matches!(
            catalog.bind_agent_conversation(key.clone(), second),
            Err(CatalogError::AgentBindingConflict)
        ));
        catalog.save().await.expect("persist binding");

        let mut reopened = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen binding");
        assert_eq!(reopened.agent_binding(&key), Some(first));
        assert!(
            reopened
                .unregister_project(&first_path)
                .expect("forget bound project")
        );
        assert_eq!(reopened.agent_binding(&key), None);
    }

    #[tokio::test]
    async fn missing_agent_bound_project_survives_pruning_until_explicit_forget() {
        let fixture = Fixture::new();
        let project = fixture.project("bound-missing-project");
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);
        let instance_id = catalog
            .register_project(
                ProjectCatalog::discover(project.clone())
                    .await
                    .expect("discover bound project"),
                Some("bound-missing-project".to_owned()),
            )
            .expect("register bound project");
        let conversation =
            AgentConversationKey::digest("active conversation").expect("digest conversation");
        catalog
            .bind_agent_conversation(conversation.clone(), instance_id)
            .expect("bind conversation");
        std::fs::remove_dir_all(&project).expect("remove bound project");

        assert_eq!(
            catalog
                .prune_missing_projects()
                .expect("prune missing projects"),
            0
        );
        assert_eq!(
            catalog.instances[&instance_id].presence,
            CatalogPresence::Missing
        );
        assert_eq!(
            catalog.agent_binding(&conversation),
            Some(instance_id),
            "automatic pruning must preserve first-project binding authority"
        );

        assert!(
            catalog
                .unregister_project(&project)
                .expect("explicitly forget bound project")
        );
        assert_eq!(catalog.agent_binding(&conversation), None);
        assert!(!catalog.instances.contains_key(&instance_id));
    }

    #[tokio::test]
    async fn v2_with_domain_index_preserves_exact_service_targets() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.project("v2-indexed-project");
        let mut source = ProjectCatalog::with_path(paths.catalog.clone());
        let discovery = ProjectCatalog::discover(project)
            .await
            .expect("discover v2 project");
        let instance_id = source
            .register_project(discovery, Some("v2-indexed".to_owned()))
            .expect("register v2 project");
        source
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "v2-indexed.localhost".parse().expect("valid exact domain"),
                    instance_id,
                    "v2-indexed:web".to_owned(),
                )],
            )
            .expect("record v2 exact target");
        let v2_bytes = catalog_fixture_bytes(&source, LEGACY_CATALOG_VERSION, true);
        tokio::fs::write(&paths.catalog, v2_bytes)
            .await
            .expect("write v2 catalog with domain index");

        let migrated = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("migrate indexed v2 catalog");

        assert_eq!(migrated.instances, source.instances);
        assert_eq!(
            migrated.domain_index().resolve("v2-indexed.localhost"),
            Some(&DomainTarget::Service {
                project_instance_id: instance_id,
                service_name: Some("v2-indexed:web".to_owned()),
            })
        );
        let stored = tokio::fs::read(&paths.catalog)
            .await
            .expect("read indexed v4 catalog");
        assert_eq!(catalog_fixture_version(&stored), u64::from(CATALOG_VERSION));
    }

    #[tokio::test]
    async fn invalid_v2_migration_preserves_original_bytes() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.project("invalid-v2-project");
        let mut source = ProjectCatalog::with_path(paths.catalog.clone());
        let instance_id = source
            .register_project(
                ProjectCatalog::discover(project)
                    .await
                    .expect("discover invalid v2 project"),
                Some("invalid-v2".to_owned()),
            )
            .expect("register invalid v2 project");
        source
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "indexed.localhost".parse().expect("valid indexed domain"),
                    instance_id,
                    "invalid-v2:web".to_owned(),
                )],
            )
            .expect("record indexed v2 claim");
        let mut value: Value = serde_json::from_slice(&catalog_fixture_bytes(
            &source,
            LEGACY_CATALOG_VERSION,
            true,
        ))
        .expect("parse invalid v2 fixture");
        value["instances"][instance_id.to_string()]["domain_claims"] =
            serde_json::json!(["different.localhost"]);
        let mut original = serde_json::to_vec_pretty(&value).expect("encode invalid v2 fixture");
        original.push(b'\n');
        tokio::fs::write(&paths.catalog, &original)
            .await
            .expect("write invalid v2 catalog");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("inconsistent v2 catalog must fail migration");

        assert!(matches!(error, CatalogError::Invariant(_)));
        assert_eq!(
            tokio::fs::read(&paths.catalog)
                .await
                .expect("read rejected v2 catalog"),
            original
        );
    }

    #[tokio::test]
    async fn native_v4_requires_domain_index_without_rewriting() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let catalog = ProjectCatalog::with_path(paths.catalog.clone());
        let original = catalog_fixture_bytes(&catalog, CATALOG_VERSION, false);
        tokio::fs::write(&paths.catalog, &original)
            .await
            .expect("write incomplete v4 catalog");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("v4 domain index is required");

        assert!(matches!(error, CatalogError::InvalidData { .. }));
        assert_eq!(
            tokio::fs::read(&paths.catalog)
                .await
                .expect("read rejected v4 catalog"),
            original
        );
    }

    #[tokio::test]
    async fn native_v4_requires_agent_bindings_without_rewriting() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let catalog = ProjectCatalog::with_path(paths.catalog.clone());
        let mut value = serde_json::to_value(catalog).expect("serialize current catalog");
        value
            .as_object_mut()
            .expect("catalog object")
            .remove("agent_bindings");
        let mut original = serde_json::to_vec_pretty(&value).expect("encode incomplete catalog");
        original.push(b'\n');
        tokio::fs::write(&paths.catalog, &original)
            .await
            .expect("write incomplete v4 catalog");

        let error = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect_err("v4 agent binding index is required");

        assert!(matches!(error, CatalogError::InvalidData { .. }));
        assert_eq!(
            tokio::fs::read(&paths.catalog)
                .await
                .expect("read rejected v4 catalog"),
            original
        );
    }

    #[tokio::test]
    async fn v2_migration_sync_failure_returns_published_v4() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let catalog = ProjectCatalog::with_path(paths.catalog.clone());
        let v2_bytes = catalog_fixture_bytes(&catalog, LEGACY_CATALOG_VERSION, false);
        tokio::fs::write(&paths.catalog, v2_bytes)
            .await
            .expect("write v2 migration fixture");

        let migrated =
            ProjectCatalog::load_existing_with_parent_sync(&paths.catalog, |path| async move {
                Err(CatalogError::Io {
                    operation: "perform injected migration parent sync",
                    path,
                    source: io::Error::other("injected migration parent sync failure"),
                })
            })
            .await
            .expect("post-rename migration sync failure must keep the published catalog active");

        let published = tokio::fs::read(&paths.catalog)
            .await
            .expect("read published migration result");
        assert_eq!(
            catalog_fixture_version(&published),
            u64::from(CATALOG_VERSION)
        );
        let reopened = ProjectCatalog::load_existing(&paths.catalog)
            .await
            .expect("published v4 migration remains readable");
        assert_eq!(reopened, migrated);
    }

    #[tokio::test]
    async fn exact_domain_targets_survive_catalog_reopen() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.project("domain-project");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(project)
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project(discovery, Some("domain-project".to_owned()))
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "domain-project.localhost"
                        .parse()
                        .expect("valid exact domain"),
                    instance_id,
                    "domain-project:web".to_owned(),
                )],
            )
            .expect("replace exact claims");
        catalog.save().await.expect("save exact claims");

        let reopened = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("reopen catalog");

        assert_eq!(
            reopened.domain_index().resolve("DOMAIN-PROJECT.LOCALHOST."),
            Some(&DomainTarget::Service {
                project_instance_id: instance_id,
                service_name: Some("domain-project:web".to_owned()),
            })
        );
        assert!(
            reopened.instances[&instance_id]
                .domain_claims
                .contains("domain-project.localhost")
        );
    }

    #[tokio::test]
    async fn legacy_flat_domain_claims_hydrate_as_owned_exact_names() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let project = fixture.project("legacy-domain-project");
        let mut catalog = ProjectCatalog::load_from_paths(paths.clone())
            .await
            .expect("initialize catalog");
        let discovery = ProjectCatalog::discover(project)
            .await
            .expect("discover project");
        let instance_id = catalog
            .register_project_and_save(discovery, Some("legacy-domain".to_owned()))
            .await
            .expect("register project");
        let mut stored: Value =
            serde_json::from_slice(&tokio::fs::read(&paths.catalog).await.expect("read catalog"))
                .expect("parse catalog");
        stored["version"] = Value::from(LEGACY_CATALOG_VERSION);
        stored
            .as_object_mut()
            .expect("catalog object")
            .remove("domain_index");
        stored["instances"][instance_id.to_string()]["domain_claims"] =
            serde_json::json!(["LEGACY.LOCALHOST."]);
        tokio::fs::write(
            &paths.catalog,
            serde_json::to_vec_pretty(&stored).expect("serialize legacy catalog"),
        )
        .await
        .expect("write legacy catalog");

        let reopened = ProjectCatalog::load_from_paths(paths)
            .await
            .expect("hydrate legacy claims");

        assert_eq!(
            reopened.domain_index().resolve("legacy.localhost"),
            Some(&DomainTarget::Service {
                project_instance_id: instance_id,
                service_name: None,
            })
        );
        assert_eq!(
            reopened.instances[&instance_id].domain_claims,
            BTreeSet::from(["legacy.localhost".to_owned()])
        );
    }

    #[tokio::test]
    async fn conflicting_catalog_replacement_preserves_the_previous_claim_set() {
        let fixture = Fixture::new();
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);
        let first_path = fixture.project("first-domain-project");
        let second_path = fixture.project("second-domain-project");
        let first = catalog
            .register_project(
                ProjectCatalog::discover(first_path)
                    .await
                    .expect("discover first project"),
                Some("first".to_owned()),
            )
            .expect("register first project");
        let second = catalog
            .register_project(
                ProjectCatalog::discover(second_path)
                    .await
                    .expect("discover second project"),
                Some("second".to_owned()),
            )
            .expect("register second project");
        catalog
            .replace_domain_claims(
                first,
                [DomainClaim::service(
                    "shared.localhost".parse().expect("valid domain"),
                    first,
                    "first:web".to_owned(),
                )],
            )
            .expect("record first claim");
        let before = catalog.clone();

        let error = catalog
            .replace_domain_claims(
                second,
                [DomainClaim::service(
                    "shared.localhost".parse().expect("valid domain"),
                    second,
                    "second:web".to_owned(),
                )],
            )
            .expect_err("conflict must fail");

        assert!(error.to_string().contains("first:web"));
        assert!(error.to_string().contains("second:web"));
        assert_eq!(catalog, before);
    }

    #[tokio::test]
    async fn forgetting_an_instance_releases_its_exact_domains() {
        let fixture = Fixture::new();
        let project = fixture.project("forgotten-domain-project");
        let mut catalog = ProjectCatalog::with_path(fixture.paths().catalog);
        let instance_id = catalog
            .register_project(
                ProjectCatalog::discover(project.clone())
                    .await
                    .expect("discover project"),
                Some("forgotten".to_owned()),
            )
            .expect("register project");
        catalog
            .replace_domain_claims(
                instance_id,
                [DomainClaim::service(
                    "forgotten.localhost".parse().expect("valid domain"),
                    instance_id,
                    "forgotten:web".to_owned(),
                )],
            )
            .expect("record claim");

        assert!(
            catalog
                .unregister_project(&project)
                .expect("forget project")
        );

        assert!(
            catalog
                .domain_index()
                .resolve("forgotten.localhost")
                .is_none()
        );
        assert!(!catalog.instances.contains_key(&instance_id));
    }

    #[allow(clippy::disallowed_methods)]
    fn git(current_dir: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(current_dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn copy_directory(source: &Path, destination: &Path) {
        std::fs::create_dir(destination).expect("create destination");
        for entry in std::fs::read_dir(source).expect("read source") {
            let entry = entry.expect("source entry");
            let target = destination.join(entry.file_name());
            if entry.file_type().expect("entry type").is_dir() {
                copy_directory(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).expect("copy file");
            }
        }
    }
}
