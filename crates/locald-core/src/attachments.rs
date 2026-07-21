use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{MANUAL_DEMAND_TTL, ProjectInstanceId, VSCODE_DEMAND_TTL};

const LEGACY_EDITOR_MIGRATION_TTL: Duration = Duration::from_mins(30);

/// Retry-stable identity for one log-following `locald up` session.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualCliSession {
    pid: u32,
    #[schemars(with = "String")]
    id: Uuid,
}

impl ManualCliSession {
    #[must_use]
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            id: Uuid::new_v4(),
        }
    }

    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }

    #[must_use]
    pub const fn id(self) -> Uuid {
        self.id
    }

    #[must_use]
    pub const fn attachment_source(self) -> AttachmentSource {
        AttachmentSource::ManualCLI(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub enum AttachmentSource {
    Editor {
        name: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
    },
    CLI {
        pid: u32,
    },
    /// Process owner paired with the `Start` request that acquired the
    /// semantic Manual CLI demand for a log-following `locald up` session.
    ///
    /// Unlike a generic legacy CLI attachment, detaching this exact owner may
    /// release that Manual demand. Keeping the provenance in durable
    /// compatibility state makes the behavior stable across daemon restarts.
    ManualCLI(ManualCliSession),
    /// Parked quiet-up hold retained for the availability-store migration.
    ///
    /// It is preserved as legacy evidence and does not count as a current live
    /// attachment in this compatibility model.
    Runtime,
    Pin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub project_path: PathBuf,
    pub source: AttachmentSource,
    pub created_at: SystemTime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub enum ProjectFilter {
    Active,
    Pinned,
    Recent,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ProjectStatusInfo {
    pub project_path: PathBuf,
    pub project_name: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub is_running: bool,
    #[serde(default)]
    pub services: Vec<String>,
    /// Full service details (ports, URLs, health, etc.)
    #[serde(default)]
    pub service_details: Vec<crate::ipc::ServiceStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ProjectListEntry {
    pub project_path: PathBuf,
    pub project_name: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub is_running: bool,
    pub section: ProjectSection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub enum ProjectSection {
    Active,
    AlwaysOn,
    Recent,
}

/// An exact, deterministically ordered attachment compatibility projection.
///
/// This is suitable for embedding as the before or after image in a lifecycle
/// transaction journal. Project keys and manual-stop markers serialize in path
/// order while attachment order remains exactly as it appeared in the store.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttachmentStoreSnapshot {
    pub attachments: BTreeMap<PathBuf, Vec<Attachment>>,
    pub manually_stopped: BTreeSet<PathBuf>,
    pub instance_owners: BTreeMap<PathBuf, ProjectInstanceId>,
}

/// The complete attachment compatibility state owned by one project path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentProjectSnapshot {
    pub project_path: PathBuf,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub manually_stopped: bool,
    #[serde(default)]
    pub instance_owner: Option<ProjectInstanceId>,
}

/// The exact effect of replacing one project's compatibility projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentProjectDelta {
    pub before: AttachmentProjectSnapshot,
    pub after: AttachmentProjectSnapshot,
    #[serde(default)]
    pub removed: Vec<Attachment>,
    #[serde(default)]
    pub added: Vec<Attachment>,
}

/// One legacy attachment together with its liveness at a caller-selected time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentLivenessEvidence {
    pub attachment: Attachment,
    pub alive: bool,
}

/// Deterministic legacy lifecycle evidence for one compatibility-store project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentCompatibilityEvidence {
    pub project_path: PathBuf,
    #[serde(default)]
    pub attachments: Vec<AttachmentLivenessEvidence>,
    pub manually_stopped: bool,
}

/// A compatibility-store load or persistence failure.
#[derive(Debug, Error)]
pub enum AttachmentStoreError {
    #[error("invalid attachment state at {path}: {reason}")]
    InvalidData { path: PathBuf, reason: String },
    #[error("failed to {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "attachment state at {path} was published but may not be durable because its parent directory could not be synced: {reason}"
    )]
    PublishedNotDurable { path: PathBuf, reason: String },
}

#[derive(Debug, Default, Deserialize)]
struct LegacyAttachmentStoreData {
    #[serde(default)]
    attachments: BTreeMap<PathBuf, Vec<serde_json::Value>>,
    #[serde(default)]
    manually_stopped: BTreeSet<PathBuf>,
    #[serde(default)]
    instance_owners: BTreeMap<PathBuf, ProjectInstanceId>,
}

#[derive(Debug, Default, Clone)]
pub struct AttachmentStore {
    path: PathBuf,
    attachments: HashMap<PathBuf, Vec<Attachment>>,
    manually_stopped: HashSet<PathBuf>,
    instance_owners: HashMap<PathBuf, ProjectInstanceId>,
}

impl AttachmentStoreSnapshot {
    /// Return the exact compatibility projection for one canonicalized target.
    pub fn project(&self, project_path: &Path) -> AttachmentProjectSnapshot {
        let project_path = AttachmentStore::canonicalize_path(project_path);
        AttachmentProjectSnapshot {
            attachments: self
                .attachments
                .get(&project_path)
                .cloned()
                .unwrap_or_default(),
            manually_stopped: self.manually_stopped.contains(&project_path),
            instance_owner: self.instance_owners.get(&project_path).copied(),
            project_path,
        }
    }

    /// Replace one project's complete compatibility projection and return its
    /// exact before/after delta.
    ///
    /// This intentionally accepts parked `Runtime` evidence: unlike the live
    /// `attach` operation, whole-target replacement is the privileged seam used
    /// by migration planning and journal replay.
    pub fn replace_project(
        &mut self,
        project_path: &Path,
        mut attachments: Vec<Attachment>,
        manually_stopped: bool,
    ) -> AttachmentProjectDelta {
        let project_path = AttachmentStore::canonicalize_path(project_path);
        let before = self.project(&project_path);
        for attachment in &mut attachments {
            attachment.project_path.clone_from(&project_path);
        }

        if attachments.is_empty() {
            self.attachments.remove(&project_path);
        } else {
            self.attachments
                .insert(project_path.clone(), attachments.clone());
        }
        if manually_stopped {
            self.manually_stopped.insert(project_path.clone());
        } else {
            self.manually_stopped.remove(&project_path);
        }
        if attachments.is_empty() && !manually_stopped {
            self.instance_owners.remove(&project_path);
        }

        let instance_owner = self.instance_owners.get(&project_path).copied();
        let after = AttachmentProjectSnapshot {
            project_path,
            attachments,
            manually_stopped,
            instance_owner,
        };
        let removed = exact_attachment_difference(&before.attachments, &after.attachments);
        let added = exact_attachment_difference(&after.attachments, &before.attachments);
        AttachmentProjectDelta {
            before,
            after,
            removed,
            added,
        }
    }

    #[must_use]
    pub fn instance_owner(&self, project_path: &Path) -> Option<ProjectInstanceId> {
        let project_path = AttachmentStore::canonicalize_path(project_path);
        self.instance_owners.get(&project_path).copied()
    }

    pub fn set_instance_owner(&mut self, project_path: &Path, instance_id: ProjectInstanceId) {
        let project_path = AttachmentStore::canonicalize_path(project_path);
        self.instance_owners.insert(project_path, instance_id);
    }

    pub fn clear_instance_owner(&mut self, project_path: &Path) {
        let project_path = AttachmentStore::canonicalize_path(project_path);
        self.instance_owners.remove(&project_path);
    }

    /// Validate the invariants required of exact compatibility authority.
    pub fn validate_exact(&self) -> Result<()> {
        for (project_path, attachments) in &self.attachments {
            anyhow::ensure!(
                !attachments.is_empty(),
                "attachment state contains an empty compatibility entry at `{}`",
                project_path.display()
            );
            anyhow::ensure!(
                attachments
                    .iter()
                    .all(|attachment| attachment.project_path == *project_path),
                "attachment state at `{}` contains an attachment owned by a different project path",
                project_path.display()
            );
        }
        for project_path in self.instance_owners.keys() {
            let has_attachments = self
                .attachments
                .get(project_path)
                .is_some_and(|attachments| !attachments.is_empty());
            anyhow::ensure!(
                has_attachments || self.manually_stopped.contains(project_path),
                "attachment state contains an instance owner without compatibility state at `{}`",
                project_path.display()
            );
        }
        Ok(())
    }

