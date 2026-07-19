//! Durable desired-availability state for one project instance.
//!
//! Availability is daemon-owned intent. It is keyed by stable
//! [`ProjectInstanceId`] rather than a path and is persisted separately from
//! both project configuration and process snapshots.

use crate::ProjectInstanceId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// The current on-disk availability schema.
pub const AVAILABILITY_VERSION: u32 = 1;

/// How long an explicit CLI demand remains live without renewal.
pub const MANUAL_DEMAND_TTL: Duration = Duration::from_hours(4);

/// The expected cadence for VS Code demand renewal.
pub const VSCODE_RENEWAL_INTERVAL: Duration = Duration::from_secs(30);

/// How long a VS Code demand remains live without renewal.
pub const VSCODE_DEMAND_TTL: Duration = Duration::from_mins(2);

/// The active portion of an agent-conversation demand.
pub const AGENT_ACTIVE_TTL: Duration = Duration::from_mins(15);

/// The review grace retained after an agent's active lease.
pub const AGENT_REVIEW_GRACE: Duration = Duration::from_mins(30);

/// How long an agent-conversation demand remains live without renewal.
pub const AGENT_DEMAND_TTL: Duration = Duration::from_mins(45);

/// The default delay before stopping after the final live demand disappears.
pub const SHUTDOWN_COOLDOWN: Duration = Duration::from_mins(2);

const AVAILABILITY_FILE_NAME: &str = "availability.json";
const OWNER_DIGEST_DOMAIN: &[u8] = b"locald-demand-owner-v1\0";

/// A source of wall-clock time for restart-stable lease deadlines.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The production availability clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)]
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// The semantic category of a renewable availability demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemandKind {
    ManualCli,
    VsCodeWindow,
    AgentConversation,
    LegacyProcessAttachment,
    StoppedPageResume,
}

impl DemandKind {
    /// A privacy-safe label suitable for normal status projections.
    #[must_use]
    pub const fn safe_label(self) -> &'static str {
        match self {
            Self::ManualCli => "Manual CLI",
            Self::VsCodeWindow => "VS Code window",
            Self::AgentConversation => "Agent conversation",
            Self::LegacyProcessAttachment => "Legacy process attachment",
            Self::StoppedPageResume => "Stopped-page resume",
        }
    }

    const fn lease_duration(self) -> Option<Duration> {
        match self {
            Self::ManualCli | Self::StoppedPageResume => Some(MANUAL_DEMAND_TTL),
            Self::VsCodeWindow => Some(VSCODE_DEMAND_TTL),
            Self::AgentConversation => Some(AGENT_DEMAND_TTL),
            Self::LegacyProcessAttachment => None,
        }
    }

    const fn requires_owner(self) -> bool {
        matches!(
            self,
            Self::VsCodeWindow | Self::AgentConversation | Self::LegacyProcessAttachment
        )
    }

    const fn persistence_tag(self) -> &'static [u8] {
        match self {
            Self::ManualCli => b"manual_cli",
            Self::VsCodeWindow => b"vs_code_window",
            Self::AgentConversation => b"agent_conversation",
            Self::LegacyProcessAttachment => b"legacy_process_attachment",
            Self::StoppedPageResume => b"stopped_page_resume",
        }
    }
}

/// A validation failure while constructing a demand key from private host data.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DemandKeyError {
    #[error("{kind:?} demand identity must not be empty")]
    EmptyPrivateIdentity { kind: DemandKind },
}

/// A stable, privacy-preserving demand-owner digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
struct OpaqueDemandOwner(String);

impl OpaqueDemandOwner {
    fn digest(kind: DemandKind, private_identity: &str) -> Result<Self, DemandKeyError> {
        if private_identity.trim().is_empty() {
            return Err(DemandKeyError::EmptyPrivateIdentity { kind });
        }

        let mut hasher = Sha256::new();
        hasher.update(OWNER_DIGEST_DOMAIN);
        hasher.update(kind.persistence_tag());
        hasher.update([0]);
        hasher.update(private_identity.as_bytes());
        Ok(Self(format!("{:x}", hasher.finalize())))
    }

    fn is_valid(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// The stable key for one independent availability demand.
///
/// Private editor, conversation, and compatibility identities are hashed
/// before they enter durable state. Normal status surfaces expose only
/// [`DemandKind`] and [`DemandKind::safe_label`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemandKey {
    kind: DemandKind,
    owner: Option<OpaqueDemandOwner>,
}

impl DemandKey {
    /// The singleton manual CLI demand used by `locald up`.
    #[must_use]
    pub const fn manual_cli() -> Self {
        Self {
            kind: DemandKind::ManualCli,
            owner: None,
        }
    }

    /// A demand owned by one trusted VS Code window identity.
    pub fn vs_code_window(private_identity: &str) -> Result<Self, DemandKeyError> {
        Self::owned(DemandKind::VsCodeWindow, private_identity)
    }

    /// A demand owned by one private agent-conversation identity.
    pub fn agent_conversation(private_identity: &str) -> Result<Self, DemandKeyError> {
        Self::owned(DemandKind::AgentConversation, private_identity)
    }

    /// A process-bound compatibility demand imported from legacy state.
    pub fn legacy_process_attachment(private_identity: &str) -> Result<Self, DemandKeyError> {
        Self::owned(DemandKind::LegacyProcessAttachment, private_identity)
    }

    /// The singleton demand created by an explicit stopped-page Resume action.
    #[must_use]
    pub const fn stopped_page_resume() -> Self {
        Self {
            kind: DemandKind::StoppedPageResume,
            owner: None,
        }
    }

    /// Return the privacy-safe demand category.
    #[must_use]
    pub const fn kind(&self) -> DemandKind {
        self.kind
    }

    /// Return the privacy-safe label for normal status projections.
    #[must_use]
    pub const fn safe_label(&self) -> &'static str {
        self.kind.safe_label()
    }

    fn owned(kind: DemandKind, private_identity: &str) -> Result<Self, DemandKeyError> {
        Ok(Self {
            kind,
            owner: Some(OpaqueDemandOwner::digest(kind, private_identity)?),
        })
    }

    fn validate(&self) -> Result<(), String> {
        if self.kind.requires_owner() != self.owner.is_some() {
            return Err(format!(
                "{:?} demand has an invalid owner representation",
                self.kind
            ));
        }
        if self.owner.as_ref().is_some_and(|owner| !owner.is_valid()) {
            return Err(format!(
                "{:?} demand owner is not a canonical SHA-256 digest",
                self.kind
            ));
        }
        Ok(())
    }
}

/// One independently renewable reason for keeping a project instance up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemandLease {
    key: DemandKey,
    generation: u64,
    acquired_at: SystemTime,
    renewed_at: SystemTime,
    expires_at: Option<SystemTime>,
}

impl DemandLease {
    #[must_use]
    pub const fn key(&self) -> &DemandKey {
        &self.key
    }

    #[must_use]
    pub const fn kind(&self) -> DemandKind {
        self.key.kind()
    }

    #[must_use]
    pub const fn safe_label(&self) -> &'static str {
        self.key.safe_label()
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn acquired_at(&self) -> SystemTime {
        self.acquired_at
    }

    #[must_use]
    pub const fn renewed_at(&self) -> SystemTime {
        self.renewed_at
    }

    #[must_use]
    pub const fn expires_at(&self) -> Option<SystemTime> {
        self.expires_at
    }

    /// Whether this demand is live at the supplied wall-clock time.
    #[must_use]
    pub fn is_live_at(&self, now: SystemTime) -> bool {
        self.expires_at.is_none_or(|expires_at| expires_at > now)
    }

    fn new(key: DemandKey, generation: u64, now: SystemTime) -> Result<Self, String> {
        let expires_at = deadline(now, key.kind().lease_duration())?;
        Ok(Self {
            key,
            generation,
            acquired_at: now,
            renewed_at: now,
            expires_at,
        })
    }

    fn renew(&mut self, now: SystemTime) -> Result<(), String> {
        let effective_now = now.max(self.renewed_at);
        self.renewed_at = effective_now;
        self.expires_at = deadline(effective_now, self.kind().lease_duration())?;
        Ok(())
    }

    fn validate(&self, activity_generation: u64) -> Result<(), String> {
        self.key.validate()?;
        if self.generation == 0 || self.generation > activity_generation {
            return Err(format!(
                "{:?} demand generation {} is outside 1..={activity_generation}",
                self.kind(),
                self.generation
            ));
        }
        if self.renewed_at < self.acquired_at {
            return Err(format!(
                "{:?} demand renewal predates acquisition",
                self.kind()
            ));
        }
        let expected_expiry = deadline(self.renewed_at, self.kind().lease_duration())?;
        if self.expires_at != expected_expiry {
            return Err(format!(
                "{:?} demand expiry does not match its canonical lease duration",
                self.kind()
            ));
        }
        Ok(())
    }
}

/// Durable desired-availability inputs for one project instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAvailability {
    activity_generation: u64,
    always_on: bool,
    pause_through_generation: Option<u64>,
    demands: Vec<DemandLease>,
    shutdown_cooldown_until: Option<SystemTime>,
    trusted_launch_path: Option<String>,
    last_convergence_error: Option<String>,
}

impl ProjectAvailability {
    #[must_use]
    pub const fn activity_generation(&self) -> u64 {
        self.activity_generation
    }

    #[must_use]
    pub const fn always_on(&self) -> bool {
        self.always_on
    }

    #[must_use]
    pub const fn pause_through_generation(&self) -> Option<u64> {
        self.pause_through_generation
    }

    #[must_use]
    pub fn demands(&self) -> &[DemandLease] {
        &self.demands
    }