    /// Derive liveness for one exact snapshot projection using a caller-owned
    /// clock and PID probe.
    pub fn compatibility_evidence_at<F>(
        &self,
        project_path: &Path,
        now: SystemTime,
        mut pid_alive: F,
    ) -> AttachmentCompatibilityEvidence
    where
        F: FnMut(u32) -> bool,
    {
        let project = self.project(project_path);
        let legacy_migration = project.instance_owner.is_none();
        let attachments = project
            .attachments
            .into_iter()
            .map(|attachment| {
                let alive = AttachmentStore::attachment_alive_for_owner_with(
                    &attachment,
                    now,
                    &mut pid_alive,
                    legacy_migration,
                );
                AttachmentLivenessEvidence { attachment, alive }
            })
            .collect();
        AttachmentCompatibilityEvidence {
            project_path: project.project_path,
            attachments,
            manually_stopped: project.manually_stopped,
        }
    }

    /// Remove only the exact observed compatibility record.
    ///
    /// Reapers use this compare-and-remove seam so a refreshed editor owner
    /// with the same stable window identity cannot be detached by stale PID
    /// evidence captured before the refresh.
    pub fn remove_exact_attachment(
        &mut self,
        project_path: &Path,
        attachment: &Attachment,
    ) -> bool {
        let project_path = AttachmentStore::canonicalize_path(project_path);
        let Some(existing) = self.attachments.get_mut(&project_path) else {
            return false;
        };
        let mut expected = attachment.clone();
        expected.project_path.clone_from(&project_path);
        let Some(index) = existing.iter().position(|item| item == &expected) else {
            return false;
        };
        existing.remove(index);
        if existing.is_empty() {
            self.attachments.remove(&project_path);
            if !self.manually_stopped.contains(&project_path) {
                self.instance_owners.remove(&project_path);
            }
        }
        true
    }
}