    #[must_use]
    pub const fn shutdown_cooldown_until(&self) -> Option<SystemTime> {
        self.shutdown_cooldown_until
    }

    #[must_use]
    pub fn trusted_launch_path(&self) -> Option<&str> {
        self.trusted_launch_path.as_deref()
    }

    #[must_use]
    pub fn last_convergence_error(&self) -> Option<&str> {
        self.last_convergence_error.as_deref()
    }

    /// Whether the current activity generation is covered by a manual pause.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.pause_through_generation
            .is_some_and(|generation| generation >= self.activity_generation)
    }

    /// Iterate over the demands that remain live at `now`.
    pub fn live_demands_at(&self, now: SystemTime) -> impl Iterator<Item = &DemandLease> {
        self.demands
            .iter()
            .filter(move |demand| demand.is_live_at(now))
    }

    /// Derive desired availability without inspecting runtime process state.
    #[must_use]
    pub fn desired_up_at(&self, now: SystemTime) -> bool {
        !self.is_paused() && (self.always_on || self.live_demands_at(now).next().is_some())
    }

    fn ensure_demand(
        &mut self,
        key: DemandKey,
        now: SystemTime,
    ) -> Result<EnsureDemandResult, AvailabilityError> {
        let existing = self.demands.iter().position(|demand| demand.key == key);
        let existing_is_live = existing.is_some_and(|index| self.demands[index].is_live_at(now));

        if existing_is_live && !self.is_paused() {
            let index = existing.ok_or_else(|| AvailabilityError::Invariant {
                reason: "live demand index disappeared during renewal".to_owned(),
            })?;
            self.demands[index]
                .renew(now)
                .map_err(|reason| AvailabilityError::Invariant { reason })?;
            return Ok(EnsureDemandResult {
                effect: EnsureDemandEffect::Renewed,
                lease: self.demands[index].clone(),
            });
        }

        let was_paused = self.is_paused();
        let generation = self.activity_generation.checked_add(1).ok_or(
            AvailabilityError::GenerationExhausted {
                current: self.activity_generation,
            },
        )?;
        self.activity_generation = generation;
        self.demands.retain(|demand| demand.key != key);
        let lease = DemandLease::new(key, generation, now)
            .map_err(|reason| AvailabilityError::Invariant { reason })?;
        self.demands.push(lease.clone());
        self.demands.sort_by(|left, right| left.key.cmp(&right.key));

        Ok(EnsureDemandResult {
            effect: if was_paused {
                EnsureDemandEffect::Resumed
            } else {
                EnsureDemandEffect::Acquired
            },
            lease,
        })
    }

    fn renew_demand(
        &mut self,
        key: &DemandKey,
        now: SystemTime,
    ) -> Result<RenewDemandResult, AvailabilityError> {
        let Some(index) = self.demands.iter().position(|demand| &demand.key == key) else {
            return Ok(RenewDemandResult::Missing);
        };
        if !self.demands[index].is_live_at(now) {
            self.demands.remove(index);
            return Ok(RenewDemandResult::Expired);
        }
        self.demands[index]
            .renew(now)
            .map_err(|reason| AvailabilityError::Invariant { reason })?;
        Ok(RenewDemandResult::Renewed(self.demands[index].clone()))
    }

    fn release_demand(&mut self, key: &DemandKey) -> bool {
        let previous_len = self.demands.len();
        self.demands.retain(|demand| &demand.key != key);
        self.demands.len() != previous_len
    }

    fn expire_demands(&mut self, now: SystemTime) -> usize {
        let previous_len = self.demands.len();
        self.demands.retain(|demand| demand.is_live_at(now));
        previous_len - self.demands.len()
    }

    fn validate(&self) -> Result<(), String> {
        if self
            .pause_through_generation
            .is_some_and(|pause| pause > self.activity_generation)
        {
            return Err(format!(
                "pause generation exceeds activity generation {}",
                self.activity_generation
            ));
        }
        if self
            .trusted_launch_path
            .as_ref()
            .is_some_and(|path| path.contains('\0'))
        {
            return Err("trusted launch PATH contains a NUL byte".to_owned());
        }
        if self
            .last_convergence_error
            .as_ref()
            .is_some_and(|error| error.trim().is_empty())
        {
            return Err("last convergence error must not be empty".to_owned());
        }

        let mut keys = BTreeSet::new();
        for demand in &self.demands {
            demand.validate(self.activity_generation)?;
            if !keys.insert(&demand.key) {
                return Err(format!("duplicate {:?} demand", demand.kind()));
            }
        }
        Ok(())
    }
}

/// The effect of an explicit semantic ensure operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureDemandEffect {
    /// A new owner or a renewed owner after expiry started a generation.
    Acquired,
    /// A still-live owner renewed its lease in the current generation.
    Renewed,
    /// Explicit activity advanced beyond the current pause barrier.
    Resumed,
}

/// The result of explicitly ensuring one demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureDemandResult {
    pub effect: EnsureDemandEffect,
    pub lease: DemandLease,
}