impl AttachmentStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            attachments: HashMap::new(),
            manually_stopped: HashSet::new(),
            instance_owners: HashMap::new(),
        }
    }

    pub fn path() -> PathBuf {
        crate::storage::data_dir().join("attachments.json")
    }

    /// Return the concrete persistence target owned by this store.
    pub fn storage_path(&self) -> &Path {
        &self.path
    }

    #[allow(clippy::disallowed_methods)]
    pub async fn load(&mut self) -> Result<()> {
        if self.path.exists() {
            let content = tokio::fs::read_to_string(&self.path).await?;
            if content.trim().is_empty() {
                self.attachments.clear();
                self.manually_stopped.clear();
                self.instance_owners.clear();
                return Ok(());
            }
            let data: LegacyAttachmentStoreData = serde_json::from_str(&content)?;
            let mut attachments_by_path = HashMap::<PathBuf, Vec<Attachment>>::new();
            for (path, attachments) in data.attachments {
                let path = Self::canonicalize_path(&path);
                let mut attachments = attachments
                    .into_iter()
                    .filter_map(Self::parse_legacy_attachment)
                    .collect::<Result<Vec<_>>>()?;
                for attachment in &mut attachments {
                    attachment.project_path.clone_from(&path);
                }
                attachments_by_path
                    .entry(path)
                    .or_default()
                    .extend(attachments);
            }
            self.attachments = attachments_by_path;
            self.manually_stopped = data
                .manually_stopped
                .into_iter()
                .map(|path| Self::canonicalize_path(&path))
                .collect();
            let mut instance_owners = HashMap::new();
            for (path, instance_id) in data.instance_owners {
                let path = Self::canonicalize_path(&path);
                anyhow::ensure!(
                    instance_owners
                        .insert(path.clone(), instance_id)
                        .is_none_or(|existing| existing == instance_id),
                    "legacy attachment aliases for `{}` disagree about their project instance owner",
                    path.display()
                );
            }
            self.instance_owners = instance_owners;
        }
        Ok(())
    }

    /// Load the exact post-migration compatibility projection.
    ///
    /// Unlike [`Self::load`], this rejects missing, empty, unknown, or legacy
    /// attachment state. Once lifecycle-v2 authority exists, compatibility
    /// state is a replay image rather than best-effort legacy evidence.
    pub async fn load_exact(&mut self) -> Result<()> {
        let snapshot = self.read_exact_snapshot().await?;
        snapshot.validate_exact().with_context(|| {
            format!(
                "authoritative attachment state `{}` violates exact-state invariants",
                self.path.display()
            )
        })?;
        self.apply_snapshot(snapshot);
        Ok(())
    }

    /// Load an exact before or after image from a prevalidated lifecycle
    /// transaction.
    ///
    /// Legacy migration before-images may contain invariants that the target
    /// repairs. Equality with one of the journal images is therefore the
    /// authority check at this recovery-only boundary; the target must always
    /// satisfy exact-state invariants.
    pub async fn load_exact_transaction_image(
        &mut self,
        base: &AttachmentStoreSnapshot,
        target: &AttachmentStoreSnapshot,
    ) -> Result<()> {
        target
            .validate_exact()
            .context("lifecycle transaction attachment target violates exact-state invariants")?;
        let snapshot = self.read_exact_snapshot().await?;
        anyhow::ensure!(
            snapshot == *base || snapshot == *target,
            "exact compatibility state `{}` matches neither the journal base nor target",
            self.path.display()
        );
        self.apply_snapshot(snapshot);
        Ok(())
    }

    async fn read_exact_snapshot(&self) -> Result<AttachmentStoreSnapshot> {
        let content = tokio::fs::read_to_string(&self.path).await?;
        anyhow::ensure!(
            !content.trim().is_empty(),
            "authoritative attachment state `{}` is empty",
            self.path.display()
        );
        serde_json::from_str(&content).map_err(Into::into)
    }

    /// Persist compatibility state through the exact-state publication boundary.
    ///
    /// Empty buckets left by ignored legacy attachment sources carry no
    /// compatibility evidence, so they are omitted from the published image.
    #[allow(clippy::disallowed_methods)]
    pub async fn save(&self) -> Result<()> {
        let mut snapshot = self.snapshot();
        snapshot
            .attachments
            .retain(|_, attachments| !attachments.is_empty());
        persist_attachment_snapshot(&snapshot, &self.path)
            .await
            .map_err(Into::into)
    }

    /// Return the exact compatibility-store state in deterministic key order.
    pub fn snapshot(&self) -> AttachmentStoreSnapshot {
        AttachmentStoreSnapshot {
            attachments: self
                .attachments
                .iter()
                .map(|(path, attachments)| (path.clone(), attachments.clone()))
                .collect(),
            manually_stopped: self.manually_stopped.iter().cloned().collect(),
            instance_owners: self
                .instance_owners
                .iter()
                .map(|(path, instance_id)| (path.clone(), *instance_id))
                .collect(),
        }
    }

    /// Atomically publish a complete candidate projection before making it the
    /// in-memory authority.
    ///
    /// A parent-directory sync failure means the rename already published the
    /// candidate. In that case the in-memory state follows the published file
    /// while the typed error tells the transaction journal that durability is
    /// uncertain and replay is still required.
    pub async fn replace_snapshot(
        &mut self,
        snapshot: AttachmentStoreSnapshot,
    ) -> std::result::Result<(), AttachmentStoreError> {
        self.replace_snapshot_with_parent_sync(snapshot, |path| async move {
            sync_attachment_parent(&path).await
        })
        .await
    }

    async fn replace_snapshot_with_parent_sync<Sync, SyncFuture>(
        &mut self,
        snapshot: AttachmentStoreSnapshot,
        parent_sync: Sync,
    ) -> std::result::Result<(), AttachmentStoreError>
    where
        Sync: FnOnce(PathBuf) -> SyncFuture,
        SyncFuture: Future<Output = std::result::Result<(), AttachmentStoreError>>,
    {
        snapshot
            .validate_exact()
            .map_err(|error| AttachmentStoreError::InvalidData {
                path: self.path.clone(),
                reason: error.to_string(),
            })?;
        let publication =
            persist_attachment_snapshot_with_parent_sync(&snapshot, &self.path, parent_sync).await;
        if publication.is_ok()
            || matches!(
                &publication,
                Err(AttachmentStoreError::PublishedNotDurable { .. })
            )
        {
            self.apply_snapshot(snapshot);
        }
        publication
    }

    /// Replace one project's in-memory compatibility projection and return the
    /// exact delta. Journaled callers should prefer applying this to a cloned
    /// [`AttachmentStoreSnapshot`] and then call [`Self::replace_snapshot`].
    pub fn replace_project(
        &mut self,
        project_path: &Path,
        attachments: Vec<Attachment>,
        manually_stopped: bool,
    ) -> AttachmentProjectDelta {
        let mut snapshot = self.snapshot();
        let delta = snapshot.replace_project(project_path, attachments, manually_stopped);
        self.apply_snapshot(snapshot);
        delta
    }

    pub fn set_instance_owner(&mut self, project_path: &Path, instance_id: ProjectInstanceId) {
        let project_path = Self::canonicalize_path(project_path);
        self.instance_owners.insert(project_path, instance_id);
    }

    pub fn clear_instance_owner(&mut self, project_path: &Path) {
        let project_path = Self::canonicalize_path(project_path);
        self.instance_owners.remove(&project_path);
    }

    fn apply_snapshot(&mut self, snapshot: AttachmentStoreSnapshot) {
        self.attachments = snapshot.attachments.into_iter().collect();
        self.manually_stopped = snapshot.manually_stopped.into_iter().collect();
        self.instance_owners = snapshot.instance_owners.into_iter().collect();
    }

    pub fn attach(&mut self, mut attachment: Attachment) -> Result<bool> {
        if matches!(attachment.source, AttachmentSource::Runtime) {
            anyhow::bail!(
                "Runtime attachment evidence is loaded from legacy state and cannot be created"
            );
        }
        let path = Self::canonicalize_path(&attachment.project_path);
        attachment.project_path.clone_from(&path);

        let entry = self.attachments.entry(path).or_default();
        let is_first = entry
            .iter()
            .all(|existing| matches!(existing.source, AttachmentSource::Runtime));

        entry.retain(|existing| !Self::matches_source(&attachment.source, &existing.source));
        entry.push(attachment);

        Ok(is_first)
    }

    pub fn detach(&mut self, project_path: &Path, source: &AttachmentSource) -> bool {
        if matches!(source, AttachmentSource::Runtime) {
            return false;
        }
        let path = Self::canonicalize_path(project_path);
        let Some(entry) = self.attachments.get_mut(&path) else {
            return false;
        };

        let live_before = Self::has_live_attachment(entry);
        entry.retain(|existing| !Self::matches_source(source, &existing.source));
        let live_after = Self::has_live_attachment(entry);

        if entry.is_empty() {
            self.attachments.remove(&path);
            if !self.manually_stopped.contains(&path) {
                self.instance_owners.remove(&path);
            }
        }
        live_before && !live_after
    }

    pub fn detach_all_non_pin(&mut self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        let Some(entry) = self.attachments.get_mut(&path) else {
            return false;
        };

        let live_before = Self::has_live_attachment(entry);
        entry.retain(|attachment| {
            matches!(
                attachment.source,
                AttachmentSource::Pin | AttachmentSource::Runtime
            )
        });
        let live_after = Self::has_live_attachment(entry);

        if entry.is_empty() {
            self.attachments.remove(&path);
            if !self.manually_stopped.contains(&path) {
                self.instance_owners.remove(&path);
            }
        }
        live_before && !live_after
    }

    pub fn mark_stopped(&mut self, project_path: &Path) {
        let path = Self::canonicalize_path(project_path);
        self.manually_stopped.insert(path);
    }

    pub fn clear_stopped(&mut self, project_path: &Path) {
        let path = Self::canonicalize_path(project_path);
        self.manually_stopped.remove(&path);
        if !self.attachments.contains_key(&path) {
            self.instance_owners.remove(&path);
        }
    }

    pub fn is_stopped(&self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        self.manually_stopped.contains(&path)
    }

    pub fn attachments_for(&self, project_path: &Path) -> Vec<&Attachment> {
        let path = Self::canonicalize_path(project_path);
        self.attachments
            .get(&path)
            .map_or_else(Vec::new, |attachments| attachments.iter().collect())
    }

    pub fn all_projects(&self) -> Vec<PathBuf> {
        self.attachments.keys().cloned().collect()
    }

    /// Remove every compatibility attachment and manual-stop marker owned by a
    /// project that is being explicitly removed.
    pub fn forget_project(&mut self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        let removed_attachments = self.attachments.remove(&path).is_some();
        let removed_stop = self.manually_stopped.remove(&path);
        let removed_owner = self.instance_owners.remove(&path).is_some();
        removed_attachments || removed_stop || removed_owner
    }

    pub fn reap_stale_attachments(&mut self) -> Vec<PathBuf> {
        self.reap_stale_attachments_with(SystemTime::now(), Self::pid_alive)
    }

    /// Reap stale compatibility owners using a caller-selected clock and
    /// process-liveness probe.
    ///
    /// The returned project paths are sorted so journal evidence and tests do
    /// not depend on `HashMap` iteration order.
    pub fn reap_stale_attachments_with<F>(
        &mut self,
        now: SystemTime,
        mut pid_alive: F,
    ) -> Vec<PathBuf>
    where
        F: FnMut(u32) -> bool,
    {
        let mut emptied = Vec::new();

        let mut to_remove = Vec::new();
        for (path, attachments) in &mut self.attachments {
            let legacy_migration = !self.instance_owners.contains_key(path);
            let live_before = Self::has_live_attachment(attachments);
            attachments.retain(|attachment| {
                Self::attachment_alive_for_owner_with(
                    attachment,
                    now,
                    &mut pid_alive,
                    legacy_migration,
                )
            });
            let live_after = Self::has_live_attachment(attachments);

            if attachments.is_empty() {
                to_remove.push(path.clone());
            }
            if live_before && !live_after {
                emptied.push(path.clone());
            }
        }

        for path in to_remove {
            self.attachments.remove(&path);
            if !self.manually_stopped.contains(&path) {
                self.instance_owners.remove(&path);
            }
        }

        emptied.sort();
        emptied
    }

    pub fn reap_stale_attachments_for(&mut self, project_path: &Path) -> bool {
        self.reap_stale_attachments_for_with(project_path, SystemTime::now(), Self::pid_alive)
    }

    /// Reap one target using a caller-selected clock and liveness probe.
    pub fn reap_stale_attachments_for_with<F>(
        &mut self,
        project_path: &Path,
        now: SystemTime,
        mut pid_alive: F,
    ) -> bool
    where
        F: FnMut(u32) -> bool,
    {
        let path = Self::canonicalize_path(project_path);
        let legacy_migration = !self.instance_owners.contains_key(&path);
        let Some(attachments) = self.attachments.get_mut(&path) else {
            return false;
        };

        let live_before = Self::has_live_attachment(attachments);
        attachments.retain(|attachment| {
            Self::attachment_alive_for_owner_with(attachment, now, &mut pid_alive, legacy_migration)
        });
        let live_after = Self::has_live_attachment(attachments);

        if attachments.is_empty() {
            self.attachments.remove(&path);
            if !self.manually_stopped.contains(&path) {
                self.instance_owners.remove(&path);
            }
        }
        live_before && !live_after
    }

    pub fn reap_stale_pids(&mut self) -> Vec<PathBuf> {
        self.reap_stale_attachments()
    }

    /// Return deterministic per-project legacy evidence without mutating the
    /// compatibility store.
    pub fn compatibility_evidence_at<F>(
        &self,
        now: SystemTime,
        mut pid_alive: F,
    ) -> Vec<AttachmentCompatibilityEvidence>
    where
        F: FnMut(u32) -> bool,
    {
        let snapshot = self.snapshot();
        let project_paths: BTreeSet<_> = snapshot
            .attachments
            .keys()
            .chain(snapshot.manually_stopped.iter())
            .cloned()
            .collect();

        project_paths
            .into_iter()
            .map(|project_path| {
                let legacy_migration = snapshot.instance_owner(&project_path).is_none();
                let attachments = snapshot
                    .attachments
                    .get(&project_path)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .map(|attachment| {
                        let alive = Self::attachment_alive_for_owner_with(
                            &attachment,
                            now,
                            &mut pid_alive,
                            legacy_migration,
                        );
                        AttachmentLivenessEvidence { attachment, alive }
                    })
                    .collect();
                let manually_stopped = snapshot.manually_stopped.contains(&project_path);
                AttachmentCompatibilityEvidence {
                    project_path,
                    attachments,
                    manually_stopped,
                }
            })
            .collect()
    }

    pub fn section_for(&self, project_path: &Path) -> ProjectSection {
        let path = Self::canonicalize_path(project_path);
        let Some(attachments) = self.attachments.get(&path) else {
            return ProjectSection::Recent;
        };

        if attachments.is_empty() {
            return ProjectSection::Recent;
        }

        let has_pin = attachments
            .iter()
            .any(|a| matches!(a.source, AttachmentSource::Pin));
        let has_active = attachments
            .iter()
            .any(|a| !matches!(a.source, AttachmentSource::Pin | AttachmentSource::Runtime));

        // Active takes priority: if there are non-Pin attachments, it's Active
        // even if also pinned.
        if has_active {
            ProjectSection::Active
        } else if has_pin {
            ProjectSection::AlwaysOn
        } else {
            ProjectSection::Recent
        }
    }

    fn canonicalize_path(path: &Path) -> PathBuf {
        crate::normalize_project_locator(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// Decode a persisted attachment while keeping lifecycle decisions tied to
    /// source variants understood by this compatibility model.
    ///
    /// Quiet-up builds wrote `Runtime` as a unit string. The catalog importer
    /// also recognizes earlier object-shaped evidence, so normalize that shape
    /// here as the same parked Runtime hold. Other legacy source variants remain
    /// catalog locator evidence and are omitted from current lifecycle state.
    fn parse_legacy_attachment(mut value: serde_json::Value) -> Option<Result<Attachment>> {
        let Some(source) = value.get_mut("source") else {
            return Some(Err(anyhow::anyhow!(
                "legacy attachment is missing its source"
            )));
        };
        let variant = match source {
            serde_json::Value::String(variant) => variant.clone(),
            serde_json::Value::Object(source) if source.len() == 1 => {
                let Some(variant) = source.keys().next().cloned() else {
                    return Some(Err(anyhow::anyhow!(
                        "legacy attachment source object is empty"
                    )));
                };
                variant
            }
            _ => {
                return Some(Err(anyhow::anyhow!(
                    "legacy attachment source must be a string or single-variant object"
                )));
            }
        };

        match variant.as_str() {
            "Runtime" | "Pin" if source.is_object() => {
                *source = serde_json::Value::String(variant.clone());
            }
            "Editor" | "CLI" | "ManualCLI" | "Runtime" | "Pin" => {}
            _ => return None,
        }

        Some(
            serde_json::from_value(value)
                .map_err(|error| anyhow::anyhow!("invalid legacy {variant} attachment: {error}")),
        )
    }

    fn matches_source(needle: &AttachmentSource, existing: &AttachmentSource) -> bool {
        match (needle, existing) {
            (
                AttachmentSource::Editor { name, id, .. },
                AttachmentSource::Editor {
                    name: existing_name,
                    id: existing_id,
                    ..
                },
            ) => {
                if name.is_empty() {
                    id == existing_id
                } else {
                    name == existing_name && id == existing_id
                }
            }
            _ => needle == existing,
        }
    }

    fn has_live_attachment(attachments: &[Attachment]) -> bool {
        attachments
            .iter()
            .any(|attachment| !matches!(attachment.source, AttachmentSource::Runtime))
    }

    fn attachment_alive_with<F>(attachment: &Attachment, now: SystemTime, pid_alive: &mut F) -> bool
    where
        F: FnMut(u32) -> bool,
    {
        match attachment.source {
            AttachmentSource::CLI { pid } | AttachmentSource::Editor { pid: Some(pid), .. } => {
                pid_alive(pid)
            }
            AttachmentSource::ManualCLI(session) => pid_alive(session.pid()),
            AttachmentSource::Editor { pid: None, .. } => now
                .duration_since(attachment.created_at)
                .map_or(true, |age| age < VSCODE_DEMAND_TTL),
            AttachmentSource::Runtime => now
                .duration_since(attachment.created_at)
                .map_or(true, |age| age < MANUAL_DEMAND_TTL),
            AttachmentSource::Pin => true,
        }
    }

    fn legacy_migration_attachment_alive_with<F>(
        attachment: &Attachment,
        now: SystemTime,
        pid_alive: &mut F,
    ) -> bool
    where
        F: FnMut(u32) -> bool,
    {
        if matches!(
            attachment.source,
            AttachmentSource::Editor { pid: None, .. }
        ) {
            return now
                .duration_since(attachment.created_at)
                .map_or(true, |age| age <= LEGACY_EDITOR_MIGRATION_TTL);
        }
        Self::attachment_alive_with(attachment, now, pid_alive)
    }

    fn attachment_alive_for_owner_with<F>(
        attachment: &Attachment,
        now: SystemTime,
        pid_alive: &mut F,
        legacy_migration: bool,
    ) -> bool
    where
        F: FnMut(u32) -> bool,
    {
        if legacy_migration {
            Self::legacy_migration_attachment_alive_with(attachment, now, pid_alive)
        } else {
            Self::attachment_alive_with(attachment, now, pid_alive)
        }
    }

    #[allow(unsafe_code)]
    fn pid_alive(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };

        let result = unsafe { libc::kill(pid, 0) };
        if result == 0 {
            return true;
        }

        let err = std::io::Error::last_os_error();
        err.raw_os_error() != Some(libc::ESRCH)
    }
}

fn exact_attachment_difference(
    candidates: &[Attachment],
    comparison: &[Attachment],
) -> Vec<Attachment> {
    let mut unmatched = comparison.to_vec();
    let mut difference = Vec::new();
    for candidate in candidates {
        if let Some(index) = unmatched.iter().position(|item| item == candidate) {
            unmatched.remove(index);
        } else {
            difference.push(candidate.clone());
        }
    }
    difference
}

async fn persist_attachment_snapshot(
    snapshot: &AttachmentStoreSnapshot,
    path: &Path,
) -> std::result::Result<(), AttachmentStoreError> {
    persist_attachment_snapshot_with_parent_sync(snapshot, path, |path| async move {
        sync_attachment_parent(&path).await
    })
    .await
}

async fn persist_attachment_snapshot_with_parent_sync<Sync, SyncFuture>(
    snapshot: &AttachmentStoreSnapshot,
    path: &Path,
    parent_sync: Sync,
) -> std::result::Result<(), AttachmentStoreError>
where
    Sync: FnOnce(PathBuf) -> SyncFuture,
    SyncFuture: Future<Output = std::result::Result<(), AttachmentStoreError>>,
{
    snapshot
        .validate_exact()
        .map_err(|error| AttachmentStoreError::InvalidData {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let temporary = write_temporary_attachment_snapshot(snapshot, path).await?;
    if let Err(source) = tokio::fs::rename(&temporary, path).await {
        let cleanup = tokio::fs::remove_file(&temporary).await;
        let reason = cleanup.err().map_or_else(
            || source.to_string(),
            |cleanup_error| format!("{source}; temporary cleanup also failed: {cleanup_error}"),
        );
        return Err(AttachmentStoreError::Io {
            operation: "atomically replace attachment state",
            path: path.to_path_buf(),
            source: io::Error::new(source.kind(), reason),
        });
    }

    parent_sync(path.to_path_buf()).await.map_err(|error| {
        AttachmentStoreError::PublishedNotDurable {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    })
}

async fn write_temporary_attachment_snapshot(
    snapshot: &AttachmentStoreSnapshot,
    path: &Path,
) -> std::result::Result<PathBuf, AttachmentStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| AttachmentStoreError::InvalidData {
            path: path.to_path_buf(),
            reason: "attachment state path has no parent directory".to_owned(),
        })?;
    let destination_unpublished = match tokio::fs::symlink_metadata(path).await {
        Ok(_) => false,
        Err(source) if source.kind() == io::ErrorKind::NotFound => true,
        Err(source) => {
            return Err(AttachmentStoreError::Io {
                operation: "inspect attachment state destination",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    create_attachment_parent_durably(parent, destination_unpublished).await?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachments.json");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut content = serde_json::to_vec_pretty(snapshot).map_err(|source| {
        AttachmentStoreError::InvalidData {
            path: path.to_path_buf(),
            reason: source.to_string(),
        }
    })?;
    content.push(b'\n');

    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| AttachmentStoreError::Io {
            operation: "create temporary attachment state",
            path: temporary.clone(),
            source,
        })?;
    let write_result = async {
        output.write_all(&content).await?;
        output.sync_all().await
    }
    .await;
    drop(output);
    if let Err(source) = write_result {
        let cleanup = tokio::fs::remove_file(&temporary).await;
        let reason = cleanup.err().map_or_else(
            || source.to_string(),
            |cleanup_error| format!("{source}; temporary cleanup also failed: {cleanup_error}"),
        );
        return Err(AttachmentStoreError::Io {
            operation: "write and sync temporary attachment state",
            path: temporary,
            source: io::Error::new(source.kind(), reason),
        });
    }

    Ok(temporary)
}

async fn create_attachment_parent_durably(
    parent: &Path,
    destination_unpublished: bool,
) -> std::result::Result<(), AttachmentStoreError> {
    let mut cursor = parent;
    loop {
        match tokio::fs::symlink_metadata(cursor).await {
            Ok(_) => break,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                cursor = cursor
                    .parent()
                    .ok_or_else(|| AttachmentStoreError::InvalidData {
                        path: parent.to_path_buf(),
                        reason: "attachment state directory has no existing ancestor".to_owned(),
                    })?;
            }
            Err(source) => {
                return Err(AttachmentStoreError::Io {
                    operation: "inspect attachment state directory",
                    path: cursor.to_path_buf(),
                    source,
                });
            }
        }
    }

    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|source| AttachmentStoreError::Io {
            operation: "create attachment state directory",
            path: parent.to_path_buf(),
            source,
        })?;

    // When no authoritative file exists yet, this may be a retry after an
    // earlier attempt created the hierarchy and then failed one of these
    // syncs. The retry cannot distinguish those directories from older ones,
    // so repair every owning ancestor before publishing the first file.
    if destination_unpublished {
        for owning_parent in attachment_hierarchy_owners(parent) {
            sync_attachment_directory(&owning_parent).await?;
        }
    }
    Ok(())
}