/// The result of passively renewing an existing demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewDemandResult {
    Renewed(DemandLease),
    Missing,
    Expired,
}

/// A load, transition, or persistence failure in authoritative availability state.
#[derive(Debug, Error)]
pub enum AvailabilityError {
    #[error("failed to {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("availability `{path}` uses unsupported schema version {found}; expected {expected}")]
    UnsupportedVersion {
        path: PathBuf,
        found: u64,
        expected: u32,
    },

    #[error("invalid availability state `{path}`: {reason}")]
    InvalidData { path: PathBuf, reason: String },

    #[error("activity generation {current} cannot be advanced")]
    GenerationExhausted { current: u64 },

    #[error("availability invariant failed: {reason}")]
    Invariant { reason: String },

    #[error("availability `{path}` was published and its parent-directory sync failed: {reason}")]
    PublishedNotDurable { path: PathBuf, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AvailabilityFile {
    version: u32,
    project_instance_id: ProjectInstanceId,
    availability: ProjectAvailability,
}

/// The authoritative availability record for one stable project instance.
#[derive(Debug)]
pub struct AvailabilityStore<C = SystemClock> {
    project_instance_id: ProjectInstanceId,
    path: PathBuf,
    availability: ProjectAvailability,
    clock: C,
}

impl AvailabilityStore<SystemClock> {
    /// Load one project instance from the standard layout beneath `data_dir`.
    pub async fn load(
        data_dir: &Path,
        project_instance_id: ProjectInstanceId,
    ) -> Result<Self, AvailabilityError> {
        Self::load_with_clock(data_dir, project_instance_id, SystemClock).await
    }
}

impl<C: Clock> AvailabilityStore<C> {
    /// Load one project instance with an injected clock.
    pub async fn load_with_clock(
        data_dir: &Path,
        project_instance_id: ProjectInstanceId,
        clock: C,
    ) -> Result<Self, AvailabilityError> {
        let path = availability_path(data_dir, project_instance_id);
        let availability = load_availability(&path, project_instance_id).await?;
        Ok(Self {
            project_instance_id,
            path,
            availability,
            clock,
        })
    }

    #[must_use]
    pub const fn project_instance_id(&self) -> ProjectInstanceId {
        self.project_instance_id
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn availability(&self) -> &ProjectAvailability {
        &self.availability
    }

    /// Explicitly acquire, renew, or resume one demand.
    pub async fn ensure_demand(
        &mut self,
        key: DemandKey,
    ) -> Result<EnsureDemandResult, AvailabilityError> {
        let now = self.clock.now();
        let mut candidate = self.availability.clone();
        let result = candidate.ensure_demand(key, now)?;
        self.commit(candidate).await?;
        Ok(result)
    }

    /// Passively renew a live demand without advancing or resuming a generation.
    pub async fn renew_demand(
        &mut self,
        key: &DemandKey,
    ) -> Result<RenewDemandResult, AvailabilityError> {
        let now = self.clock.now();
        let mut candidate = self.availability.clone();
        let result = candidate.renew_demand(key, now)?;
        if !matches!(result, RenewDemandResult::Missing) {
            self.commit(candidate).await?;
        }
        Ok(result)
    }

    /// Release one owner while preserving every other demand.
    pub async fn release_demand(&mut self, key: &DemandKey) -> Result<bool, AvailabilityError> {
        let mut candidate = self.availability.clone();
        let released = candidate.release_demand(key);
        if released {
            self.commit(candidate).await?;
        }
        Ok(released)
    }

    /// Remove every demand whose absolute deadline has been reached.
    pub async fn expire_demands(&mut self) -> Result<usize, AvailabilityError> {
        let now = self.clock.now();
        let mut candidate = self.availability.clone();
        let expired = candidate.expire_demands(now);
        if expired > 0 {
            self.commit(candidate).await?;
        }
        Ok(expired)
    }

    /// Derive desired availability using one clock observation.
    #[must_use]
    pub fn desired_up(&self) -> bool {
        self.availability.desired_up_at(self.clock.now())
    }

    async fn commit(&mut self, candidate: ProjectAvailability) -> Result<(), AvailabilityError> {
        self.commit_with_parent_sync(candidate, |path| async move { sync_parent(&path).await })
            .await
    }

    async fn commit_with_parent_sync<Sync, SyncFuture>(
        &mut self,
        candidate: ProjectAvailability,
        parent_sync: Sync,
    ) -> Result<(), AvailabilityError>
    where
        Sync: FnOnce(PathBuf) -> SyncFuture,
        SyncFuture: Future<Output = Result<(), AvailabilityError>>,
    {
        candidate
            .validate()
            .map_err(|reason| AvailabilityError::InvalidData {
                path: self.path.clone(),
                reason,
            })?;
        let result = replace_availability_with_parent_sync(
            &candidate,
            self.project_instance_id,
            &self.path,
            parent_sync,
        )
        .await;
        if result.is_ok() || matches!(&result, Err(AvailabilityError::PublishedNotDurable { .. })) {
            self.availability = candidate;
        }
        result
    }
}

/// Return the standard per-instance availability path beneath `data_dir`.
#[must_use]
pub fn availability_path(data_dir: &Path, project_instance_id: ProjectInstanceId) -> PathBuf {
    data_dir
        .join("instances")
        .join(project_instance_id.to_string())
        .join(AVAILABILITY_FILE_NAME)
}

async fn load_availability(
    path: &Path,
    project_instance_id: ProjectInstanceId,
) -> Result<ProjectAvailability, AvailabilityError> {
    let content = match tokio::fs::read(path).await {
        Ok(content) => content,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(ProjectAvailability::default());
        }
        Err(source) => {
            return Err(AvailabilityError::Io {
                operation: "read availability state",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let value: serde_json::Value =
        serde_json::from_slice(&content).map_err(|source| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
    let found = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: "missing unsigned integer schema version".to_owned(),
        })?;
    if found != u64::from(AVAILABILITY_VERSION) {
        return Err(AvailabilityError::UnsupportedVersion {
            path: path.to_path_buf(),
            found,
            expected: AVAILABILITY_VERSION,
        });
    }

    let file: AvailabilityFile =
        serde_json::from_value(value).map_err(|source| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
    if file.project_instance_id != project_instance_id {
        return Err(AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: format!(
                "embedded project instance {} does not match path owner {project_instance_id}",
                file.project_instance_id
            ),
        });
    }
    file.availability
        .validate()
        .map_err(|reason| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason,
        })?;
    Ok(file.availability)
}

async fn replace_availability_with_parent_sync<Sync, SyncFuture>(
    availability: &ProjectAvailability,
    project_instance_id: ProjectInstanceId,
    path: &Path,
    parent_sync: Sync,
) -> Result<(), AvailabilityError>
where
    Sync: FnOnce(PathBuf) -> SyncFuture,
    SyncFuture: Future<Output = Result<(), AvailabilityError>>,
{
    let temporary = write_temporary_availability(availability, project_instance_id, path).await?;
    if let Err(source) = tokio::fs::rename(&temporary, path).await {
        let cleanup = tokio::fs::remove_file(&temporary).await;
        let reason = cleanup.err().map_or_else(
            || source.to_string(),
            |cleanup_error| format!("{source}; temporary cleanup also failed: {cleanup_error}"),
        );
        return Err(AvailabilityError::Io {
            operation: "replace availability state",
            path: path.to_path_buf(),
            source: io::Error::new(source.kind(), reason),
        });
    }
    parent_sync(path.to_path_buf())
        .await
        .map_err(|error| AvailabilityError::PublishedNotDurable {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })
}

async fn write_temporary_availability(
    availability: &ProjectAvailability,
    project_instance_id: ProjectInstanceId,
    path: &Path,
) -> Result<PathBuf, AvailabilityError> {
    let parent = path
        .parent()
        .ok_or_else(|| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: "availability path has no parent directory".to_owned(),
        })?;
    let first_publish = match tokio::fs::symlink_metadata(path).await {
        Ok(_) => false,
        Err(source) if source.kind() == io::ErrorKind::NotFound => true,
        Err(source) => {
            return Err(AvailabilityError::Io {
                operation: "inspect availability state before publication",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|source| AvailabilityError::Io {
            operation: "create availability directory",
            path: parent.to_path_buf(),
            source,
        })?;
    if first_publish {
        sync_new_availability_hierarchy(path).await?;
    }

    let temporary = parent.join(format!(".{AVAILABILITY_FILE_NAME}.{}.tmp", Uuid::new_v4()));
    let file = AvailabilityFile {
        version: AVAILABILITY_VERSION,
        project_instance_id,
        availability: availability.clone(),
    };
    let mut content =
        serde_json::to_vec_pretty(&file).map_err(|source| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
    content.push(b'\n');

    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| AvailabilityError::Io {
            operation: "create temporary availability state",
            path: temporary.clone(),
            source,
        })?;
    let write_result = async {
        output.write_all(&content).await?;
        output.sync_all().await
    }
    .await;
    if let Err(source) = write_result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(AvailabilityError::Io {
            operation: "write and sync temporary availability state",
            path: temporary,
            source,
        });
    }
    Ok(temporary)
}

async fn sync_parent(path: &Path) -> Result<(), AvailabilityError> {
    let parent = path
        .parent()
        .ok_or_else(|| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: "availability path has no parent directory".to_owned(),
        })?;
    sync_directory(parent).await
}

async fn sync_new_availability_hierarchy(path: &Path) -> Result<(), AvailabilityError> {
    let project_state_directory = path
        .parent()
        .ok_or_else(|| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: "availability path has no instance directory".to_owned(),
        })?;
    let instances_root =
        project_state_directory
            .parent()
            .ok_or_else(|| AvailabilityError::InvalidData {
                path: path.to_path_buf(),
                reason: "availability path has no instances directory".to_owned(),
            })?;
    let data_directory = instances_root
        .parent()
        .ok_or_else(|| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: "availability path has no data directory".to_owned(),
        })?;
    let data_parent = data_directory
        .parent()
        .ok_or_else(|| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: "availability data directory has no parent".to_owned(),
        })?;

    // A successful first publication must make every newly introduced
    // directory entry durable, not only the final availability.json entry.
    // Syncing all three parents also repairs a hierarchy left by an earlier
    // failed first-publication attempt.
    sync_directory(instances_root).await?;
    sync_directory(data_directory).await?;
    sync_directory(data_parent).await
}

async fn sync_directory(path: &Path) -> Result<(), AvailabilityError> {
    let directory = tokio::fs::File::open(path)
        .await
        .map_err(|source| AvailabilityError::Io {
            operation: "open availability directory for sync",
            path: path.to_path_buf(),
            source,
        })?;
    directory
        .sync_all()
        .await
        .map_err(|source| AvailabilityError::Io {
            operation: "sync availability directory",
            path: path.to_path_buf(),
            source,
        })
}

fn deadline(now: SystemTime, duration: Option<Duration>) -> Result<Option<SystemTime>, String> {
    duration
        .map(|duration| {
            now.checked_add(duration)
                .ok_or_else(|| "demand expiry exceeds SystemTime range".to_owned())
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    const START_SECONDS: u64 = 1_000_000;

    #[derive(Debug, Clone)]
    struct FakeClock {
        seconds: Arc<AtomicU64>,
    }

    impl FakeClock {
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

    impl Clock for FakeClock {
        fn now(&self) -> SystemTime {
            self.time()
        }
    }

    struct Fixture {
        _temp: TempDir,
        data_dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("create availability fixture");
            let data_dir = temp.path().join("data");
            Self {
                _temp: temp,
                data_dir,
            }
        }
    }

    fn instance_id(value: u128) -> ProjectInstanceId {
        ProjectInstanceId::from_str(&Uuid::from_u128(value).to_string())
            .expect("parse fixture project instance id")
    }

    async fn fake_store(
        fixture: &Fixture,
        project_instance_id: ProjectInstanceId,
        clock: FakeClock,
    ) -> AvailabilityStore<FakeClock> {
        AvailabilityStore::load_with_clock(&fixture.data_dir, project_instance_id, clock)
            .await
            .expect("load availability store")
    }

    fn temporary_files(path: &Path) -> Vec<PathBuf> {
        let Some(parent) = path.parent() else {
            return Vec::new();
        };
        std::fs::read_dir(parent)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|entry| {
                        entry
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                Path::new(name)
                                    .extension()
                                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
                            })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn demand_keys_digest_private_identity_and_expose_safe_labels() {
        let private_identity = "private-conversation-123";
        let key = DemandKey::agent_conversation(private_identity)
            .expect("construct agent conversation demand");
        let serialized = serde_json::to_string(&key).expect("serialize demand key");

        assert_eq!(key.kind(), DemandKind::AgentConversation);
        assert_eq!(key.safe_label(), "Agent conversation");
        assert!(!serialized.contains(private_identity));
        assert_ne!(
            key,
            DemandKey::agent_conversation("private-conversation-456")
                .expect("construct second agent conversation demand")
        );
        assert_eq!(
            DemandKey::vs_code_window(" ").expect_err("reject empty private identity"),
            DemandKeyError::EmptyPrivateIdentity {
                kind: DemandKind::VsCodeWindow
            }
        );
        assert_eq!(AGENT_DEMAND_TTL, AGENT_ACTIVE_TTL + AGENT_REVIEW_GRACE);
    }

    #[test]
    fn lease_validation_enforces_canonical_deadline() {
        let now = UNIX_EPOCH + Duration::from_secs(START_SECONDS);
        let mut lease =
            DemandLease::new(DemandKey::manual_cli(), 1, now).expect("construct manual lease");
        lease.expires_at = Some(now + Duration::from_secs(1));

        assert!(lease.validate(1).is_err());
    }

    #[tokio::test]
    async fn explicit_ensure_acquires_then_renews_a_live_owner() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(1), clock.clone()).await;
        let key = DemandKey::manual_cli();

        let acquired = store
            .ensure_demand(key.clone())
            .await
            .expect("acquire manual demand");
        assert_eq!(acquired.effect, EnsureDemandEffect::Acquired);
        assert_eq!(acquired.lease.generation(), 1);
        assert_eq!(
            acquired.lease.expires_at(),
            Some(clock.time() + MANUAL_DEMAND_TTL)
        );

        clock.advance(Duration::from_secs(10));
        let renewed = store
            .ensure_demand(key)
            .await
            .expect("renew live manual demand");
        assert_eq!(renewed.effect, EnsureDemandEffect::Renewed);
        assert_eq!(renewed.lease.generation(), 1);
        assert_eq!(renewed.lease.acquired_at(), acquired.lease.acquired_at());
        assert_eq!(renewed.lease.renewed_at(), clock.time());
        assert_eq!(store.availability().activity_generation(), 1);
    }

    #[tokio::test]
    async fn new_owner_advances_generation_and_demands_coexist() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(2), clock).await;