fn attachment_hierarchy_owners(parent: &Path) -> Vec<PathBuf> {
    parent
        .ancestors()
        .skip(1)
        .take_while(|ancestor| ancestor.parent().is_some())
        .map(Path::to_path_buf)
        .collect()
}

async fn sync_attachment_parent(path: &Path) -> std::result::Result<(), AttachmentStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| AttachmentStoreError::InvalidData {
            path: path.to_path_buf(),
            reason: "attachment state path has no parent directory".to_owned(),
        })?;
    sync_attachment_directory(parent).await
}

async fn sync_attachment_directory(
    directory_path: &Path,
) -> std::result::Result<(), AttachmentStoreError> {
    let directory = tokio::fs::File::open(directory_path)
        .await
        .map_err(|source| AttachmentStoreError::Io {
            operation: "open attachment state directory for sync",
            path: directory_path.to_path_buf(),
            source,
        })?;
    directory
        .sync_all()
        .await
        .map_err(|source| AttachmentStoreError::Io {
            operation: "sync attachment state directory",
            path: directory_path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::UNIX_EPOCH;
    use tempfile::tempdir;

    fn normalized_locator(path: &Path) -> PathBuf {
        crate::normalize_project_locator(path).expect("normalize test project locator")
    }

    #[test]
    fn attach_and_detach_updates_counts() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        let first = store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        assert!(first);
        assert_eq!(store.attachments_for(&project).len(), 1);

        let second = store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: 1234 },
                created_at: SystemTime::now(),
            })
            .expect("attach CLI");
        assert!(!second);
        assert_eq!(store.attachments_for(&project).len(), 2);

        let last_removed = store.detach(&project, &AttachmentSource::Pin);
        assert!(!last_removed);
        assert_eq!(store.attachments_for(&project).len(), 1);

        let last_removed = store.detach(&project, &AttachmentSource::CLI { pid: 1234 });
        assert!(last_removed);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn section_for_respects_pin_and_activity() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        assert_eq!(store.section_for(&project), ProjectSection::Recent);

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: 42 },
                created_at: SystemTime::now(),
            })
            .expect("attach CLI");
        assert_eq!(store.section_for(&project), ProjectSection::Active);

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        // Active takes priority: pinned + CLI attachment = Active.
        assert_eq!(store.section_for(&project), ProjectSection::Active);

        // Remove the CLI attachment — now only Pin remains = AlwaysOn.
        store.detach(&project, &AttachmentSource::CLI { pid: 42 });
        assert_eq!(store.section_for(&project), ProjectSection::AlwaysOn);
    }

    #[test]
    fn detach_all_non_pin_keeps_pin() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: 42 },
                created_at: SystemTime::now(),
            })
            .expect("attach CLI");

        let last_removed = store.detach_all_non_pin(&project);
        assert!(!last_removed);

        let remaining = store.attachments_for(&project);
        assert_eq!(remaining.len(), 1);
        assert!(matches!(remaining[0].source, AttachmentSource::Pin));
    }

    #[test]
    fn reap_stale_pids_prunes_dead_entries() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let alive_pid = child.id();

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: alive_pid },
                created_at: SystemTime::now(),
            })
            .expect("attach live CLI");
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: u32::MAX },
                created_at: SystemTime::now(),
            })
            .expect("attach dead CLI");

        let removed = store.reap_stale_pids();
        assert!(removed.is_empty());
        assert_eq!(store.attachments_for(&project).len(), 1);

        let _ = child.kill();
        let _ = child.wait();

        let removed = store.reap_stale_pids();
        let canonical = normalized_locator(&project);
        assert_eq!(removed, vec![canonical]);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn reap_stale_attachments_prunes_legacy_old_editor() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: None,
                },
                created_at: SystemTime::now() - Duration::from_secs(31 * 60),
            })
            .expect("attach old editor");

        let removed = store.reap_stale_attachments();
        let canonical = normalized_locator(&project);
        assert_eq!(removed, vec![canonical]);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn reap_stale_attachments_keeps_legacy_fresh_editor() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: None,
                },
                created_at: SystemTime::now(),
            })
            .expect("attach fresh editor");

        let removed = store.reap_stale_attachments();
        assert!(removed.is_empty());
        assert_eq!(store.attachments_for(&project).len(), 1);
    }

    #[test]
    fn pidless_editor_compatibility_expires_with_the_authoritative_editor_lease() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_owned(),
                    id: "window".to_owned(),
                    pid: None,
                },
                created_at: now - VSCODE_DEMAND_TTL,
            })
            .expect("attach editor at expiry boundary");
        store.set_instance_owner(
            &project,
            "00000000-0000-4000-8000-000000000123"
                .parse()
                .expect("parse project instance ID"),
        );

        assert_eq!(
            store.reap_stale_attachments_with(now, |_| false),
            vec![normalized_locator(&project)]
        );
    }

    #[test]
    fn unclaimed_pidless_editor_uses_legacy_liveness_until_instance_claim() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_owned(),
                    id: "legacy-window".to_owned(),
                    pid: None,
                },
                created_at: now - Duration::from_secs(5 * 60),
            })
            .expect("attach five-minute-old legacy editor");

        assert!(store.compatibility_evidence_at(now, |_| false)[0].attachments[0].alive);
        assert!(store.reap_stale_attachments_with(now, |_| false).is_empty());

        store.set_instance_owner(
            &project,
            "00000000-0000-4000-8000-000000000123"
                .parse()
                .expect("parse project instance ID"),
        );
        assert_eq!(
            store.reap_stale_attachments_with(now, |_| false),
            vec![normalized_locator(&project)]
        );
    }

    #[test]
    fn deferred_runtime_evidence_expires_after_its_migration_window() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");
        let now = UNIX_EPOCH + Duration::from_secs(20_000);
        store.replace_project(
            &project,
            vec![Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Runtime,
                created_at: now - MANUAL_DEMAND_TTL + Duration::from_secs(1),
            }],
            false,
        );

        assert!(store.compatibility_evidence_at(now, |_| false)[0].attachments[0].alive);
        let after_deadline = now + Duration::from_secs(1);
        assert_eq!(
            store.reap_stale_attachments_with(after_deadline, |_| false),
            Vec::<PathBuf>::new()
        );
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn reap_stale_attachments_prunes_dead_editor_pid() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: Some(u32::MAX),
                },
                created_at: SystemTime::now(),
            })
            .expect("attach dead editor");

        let removed = store.reap_stale_attachments();
        let canonical = normalized_locator(&project);
        assert_eq!(removed, vec![canonical]);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn reap_stale_attachments_keeps_pin() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now() - Duration::from_secs(31 * 60),
            })
            .expect("attach pin");

        let removed = store.reap_stale_attachments();
        assert!(removed.is_empty());
        assert_eq!(store.attachments_for(&project).len(), 1);
    }

    #[test]
    fn detach_editor_matching_ignores_pid() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: Some(1234),
                },
                created_at: SystemTime::now(),
            })
            .expect("attach editor");

        let last_removed = store.detach(
            &project,
            &AttachmentSource::Editor {
                name: String::new(),
                id: "abc".to_string(),
                pid: None,
            },
        );

        assert!(last_removed);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn exact_snapshot_removal_preserves_refreshed_editor_owner() {
        let project = PathBuf::from("/projects/editor-refresh");
        let stale = Attachment {
            project_path: project.clone(),
            source: AttachmentSource::Editor {
                name: "Code".to_owned(),
                id: "window-a".to_owned(),
                pid: Some(10),
            },
            created_at: UNIX_EPOCH + Duration::from_secs(1),
        };
        let refreshed = Attachment {
            project_path: project.clone(),
            source: AttachmentSource::Editor {
                name: "Code".to_owned(),
                id: "window-a".to_owned(),
                pid: Some(20),
            },
            created_at: UNIX_EPOCH + Duration::from_secs(2),
        };
        let mut snapshot = AttachmentStoreSnapshot::default();
        snapshot.replace_project(&project, vec![refreshed.clone()], false);

        assert!(!snapshot.remove_exact_attachment(&project, &stale));
        assert_eq!(snapshot.project(&project).attachments, vec![refreshed]);
    }

    #[test]
    fn compatibility_projection_persists_instance_provenance_and_clears_it_with_state() {
        let project = PathBuf::from("/projects/owned");
        let instance_id: ProjectInstanceId = "00000000-0000-4000-8000-000000000123"
            .parse()
            .expect("parse project instance ID");
        let mut snapshot = AttachmentStoreSnapshot::default();
        snapshot.replace_project(
            &project,
            vec![Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: UNIX_EPOCH,
            }],
            false,
        );
        snapshot.set_instance_owner(&project, instance_id);

        let encoded = serde_json::to_vec(&snapshot).expect("serialize owned projection");
        let mut decoded: AttachmentStoreSnapshot =
            serde_json::from_slice(&encoded).expect("deserialize owned projection");
        assert_eq!(decoded.instance_owner(&project), Some(instance_id));

        decoded.replace_project(&project, Vec::new(), false);
        assert_eq!(decoded.instance_owner(&project), None);
    }

    #[test]
    fn exact_compatibility_snapshot_rejects_unknown_and_missing_fields() {
        let encoded =
            serde_json::to_value(AttachmentStoreSnapshot::default()).expect("serialize snapshot");
        let mut unknown = encoded.clone();
        unknown["unexpected"] = serde_json::json!(true);

        assert!(serde_json::from_value::<AttachmentStoreSnapshot>(unknown).is_err());

        for field in ["attachments", "manually_stopped", "instance_owners"] {
            let mut missing = encoded.clone();
            missing
                .as_object_mut()
                .expect("snapshot serializes as an object")
                .remove(field);
            assert!(
                serde_json::from_value::<AttachmentStoreSnapshot>(missing).is_err(),
                "exact snapshot must require {field}"
            );
        }

        assert!(
            serde_json::from_value::<AttachmentSource>(serde_json::json!({
                "Editor": {
                    "name": "Code",
                    "id": "window-a",
                    "unexpected": true
                }
            }))
            .is_err()
        );
        let mut attachment = serde_json::to_value(Attachment {
            project_path: PathBuf::from("/project"),
            source: AttachmentSource::Pin,
            created_at: UNIX_EPOCH,
        })
        .expect("serialize attachment");
        attachment["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Attachment>(attachment).is_err());
    }

    #[tokio::test]
    async fn exact_load_rejects_missing_empty_and_legacy_state() {
        let dir = tempdir().expect("create temporary directory");
        let store_path = dir.path().join("attachments.json");
        let mut store = AttachmentStore::new(store_path.clone());
        assert!(store.load_exact().await.is_err());

        tokio::fs::write(&store_path, b"\n")
            .await
            .expect("write empty authoritative state");
        assert!(store.load_exact().await.is_err());

        tokio::fs::write(&store_path, br#"{"attachments": {}}"#)
            .await
            .expect("write legacy attachment state");
        assert!(store.load_exact().await.is_err());
    }

    #[tokio::test]
    async fn exact_load_rejects_invariant_violations_without_mutating_memory() {
        let dir = tempdir().expect("create exact-state invariant fixture");
        let store_path = dir.path().join("attachments.json");
        let baseline_project = dir.path().join("baseline");
        let project = dir.path().join("project");
        let foreign_project = dir.path().join("foreign");
        let instance_id: ProjectInstanceId = "00000000-0000-4000-8000-000000000123"
            .parse()
            .expect("parse project instance ID");

        let mut stop_only = AttachmentStoreSnapshot::default();
        stop_only.manually_stopped.insert(project.clone());
        stop_only
            .instance_owners
            .insert(project.clone(), instance_id);
        stop_only
            .validate_exact()
            .expect("a manual-stop marker is compatibility state for an owner");

        let mut empty_bucket = AttachmentStoreSnapshot::default();
        empty_bucket.attachments.insert(project.clone(), Vec::new());
        empty_bucket
            .instance_owners
            .insert(project.clone(), instance_id);

        let mut mismatched_path = AttachmentStoreSnapshot::default();
        mismatched_path.attachments.insert(
            project.clone(),
            vec![Attachment {
                project_path: foreign_project,
                source: AttachmentSource::Pin,
                created_at: UNIX_EPOCH,
            }],
        );

        let mut orphaned_owner = AttachmentStoreSnapshot::default();
        orphaned_owner.instance_owners.insert(project, instance_id);

        for (label, candidate, expected_error) in [
            (
                "empty attachment bucket",
                empty_bucket,
                "empty compatibility entry",
            ),
            (
                "mismatched embedded path",
                mismatched_path,
                "different project path",
            ),
            (
                "orphaned instance owner",
                orphaned_owner,
                "instance owner without compatibility state",
            ),
        ] {
            let preserved =
                serde_json::to_vec_pretty(&candidate).expect("serialize invalid exact state");
            tokio::fs::write(&store_path, &preserved)
                .await
                .expect("write invalid exact state");
            let mut store = AttachmentStore::new(store_path.clone());
            store.mark_stopped(&baseline_project);
            let baseline = store.snapshot();

            let error = store
                .load_exact()
                .await
                .expect_err("invalid exact state must fail closed");

            assert!(
                format!("{error:#}").contains(expected_error),
                "{label}: {error:#}"
            );
            assert_eq!(store.snapshot(), baseline, "{label} changed memory");
            assert_eq!(
                tokio::fs::read(&store_path)
                    .await
                    .expect("reread invalid exact state"),
                preserved,
                "{label} changed disk"
            );
        }
    }

    #[tokio::test]
    async fn exact_replacement_rejects_invalid_authority_before_publication() {
        let dir = tempdir().expect("create exact replacement fixture");
        let store_path = dir.path().join("attachments.json");
        let baseline_project = dir.path().join("baseline");
        let candidate_project = dir.path().join("candidate");
        let mut baseline = AttachmentStoreSnapshot::default();
        baseline.replace_project(
            &baseline_project,
            vec![Attachment {
                project_path: baseline_project.clone(),
                source: AttachmentSource::Pin,
                created_at: UNIX_EPOCH,
            }],
            false,
        );
        let mut store = AttachmentStore::new(store_path.clone());
        store
            .replace_snapshot(baseline.clone())
            .await
            .expect("publish valid baseline");
        let preserved = tokio::fs::read(&store_path)
            .await
            .expect("read valid baseline");

        let mut invalid = AttachmentStoreSnapshot::default();
        invalid.attachments.insert(candidate_project, Vec::new());
        let error = store
            .replace_snapshot(invalid)
            .await
            .expect_err("invalid exact replacement must fail before publication");

        assert!(matches!(error, AttachmentStoreError::InvalidData { .. }));
        assert_eq!(store.snapshot(), baseline);
        assert_eq!(
            tokio::fs::read(&store_path)
                .await
                .expect("reread preserved baseline"),
            preserved
        );
    }

    #[tokio::test]
    async fn save_rejects_an_orphaned_owner_before_publication() {
        let dir = tempdir().expect("create exact save fixture");
        let store_path = dir.path().join("attachments.json");
        let baseline_project = dir.path().join("baseline");
        let orphaned_project = dir.path().join("orphaned");
        let instance_id: ProjectInstanceId = "00000000-0000-4000-8000-000000000456"
            .parse()
            .expect("parse project instance ID");
        let mut store = AttachmentStore::new(store_path.clone());
        store.mark_stopped(&baseline_project);
        store.save().await.expect("publish valid baseline");
        let preserved = tokio::fs::read(&store_path)
            .await
            .expect("read valid baseline");

        store.set_instance_owner(&orphaned_project, instance_id);
        let error = store
            .save()
            .await
            .expect_err("an orphaned owner must fail before publication");

        assert!(
            format!("{error:#}").contains("instance owner without compatibility state"),
            "{error:#}"
        );
        assert_eq!(
            tokio::fs::read(&store_path)
                .await
                .expect("reread preserved baseline"),
            preserved
        );
        let mut reloaded = AttachmentStore::new(store_path);
        reloaded
            .load_exact()
            .await
            .expect("the previously published exact baseline remains valid");
        assert!(reloaded.is_stopped(&baseline_project));
    }

    #[tokio::test]
    async fn transaction_image_load_accepts_an_exact_legacy_base_for_repair() {
        let dir = tempdir().expect("create transaction image fixture");
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let mut legacy_base = AttachmentStoreSnapshot::default();
        legacy_base.attachments.insert(
            project,
            vec![Attachment {
                project_path: dir.path().join("embedded-legacy-project"),
                source: AttachmentSource::Runtime,
                created_at: UNIX_EPOCH,
            }],
        );
        let target = AttachmentStoreSnapshot::default();
        tokio::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy_base).expect("serialize legacy base"),
        )
        .await
        .expect("write legacy base");
        let mut store = AttachmentStore::new(store_path);

        store
            .load_exact_transaction_image(&legacy_base, &target)
            .await
            .expect("load exact journal base for target repair");

        assert_eq!(store.snapshot(), legacy_base);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_alias_collisions_merge_in_stable_source_key_order() {
        let dir = tempdir().expect("create legacy alias fixture");
        let store_path = dir.path().join("attachments.json");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("create canonical project path");

        let mut legacy_attachments = BTreeMap::<PathBuf, Vec<serde_json::Value>>::new();
        for index in 0..8 {
            let alias = dir.path().join(format!("alias-{index:02}"));
            std::os::unix::fs::symlink(&real, &alias).expect("create project path alias");
            let attachment = Attachment {
                project_path: alias.clone(),
                source: AttachmentSource::CLI { pid: index },
                created_at: UNIX_EPOCH + Duration::from_secs(u64::from(index)),
            };
            legacy_attachments.insert(
                alias,
                vec![serde_json::to_value(attachment).expect("serialize legacy attachment")],
            );
        }
        let legacy = serde_json::json!({
            "attachments": legacy_attachments,
            "manually_stopped": []
        });
        tokio::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy store"),
        )
        .await
        .expect("write legacy store");

        let mut expected = None;
        for _ in 0..8 {
            let mut store = AttachmentStore::new(store_path.clone());
            store.load().await.expect("load legacy aliases");
            let snapshot = store.snapshot();
            if let Some(expected) = &expected {
                assert_eq!(&snapshot, expected);
            } else {
                expected = Some(snapshot);
            }
            assert_eq!(
                store
                    .attachments_for(&real)
                    .into_iter()
                    .map(|attachment| match &attachment.source {
                        AttachmentSource::CLI { pid } => *pid,
                        source => panic!("expected CLI attachment, found {source:?}"),
                    })
                    .collect::<Vec<_>>(),
                (0..8).collect::<Vec<_>>()
            );
        }
    }

    #[tokio::test]
    async fn manually_stopped_persists() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");

        let mut store = AttachmentStore::new(store_path.clone());
        store.mark_stopped(&project);
        store.save().await.unwrap();

        let mut loaded = AttachmentStore::new(store_path.clone());
        loaded.load().await.unwrap();
        assert!(loaded.is_stopped(&project));

        loaded.clear_stopped(&project);
        loaded.save().await.unwrap();

        let mut reloaded = AttachmentStore::new(store_path);
        reloaded.load().await.unwrap();
        assert!(!reloaded.is_stopped(&project));
    }

    #[test]
    fn snapshot_target_replacement_reports_an_exact_multiset_delta() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).expect("create project");
        let canonical = std::fs::canonicalize(&project).expect("canonical project");
        let first = Attachment {
            project_path: canonical.clone(),
            source: AttachmentSource::CLI { pid: 1 },
            created_at: UNIX_EPOCH + Duration::from_secs(10),
        };
        let duplicate = first.clone();
        let retained = Attachment {
            project_path: canonical.clone(),
            source: AttachmentSource::Pin,
            created_at: UNIX_EPOCH + Duration::from_secs(20),
        };
        let added = Attachment {
            project_path: canonical.clone(),
            source: AttachmentSource::Editor {
                name: "vscode".to_owned(),
                id: "window".to_owned(),
                pid: None,
            },
            created_at: UNIX_EPOCH + Duration::from_secs(30),
        };

        let mut snapshot = AttachmentStoreSnapshot::default();
        snapshot.attachments.insert(
            canonical.clone(),
            vec![first.clone(), duplicate, retained.clone()],
        );

        let delta = snapshot.replace_project(
            &project,
            vec![first.clone(), retained.clone(), added.clone()],
            true,
        );

        assert_eq!(delta.before.attachments.len(), 3);
        assert_eq!(
            delta.after.attachments,
            vec![first, retained, added.clone()]
        );
        assert_eq!(delta.removed.len(), 1);
        assert!(matches!(
            delta.removed[0].source,
            AttachmentSource::CLI { pid: 1 }
        ));
        assert_eq!(delta.added, vec![added]);
        assert!(delta.after.manually_stopped);
        assert_eq!(snapshot.project(&project), delta.after);
    }

    #[test]
    fn compatibility_evidence_uses_injected_time_and_process_liveness() {
        let dir = tempdir().unwrap();
        let first_project = dir.path().join("a-project");
        let stopped_project = dir.path().join("b-stopped");
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        let attachments = vec![
            Attachment {
                project_path: first_project.clone(),
                source: AttachmentSource::CLI { pid: 10 },
                created_at: now,
            },
            Attachment {
                project_path: first_project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_owned(),
                    id: "old-window".to_owned(),
                    pid: None,
                },
                created_at: now - Duration::from_secs(31 * 60),
            },
            Attachment {
                project_path: first_project.clone(),
                source: AttachmentSource::Runtime,
                created_at: UNIX_EPOCH,
            },
        ];
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        store.replace_project(&first_project, attachments, false);
        store.mark_stopped(&stopped_project);

        let evidence = store.compatibility_evidence_at(now, |pid| pid == 10);

        assert_eq!(evidence.len(), 2);
        assert_eq!(evidence[0].project_path, normalized_locator(&first_project));
        assert_eq!(
            evidence[0]
                .attachments
                .iter()
                .map(|item| item.alive)
                .collect::<Vec<_>>(),
            vec![true, false, true]
        );
        assert!(!evidence[0].manually_stopped);
        assert_eq!(
            evidence[1].project_path,
            normalized_locator(&stopped_project)
        );
        assert!(evidence[1].attachments.is_empty());
        assert!(evidence[1].manually_stopped);
    }

    #[test]
    fn deterministic_reaper_keeps_parked_evidence_and_reports_orphaned_projects() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("project");
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        store.replace_project(
            &project,
            vec![
                Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::Runtime,
                    created_at: UNIX_EPOCH,
                },
                Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::CLI { pid: 22 },
                    created_at: now,
                },
            ],
            false,
        );

        let orphaned = store.reap_stale_attachments_with(now, |_| false);

        assert_eq!(orphaned, vec![normalized_locator(&project)]);
        assert_eq!(store.attachments_for(&project).len(), 1);
        assert!(matches!(
            store.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));
    }

    #[tokio::test]
    async fn durable_snapshot_replacement_round_trips_and_leaves_no_temporary_file() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let attachment = Attachment {
            project_path: project.clone(),
            source: AttachmentSource::Pin,
            created_at: UNIX_EPOCH + Duration::from_secs(5),
        };
        let mut candidate = AttachmentStoreSnapshot::default();
        candidate.replace_project(&project, vec![attachment], true);
        let mut store = AttachmentStore::new(store_path.clone());

        store
            .replace_snapshot(candidate.clone())
            .await
            .expect("publish candidate");

        assert_eq!(store.snapshot(), candidate);
        let mut reloaded = AttachmentStore::new(store_path.clone());
        reloaded.load_exact().await.expect("reload exact candidate");
        assert_eq!(reloaded.snapshot(), candidate);
        assert_eq!(reloaded.storage_path(), store_path);
        assert!(
            std::fs::read_dir(dir.path())
                .expect("read store directory")
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
    }

    #[tokio::test]
    async fn first_snapshot_publication_creates_a_reloadable_missing_hierarchy() {
        let dir = tempdir().unwrap();
        let store_path = dir
            .path()
            .join("missing")
            .join("nested")
            .join("attachments.json");
        let project = dir.path().join("project");
        let mut candidate = AttachmentStoreSnapshot::default();
        candidate.replace_project(
            &project,
            vec![Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: UNIX_EPOCH + Duration::from_secs(5),
            }],
            false,
        );
        let mut store = AttachmentStore::new(store_path.clone());

        store
            .replace_snapshot(candidate.clone())
            .await
            .expect("publish through a missing hierarchy");

        let mut reloaded = AttachmentStore::new(store_path);
        reloaded.load().await.expect("reload first publication");
        assert_eq!(reloaded.snapshot(), candidate);
    }

    #[test]
    fn first_snapshot_publication_repairs_every_non_root_hierarchy_owner() {
        assert_eq!(
            attachment_hierarchy_owners(Path::new("/existing/missing/nested/attachment-state")),
            vec![
                PathBuf::from("/existing/missing/nested"),
                PathBuf::from("/existing/missing"),
                PathBuf::from("/existing"),
            ]
        );
    }

    #[tokio::test]
    async fn published_not_durable_replacement_aligns_memory_with_the_published_candidate() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let mut store = AttachmentStore::new(store_path.clone());
        store.mark_stopped(&dir.path().join("old-project"));
        store.save().await.expect("persist baseline");
        let mut candidate = AttachmentStoreSnapshot::default();
        candidate.replace_project(
            &project,
            vec![Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: UNIX_EPOCH + Duration::from_secs(5),
            }],
            true,
        );

        let error = store
            .replace_snapshot_with_parent_sync(candidate.clone(), |path| async move {
                Err(AttachmentStoreError::Io {
                    operation: "injected parent-directory sync",
                    path,
                    source: io::Error::other("injected failure"),
                })
            })
            .await
            .expect_err("parent sync must report uncertain durability");

        assert!(matches!(
            error,
            AttachmentStoreError::PublishedNotDurable { .. }
        ));
        assert_eq!(store.snapshot(), candidate);
        let mut reloaded = AttachmentStore::new(store_path);
        reloaded
            .load()
            .await
            .expect("reload already-published candidate");
        assert_eq!(reloaded.snapshot(), candidate);
    }

    #[tokio::test]
    async fn prepublication_failure_preserves_memory_and_cleans_the_temporary_file() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        std::fs::create_dir(&store_path).expect("block replacement with a directory");
        let baseline_project = dir.path().join("baseline");
        let candidate_project = dir.path().join("candidate");
        let mut store = AttachmentStore::new(store_path);
        store.mark_stopped(&baseline_project);
        let baseline = store.snapshot();
        let mut candidate = AttachmentStoreSnapshot::default();
        candidate.replace_project(
            &candidate_project,
            vec![Attachment {
                project_path: candidate_project.clone(),
                source: AttachmentSource::Pin,
                created_at: UNIX_EPOCH + Duration::from_secs(5),
            }],
            false,
        );

        let error = store
            .replace_snapshot(candidate)
            .await
            .expect_err("directory destination must reject atomic replacement");

        assert!(matches!(error, AttachmentStoreError::Io { .. }));
        assert_eq!(store.snapshot(), baseline);
        assert!(
            std::fs::read_dir(dir.path())
                .expect("read store directory")
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
    }

    #[tokio::test]
    async fn parked_runtime_hold_survives_load_and_save_without_blocking_attach() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let project_text = project.to_string_lossy();
        let legacy = serde_json::json!({
            "attachments": {
                project_text.as_ref(): [{
                    "project_path": project,
                    "source": { "Runtime": { "token": "opaque" } },
                    "created_at": {
                        "secs_since_epoch": 1,
                        "nanos_since_epoch": 0
                    }
                }]
            },
            "manually_stopped": []
        });
        tokio::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy attachment"),
        )
        .await
        .expect("write legacy attachment store");

        let mut store = AttachmentStore::new(store_path.clone());
        store.load().await.expect("load legacy runtime hold");
        assert_eq!(store.section_for(&project), ProjectSection::Recent);
        assert!(matches!(
            store.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));
        assert!(
            store
                .attach(Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::CLI {
                        pid: std::process::id(),
                    },
                    created_at: SystemTime::now(),
                })
                .expect("attach CLI beside parked Runtime evidence")
        );
        store.save().await.expect("save compatible store");

        let mut reloaded = AttachmentStore::new(store_path);
        reloaded.load().await.expect("reload compatible store");
        let sources: Vec<_> = reloaded
            .attachments_for(&project)
            .into_iter()
            .map(|attachment| &attachment.source)
            .collect();
        assert!(
            sources
                .iter()
                .any(|source| matches!(source, AttachmentSource::Runtime))
        );
        assert!(sources.iter().any(|source| matches!(
            source,
            AttachmentSource::CLI { pid } if *pid == std::process::id()
        )));

        assert!(reloaded.detach(
            &project,
            &AttachmentSource::CLI {
                pid: std::process::id(),
            },
        ));
        assert_eq!(reloaded.attachments_for(&project).len(), 1);
        assert!(matches!(
            reloaded.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));

        assert!(
            reloaded
                .attach(Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::CLI {
                        pid: std::process::id(),
                    },
                    created_at: SystemTime::now(),
                })
                .expect("reattach CLI beside parked Runtime evidence")
        );
        assert!(reloaded.detach_all_non_pin(&project));
        assert!(matches!(
            reloaded.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));

        let error = reloaded
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Runtime,
                created_at: SystemTime::now(),
            })
            .expect_err("live Runtime attachments are migration-owned");
        assert!(error.to_string().contains("cannot be created"));

        assert!(
            reloaded
                .attach(Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::Pin,
                    created_at: SystemTime::now(),
                })
                .expect("attach pin beside parked Runtime evidence")
        );
        reloaded.mark_stopped(&project);
        assert!(reloaded.forget_project(&project));
        assert!(reloaded.attachments_for(&project).is_empty());
        assert!(!reloaded.is_stopped(&project));
        assert!(!reloaded.all_projects().contains(&project));
    }

    #[tokio::test]
    async fn unknown_legacy_source_is_ignored_for_lifecycle_state() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let project_text = project.to_string_lossy();
        let legacy = serde_json::json!({
            "attachments": {
                project_text.as_ref(): [{
                    "project_path": project,
                    "source": { "ExperimentalOwner": { "token": "opaque" } },
                    "created_at": {
                        "secs_since_epoch": 1,
                        "nanos_since_epoch": 0
                    }
                }]
            },
            "manually_stopped": [project]
        });
        tokio::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy attachment"),
        )
        .await
        .expect("write legacy attachment store");

        let mut store = AttachmentStore::new(store_path.clone());
        store.load().await.expect("load unknown legacy source");

        assert!(store.attachments_for(&project).is_empty());
        assert_eq!(store.section_for(&project), ProjectSection::Recent);
        assert!(store.is_stopped(&project));

        store.save().await.expect("save compatible store");
        let mut reloaded = AttachmentStore::new(store_path);
        reloaded
            .load_exact()
            .await
            .expect("reload normalized exact compatibility store");
        assert!(reloaded.attachments_for(&project).is_empty());
        assert!(reloaded.is_stopped(&project));
        assert!(!reloaded.snapshot().attachments.contains_key(&project));
    }

    #[tokio::test]
    async fn parked_runtime_evidence_does_not_mask_a_reaped_last_owner() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let project_text = project.to_string_lossy();
        let runtime_created_at = SystemTime::now();
        let legacy = serde_json::json!({
            "attachments": {
                project_text.as_ref(): [{
                    "project_path": project,
                    "source": "Runtime",
                    "created_at": runtime_created_at
                }]
            },
            "manually_stopped": []
        });
        tokio::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy attachment"),
        )
        .await
        .expect("write legacy attachment store");

        let mut store = AttachmentStore::new(store_path);
        store.load().await.expect("load legacy Runtime evidence");
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: u32::MAX },
                created_at: SystemTime::now(),
            })
            .expect("attach dead CLI");

        let orphaned = store.reap_stale_attachments();

        assert_eq!(orphaned, vec![normalized_locator(&project)]);
        assert_eq!(store.attachments_for(&project).len(), 1);
        assert!(matches!(
            store.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));
    }

    #[test]
    fn serialization_round_trip() {
        let dir = tempdir().unwrap();
        let attachment = Attachment {
            project_path: dir.path().join("project"),
            source: AttachmentSource::Editor {
                name: "vscode".to_string(),
                id: "abc".to_string(),
                pid: Some(std::process::id()),
            },
            created_at: SystemTime::now(),
        };

        let json = serde_json::to_string(&attachment).unwrap();
        let decoded: Attachment = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.project_path, attachment.project_path);
        assert_eq!(decoded.source, attachment.source);
    }

    #[test]
    fn manual_cli_session_provenance_round_trips() {
        let dir = tempdir().unwrap();
        let session = ManualCliSession::new(42);
        let attachment = Attachment {
            project_path: dir.path().join("project"),
            source: session.attachment_source(),
            created_at: UNIX_EPOCH + Duration::from_secs(10),
        };

        let json = serde_json::to_string(&attachment).unwrap();
        let decoded: Attachment = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, attachment);
    }

    #[test]
    fn legacy_editor_json_deserializes_without_pid() {
        let json = r#"
{
    "project_path": "/tmp/project",
    "source": {
        "Editor": {
            "name": "vscode",
            "id": "abc"
        }
    },
    "created_at": {
        "secs_since_epoch": 1,
        "nanos_since_epoch": 0
    }
}
"#;

        let decoded: Attachment = serde_json::from_str(json).unwrap();

        assert_eq!(
            decoded.source,
            AttachmentSource::Editor {
                name: "vscode".to_string(),
                id: "abc".to_string(),
                pid: None,
            }
        );
    }
}