        store
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("acquire manual demand");
        let editor = store
            .ensure_demand(DemandKey::vs_code_window("window-1").expect("construct editor demand"))
            .await
            .expect("acquire editor demand");

        assert_eq!(editor.effect, EnsureDemandEffect::Acquired);
        assert_eq!(editor.lease.generation(), 2);
        assert_eq!(store.availability().activity_generation(), 2);
        assert_eq!(store.availability().demands().len(), 2);
        assert!(store.desired_up());
    }

    #[tokio::test]
    async fn passive_renewal_cannot_cross_a_pause_barrier() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(3), clock.clone()).await;
        let manual = DemandKey::manual_cli();

        store
            .ensure_demand(manual.clone())
            .await
            .expect("acquire manual demand");
        store.availability.pause_through_generation = Some(1);
        assert!(store.availability().is_paused());

        clock.advance(Duration::from_secs(20));
        let renewed = store
            .renew_demand(&manual)
            .await
            .expect("passively renew demand");
        let lease = match renewed {
            RenewDemandResult::Renewed(lease) => lease,
            other => return assert!(matches!(other, RenewDemandResult::Renewed(_))),
        };
        assert_eq!(lease.generation(), 1);
        assert_eq!(store.availability().activity_generation(), 1);
        assert!(store.availability().is_paused());
        assert!(!store.desired_up());

        let resumed = store
            .ensure_demand(manual)
            .await
            .expect("explicitly resume demand");
        assert_eq!(resumed.effect, EnsureDemandEffect::Resumed);
        assert_eq!(resumed.lease.generation(), 2);
        assert!(!store.availability().is_paused());
        assert!(store.desired_up());
    }

    #[tokio::test]
    async fn renewal_reports_missing_and_removes_expired_demand() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(4), clock.clone()).await;
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");

        assert_eq!(
            store
                .renew_demand(&editor)
                .await
                .expect("inspect missing renewal"),
            RenewDemandResult::Missing
        );
        assert!(!store.path().exists());

        store
            .ensure_demand(editor.clone())
            .await
            .expect("acquire editor demand");
        clock.advance(VSCODE_DEMAND_TTL);
        assert_eq!(
            store
                .renew_demand(&editor)
                .await
                .expect("remove expired renewal"),
            RenewDemandResult::Expired
        );
        assert!(store.availability().demands().is_empty());
        assert_eq!(store.availability().activity_generation(), 1);
    }

    #[tokio::test]
    async fn release_and_expiry_preserve_independent_owners() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(5), clock.clone()).await;
        let manual = DemandKey::manual_cli();
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");

        store
            .ensure_demand(manual.clone())
            .await
            .expect("acquire manual demand");
        store
            .ensure_demand(editor)
            .await
            .expect("acquire editor demand");
        clock.advance(VSCODE_DEMAND_TTL);

        assert_eq!(store.expire_demands().await.expect("expire demands"), 1);
        assert_eq!(store.availability().demands().len(), 1);
        assert!(store.release_demand(&manual).await.expect("release manual"));
        assert!(
            !store
                .release_demand(&manual)
                .await
                .expect("release missing manual")
        );
        assert!(store.availability().demands().is_empty());
        assert!(!store.desired_up());
    }

    #[test]
    fn desired_state_combines_always_on_pause_and_live_demands() {
        let now = UNIX_EPOCH + Duration::from_secs(START_SECONDS);
        let mut availability = ProjectAvailability::default();

        assert!(!availability.desired_up_at(now));
        availability.always_on = true;
        assert!(availability.desired_up_at(now));
        availability.pause_through_generation = Some(0);
        assert!(!availability.desired_up_at(now));
        availability.activity_generation = 1;
        assert!(availability.desired_up_at(now));
    }

    #[tokio::test]
    async fn persistence_round_trip_is_stable_and_instance_scoped() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let first_id = instance_id(6);
        let second_id = instance_id(7);
        let mut first = fake_store(&fixture, first_id, clock.clone()).await;
        let mut second = fake_store(&fixture, second_id, clock.clone()).await;

        first
            .ensure_demand(
                DemandKey::agent_conversation("conversation-1")
                    .expect("construct conversation demand"),
            )
            .await
            .expect("acquire conversation demand");
        let first_bytes = std::fs::read(first.path()).expect("read first availability state");
        assert_eq!(first_bytes.last(), Some(&b'\n'));

        second
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("acquire second instance demand");
        assert_eq!(
            std::fs::read(first.path()).expect("reread first availability state"),
            first_bytes
        );

        let reopened = fake_store(&fixture, first_id, clock).await;
        assert_eq!(reopened.availability(), first.availability());
        let unchanged = reopened.availability().clone();
        let mut reopened = reopened;
        reopened
            .commit(unchanged)
            .await
            .expect("republish unchanged availability");
        assert_eq!(
            std::fs::read(reopened.path()).expect("read republished availability state"),
            first_bytes
        );
    }

    #[tokio::test]
    async fn malformed_and_unsupported_state_remain_untouched() {
        let fixture = Fixture::new();
        let project_instance_id = instance_id(8);
        let path = availability_path(&fixture.data_dir, project_instance_id);
        std::fs::create_dir_all(path.parent().expect("availability parent"))
            .expect("create availability parent");

        for content in [b"{".as_slice(), br#"{"version":2}"#.as_slice()] {
            std::fs::write(&path, content).expect("write invalid availability state");
            let error = AvailabilityStore::load(&fixture.data_dir, project_instance_id)
                .await
                .expect_err("reject invalid availability state");
            assert!(matches!(
                error,
                AvailabilityError::InvalidData { .. }
                    | AvailabilityError::UnsupportedVersion { .. }
            ));
            assert_eq!(
                std::fs::read(&path).expect("read preserved invalid state"),
                content
            );
        }
    }

    #[tokio::test]
    async fn unknown_current_version_field_remains_untouched() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(14);
        let mut store = fake_store(&fixture, project_instance_id, clock).await;
        store
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("persist valid availability state");
        let mut value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(store.path()).expect("read valid availability state"),
        )
        .expect("parse valid availability state");
        value
            .as_object_mut()
            .expect("availability envelope is an object")
            .insert("unexpected".to_owned(), serde_json::Value::Bool(true));
        let mut content = serde_json::to_vec_pretty(&value).expect("serialize unknown field state");
        content.push(b'\n');
        std::fs::write(store.path(), &content).expect("write unknown field state");

        let error = AvailabilityStore::load(&fixture.data_dir, project_instance_id)
            .await
            .expect_err("reject unknown current-version field");
        assert!(matches!(error, AvailabilityError::InvalidData { .. }));
        assert_eq!(
            std::fs::read(store.path()).expect("read preserved unknown field state"),
            content
        );
    }

    #[tokio::test]
    async fn embedded_instance_mismatch_remains_untouched() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let first_id = instance_id(9);
        let second_id = instance_id(10);
        let mut first = fake_store(&fixture, first_id, clock).await;
        first
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("persist first instance");
        let content = std::fs::read(first.path()).expect("read first instance state");
        let second_path = availability_path(&fixture.data_dir, second_id);
        std::fs::create_dir_all(second_path.parent().expect("second availability parent"))
            .expect("create second availability parent");
        std::fs::write(&second_path, &content).expect("copy mismatched instance state");

        let error = AvailabilityStore::load(&fixture.data_dir, second_id)
            .await
            .expect_err("reject mismatched project instance");
        assert!(matches!(error, AvailabilityError::InvalidData { .. }));
        assert_eq!(
            std::fs::read(second_path).expect("read preserved mismatched state"),
            content
        );
    }

    #[tokio::test]
    async fn prepublication_failure_preserves_memory_and_cleans_temporary_file() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(11), clock).await;
        let before = store.availability().clone();
        std::fs::create_dir_all(store.path()).expect("occupy authoritative path with directory");

        let error = store
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect_err("fail before publishing availability");
        assert!(matches!(error, AvailabilityError::Io { .. }));
        assert_eq!(store.availability(), &before);
        assert!(temporary_files(store.path()).is_empty());
    }

    #[tokio::test]
    async fn parent_sync_failure_aligns_memory_with_published_state() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(12);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut candidate = store.availability().clone();
        candidate
            .ensure_demand(DemandKey::manual_cli(), clock.time())
            .expect("prepare availability candidate");

        let error = store
            .commit_with_parent_sync(candidate.clone(), |path| async move {
                Err(AvailabilityError::Io {
                    operation: "test parent sync",
                    path,
                    source: io::Error::other("injected parent sync failure"),
                })
            })
            .await
            .expect_err("report published but unsynced availability");
        assert!(matches!(
            error,
            AvailabilityError::PublishedNotDurable { .. }
        ));
        assert_eq!(store.availability(), &candidate);
        assert!(temporary_files(store.path()).is_empty());

        let reopened = fake_store(&fixture, project_instance_id, clock).await;
        assert_eq!(reopened.availability(), &candidate);
    }

    #[tokio::test]
    async fn generation_overflow_preserves_state_and_disk() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(13), clock).await;
        store.availability.activity_generation = u64::MAX;
        let before = store.availability().clone();

        let error = store
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect_err("reject exhausted generation");
        assert!(matches!(
            error,
            AvailabilityError::GenerationExhausted { current: u64::MAX }
        ));
        assert_eq!(store.availability(), &before);
        assert!(!store.path().exists());
    }
}
