//! Durable desired-availability state for one project instance.
//!
//! Availability is daemon-owned intent. It is keyed by stable
//! [`ProjectInstanceId`] rather than a path and is persisted separately from
//! both project configuration and process snapshots.

use crate::ProjectInstanceId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, Weak};
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;
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

/// How long a process-bound compatibility demand survives without revalidation.
pub const LEGACY_PROCESS_DEMAND_TTL: Duration = Duration::from_mins(2);

/// The default delay before stopping after the final live demand disappears.
pub const SHUTDOWN_COOLDOWN: Duration = Duration::from_mins(2);

const AVAILABILITY_FILE_NAME: &str = "availability.json";
const OWNER_DIGEST_DOMAIN: &[u8] = b"locald-demand-owner-v1\0";

type MutationLock = AsyncMutex<()>;

static AVAILABILITY_MUTATION_LOCKS: LazyLock<StdMutex<HashMap<PathBuf, Weak<MutationLock>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn availability_mutation_lock(path: &Path) -> Arc<MutationLock> {
    let mut locks = AVAILABILITY_MUTATION_LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }

    let lock = Arc::new(MutationLock::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

async fn normalized_data_directory(data_dir: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(data_dir)?;
    let mut existing_ancestor = absolute.as_path();

    loop {
        match tokio::fs::canonicalize(existing_ancestor).await {
            Ok(canonical_ancestor) => {
                let suffix = absolute.strip_prefix(existing_ancestor).map_err(|error| {
                    io::Error::other(format!(
                        "failed to derive availability path suffix: {error}"
                    ))
                })?;
                return Ok(lexically_normalize(&canonical_ancestor.join(suffix)));
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = existing_ancestor.parent() else {
                    return Err(source);
                };
                existing_ancestor = parent;
            }
            Err(source) => return Err(source),
        }
    }
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

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

    const fn lease_duration(self) -> Duration {
        match self {
            Self::ManualCli | Self::StoppedPageResume => MANUAL_DEMAND_TTL,
            Self::VsCodeWindow => VSCODE_DEMAND_TTL,
            Self::AgentConversation => AGENT_DEMAND_TTL,
            Self::LegacyProcessAttachment => LEGACY_PROCESS_DEMAND_TTL,
        }
    }

    const fn accepts_owner_state(self, has_owner: bool) -> bool {
        match self {
            Self::ManualCli => true,
            Self::VsCodeWindow | Self::AgentConversation | Self::LegacyProcessAttachment => {
                has_owner
            }
            Self::StoppedPageResume => !has_owner,
        }
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

    /// A Manual CLI demand owned by one retry-stable log-following session.
    pub fn manual_cli_session(private_identity: &str) -> Result<Self, DemandKeyError> {
        Self::owned(DemandKind::ManualCli, private_identity)
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
        if !self.kind.accepts_owner_state(self.owner.is_some()) {
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
        let expires_at = Some(deadline(now, key.kind().lease_duration())?);
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
        self.expires_at = Some(deadline(effective_now, self.kind().lease_duration())?);
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
        let expected_expiry = Some(deadline(self.renewed_at, self.kind().lease_duration())?);
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

    /// Iterate over live demands that are newer than the last pause barrier.
    ///
    /// A passive renewal may extend a suppressed lease, but only explicit
    /// semantic activity can move that owner into a later generation.
    pub fn effective_demands_at(&self, now: SystemTime) -> impl Iterator<Item = &DemandLease> {
        let pause_through_generation = self.pause_through_generation;
        self.live_demands_at(now).filter(move |demand| {
            pause_through_generation.is_none_or(|pause| demand.generation() > pause)
        })
    }

    /// Derive desired availability without inspecting runtime process state.
    #[must_use]
    pub fn desired_up_at(&self, now: SystemTime) -> bool {
        !self.is_paused() && (self.always_on || self.effective_demands_at(now).next().is_some())
    }

    /// Whether an already-running instance may remain alive until cooldown ends.
    ///
    /// This is a convergence deferral, not desired availability. In
    /// particular, a cooldown never authorizes starting or restoring a stopped
    /// instance after daemon restart.
    #[must_use]
    pub fn shutdown_deferred_at(&self, now: SystemTime) -> bool {
        !self.is_paused()
            && !self.desired_up_at(now)
            && self
                .shutdown_cooldown_until
                .is_some_and(|deadline| deadline > now)
    }

    fn convergence_decision_at(&self, now: SystemTime) -> ConvergenceDecision {
        if self.desired_up_at(now) {
            return ConvergenceDecision::EnsureUp;
        }
        if !self.is_paused()
            && let Some(deadline) = self.shutdown_cooldown_until
            && deadline > now
        {
            return ConvergenceDecision::PreserveRuntimeUntil { deadline };
        }
        ConvergenceDecision::EnsureDown
    }

    fn ensure_demand(
        &mut self,
        key: DemandKey,
        now: SystemTime,
    ) -> Result<EnsureDemandResult, AvailabilityError> {
        let existing = self.demands.iter().position(|demand| demand.key == key);
        let existing_is_live = existing.is_some_and(|index| self.demands[index].is_live_at(now));
        let existing_is_effective = existing_is_live
            && existing.is_some_and(|index| {
                self.pause_through_generation
                    .is_none_or(|pause| self.demands[index].generation > pause)
            });

        if existing_is_effective {
            let index = existing.ok_or_else(|| AvailabilityError::Invariant {
                reason: "live demand index disappeared during renewal".to_owned(),
            })?;
            self.demands[index]
                .renew(now)
                .map_err(|reason| AvailabilityError::Invariant { reason })?;
            self.shutdown_cooldown_until = None;
            return Ok(EnsureDemandResult {
                effect: EnsureDemandEffect::Renewed,
                lease: self.demands[index].clone(),
            });
        }

        let crossed_pause_barrier = self.is_paused() || existing_is_live;
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
        self.shutdown_cooldown_until = None;

        Ok(EnsureDemandResult {
            effect: if crossed_pause_barrier {
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
            let availability_loss_at = self
                .demands
                .iter()
                .filter(|demand| {
                    self.demand_generation_is_effective(demand.generation)
                        && !demand.is_live_at(now)
                })
                .filter_map(DemandLease::expires_at)
                .max();
            self.demands.remove(index);
            self.arm_shutdown_cooldown_if_idle(now, availability_loss_at)?;
            return Ok(RenewDemandResult::Expired);
        }
        self.demands[index]
            .renew(now)
            .map_err(|reason| AvailabilityError::Invariant { reason })?;
        if self.demand_generation_is_effective(self.demands[index].generation) {
            self.shutdown_cooldown_until = None;
        }
        Ok(RenewDemandResult::Renewed(self.demands[index].clone()))
    }

    fn revalidate_demand(
        &mut self,
        key: &DemandKey,
        now: SystemTime,
    ) -> Result<RenewDemandResult, AvailabilityError> {
        let Some(index) = self.demands.iter().position(|demand| &demand.key == key) else {
            return Ok(RenewDemandResult::Missing);
        };
        self.demands[index]
            .renew(now)
            .map_err(|reason| AvailabilityError::Invariant { reason })?;
        if self.demand_generation_is_effective(self.demands[index].generation) {
            self.shutdown_cooldown_until = None;
        }
        Ok(RenewDemandResult::Renewed(self.demands[index].clone()))
    }

    fn import_demand(
        &mut self,
        key: DemandKey,
        acquired_at: SystemTime,
        effective_at: SystemTime,
    ) -> Result<bool, AvailabilityError> {
        if self.demands.iter().any(|demand| demand.key == key) {
            return Ok(false);
        }

        let acquired_at = acquired_at.min(effective_at);
        let expires_at = deadline(acquired_at, key.kind().lease_duration())
            .map_err(|reason| AvailabilityError::Invariant { reason })?;
        if expires_at <= effective_at {
            return Ok(false);
        }

        let generation = self.activity_generation.checked_add(1).ok_or(
            AvailabilityError::GenerationExhausted {
                current: self.activity_generation,
            },
        )?;
        self.activity_generation = generation;
        let lease = DemandLease::new(key, generation, acquired_at)
            .map_err(|reason| AvailabilityError::Invariant { reason })?;
        self.demands.push(lease);
        self.demands.sort_by(|left, right| left.key.cmp(&right.key));
        self.shutdown_cooldown_until = None;
        Ok(true)
    }

    fn release_demand(
        &mut self,
        key: &DemandKey,
        now: SystemTime,
    ) -> Result<bool, AvailabilityError> {
        let removed_effective = self.demands.iter().find(|demand| {
            &demand.key == key && self.demand_generation_is_effective(demand.generation)
        });
        let availability_loss_at = removed_effective.and_then(|demand| {
            if demand.is_live_at(now) {
                Some(now)
            } else {
                self.demands
                    .iter()
                    .filter(|candidate| {
                        self.demand_generation_is_effective(candidate.generation)
                            && !candidate.is_live_at(now)
                    })
                    .filter_map(DemandLease::expires_at)
                    .max()
            }
        });
        let previous_len = self.demands.len();
        self.demands.retain(|demand| &demand.key != key);
        let released = self.demands.len() != previous_len;
        self.arm_shutdown_cooldown_if_idle(
            now,
            if released { availability_loss_at } else { None },
        )?;
        Ok(released)
    }

    fn expire_demands(&mut self, now: SystemTime) -> Result<usize, AvailabilityError> {
        let availability_loss_at = self
            .demands
            .iter()
            .filter(|demand| {
                !demand.is_live_at(now) && self.demand_generation_is_effective(demand.generation)
            })
            .filter_map(DemandLease::expires_at)
            .max();
        let previous_len = self.demands.len();
        self.demands.retain(|demand| demand.is_live_at(now));
        let expired = previous_len - self.demands.len();
        self.arm_shutdown_cooldown_if_idle(now, availability_loss_at)?;
        Ok(expired)
    }

    fn set_always_on(&mut self, enabled: bool, now: SystemTime) -> Result<bool, AvailabilityError> {
        if enabled {
            let activation_required = !self.always_on || self.is_paused();
            let cooldown_cleared = self.shutdown_cooldown_until.take().is_some();
            if activation_required {
                self.activity_generation = self.activity_generation.checked_add(1).ok_or(
                    AvailabilityError::GenerationExhausted {
                        current: self.activity_generation,
                    },
                )?;
            }
            let changed = !self.always_on || activation_required || cooldown_cleared;
            self.always_on = true;
            return Ok(changed);
        }

        if !self.always_on {
            return Ok(false);
        }
        self.always_on = false;
        self.arm_shutdown_cooldown_if_idle(now, Some(now))?;
        Ok(true)
    }

    fn pause_project(&mut self) -> bool {
        let pause_through_generation = Some(self.activity_generation);
        let changed = self.pause_through_generation != pause_through_generation
            || self.shutdown_cooldown_until.is_some();
        self.pause_through_generation = pause_through_generation;
        self.shutdown_cooldown_until = None;
        changed
    }

    fn set_trusted_launch_path(&mut self, path: Option<String>) -> bool {
        if self.trusted_launch_path == path {
            return false;
        }
        self.trusted_launch_path = path;
        true
    }

    fn set_last_convergence_error(&mut self, error: Option<String>) -> bool {
        if self.last_convergence_error == error {
            return false;
        }
        self.last_convergence_error = error;
        true
    }

    fn demand_generation_is_effective(&self, generation: u64) -> bool {
        self.pause_through_generation
            .is_none_or(|pause| generation > pause)
    }

    fn arm_shutdown_cooldown_if_idle(
        &mut self,
        now: SystemTime,
        availability_loss_at: Option<SystemTime>,
    ) -> Result<(), AvailabilityError> {
        let Some(availability_loss_at) = availability_loss_at else {
            return Ok(());
        };
        if self.shutdown_cooldown_until.is_some()
            || self.always_on
            || self.is_paused()
            || self.effective_demands_at(now).next().is_some()
        {
            return Ok(());
        }

        self.shutdown_cooldown_until = Some(
            deadline(availability_loss_at, SHUTDOWN_COOLDOWN)
                .map_err(|reason| AvailabilityError::Invariant { reason })?,
        );
        Ok(())
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

/// One ordered mutation in an availability transaction.
///
/// Batch operations are evaluated at the enclosing batch's fixed
/// [`AvailabilityBatch::effective_at`] time. A prepared batch stores the exact
/// resulting state, so journal replay publishes that state instead of
/// re-evaluating these operations against a later snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AvailabilityBatchOperation {
    /// Materialize an empty authoritative record when none exists yet.
    Initialize,
    /// Explicitly acquire, renew, or resume one semantic demand.
    EnsureDemand(DemandKey),
    /// Passively renew a live demand without crossing a pause barrier.
    RenewDemand(DemandKey),
    /// Revalidate a process-proven demand at its existing generation, even
    /// when its lease deadline elapsed. A demand removed by an earlier sweep
    /// remains absent because its generation provenance no longer exists.
    RevalidateDemand(DemandKey),
    /// Import one legacy hold with its original acquisition time. An already
    /// expired hold is ignored instead of receiving a fresh lease window.
    ImportDemand {
        key: DemandKey,
        acquired_at: SystemTime,
    },
    /// Release one demand owner.
    ReleaseDemand(DemandKey),
    /// Enable or disable durable Always On policy.
    SetAlwaysOn(bool),
    /// Pause the project through the generation current at this point in the
    /// ordered batch.
    PauseProject,
    /// Replace or clear trusted launch context.
    SetTrustedLaunchPath(Option<String>),
    /// Remove the authoritative availability record.
    Retire,
}

/// An ordered availability transaction evaluated at one fixed wall-clock time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvailabilityBatch {
    effective_at: SystemTime,
    operations: Vec<AvailabilityBatchOperation>,
}

impl AvailabilityBatch {
    /// Begin a batch whose time-dependent operations all use `effective_at`.
    #[must_use]
    pub const fn new(effective_at: SystemTime) -> Self {
        Self {
            effective_at,
            operations: Vec::new(),
        }
    }

    /// Append one operation, preserving caller order.
    pub fn push(&mut self, operation: AvailabilityBatchOperation) {
        self.operations.push(operation);
    }

    /// Append one operation and return the batch for fluent construction.
    #[must_use]
    pub fn with_operation(mut self, operation: AvailabilityBatchOperation) -> Self {
        self.push(operation);
        self
    }

    #[must_use]
    pub const fn effective_at(&self) -> SystemTime {
        self.effective_at
    }

    #[must_use]
    pub fn operations(&self) -> &[AvailabilityBatchOperation] {
        &self.operations
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

/// Exact authoritative state before or after a prepared availability batch.
///
/// Keeping absence distinct from a persisted default record gives
/// initialization and retirement deterministic, restart-stable semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "availability",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AvailabilityStateImage {
    /// No authoritative availability file exists.
    Retired,
    /// The authoritative file contains this validated state.
    Present(ProjectAvailability),
}

impl AvailabilityStateImage {
    #[must_use]
    pub const fn availability(&self) -> Option<&ProjectAvailability> {
        match self {
            Self::Retired => None,
            Self::Present(availability) => Some(availability),
        }
    }

    fn cached_availability(&self) -> ProjectAvailability {
        match self {
            Self::Retired => ProjectAvailability::default(),
            Self::Present(availability) => availability.clone(),
        }
    }
}

/// A journal-ready availability transaction with an exact compare-and-publish
/// contract.
///
/// Persist this payload before applying it. On replay, the store publishes
/// `target` only when the authoritative state still equals `expected`; if it
/// already equals `target`, replay is a no-op. Operations are never re-run
/// during replay, so deadlines and activity generations cannot drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedAvailabilityBatch {
    project_instance_id: ProjectInstanceId,
    batch: AvailabilityBatch,
    expected: AvailabilityStateImage,
    target: AvailabilityStateImage,
}

impl PreparedAvailabilityBatch {
    #[must_use]
    pub const fn project_instance_id(&self) -> ProjectInstanceId {
        self.project_instance_id
    }

    #[must_use]
    pub const fn batch(&self) -> &AvailabilityBatch {
        &self.batch
    }

    #[must_use]
    pub const fn expected(&self) -> &AvailabilityStateImage {
        &self.expected
    }

    #[must_use]
    pub const fn target(&self) -> &AvailabilityStateImage {
        &self.target
    }

    /// Validate that this journaled target is the exact result of replaying
    /// its ordered operations against the captured base image.
    ///
    /// This is intentionally pure: transaction journals can reject malformed
    /// or truncated prepared state before publishing any other authority.
    pub fn validate(&self) -> Result<(), AvailabilityError> {
        let validation_path = PathBuf::from(format!(
            "<prepared-availability:{}>",
            self.project_instance_id
        ));
        let recomputed = prepare_availability_batch(
            self.project_instance_id,
            self.expected.clone(),
            &self.batch,
            &validation_path,
        )?;
        if recomputed.target != self.target {
            return Err(AvailabilityError::InvalidData {
                path: validation_path,
                reason: "prepared availability target does not match its ordered operations"
                    .to_owned(),
            });
        }
        Ok(())
    }
}

/// Whether a prepared availability publication changed the authoritative
/// directory entry or observed that an earlier attempt already did so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityBatchDisposition {
    Published,
    AlreadyApplied,
}

/// Result of applying a direct or prepared availability batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityBatchApplyResult {
    disposition: AvailabilityBatchDisposition,
    target: AvailabilityStateImage,
}

impl AvailabilityBatchApplyResult {
    #[must_use]
    pub const fn disposition(&self) -> AvailabilityBatchDisposition {
        self.disposition
    }

    #[must_use]
    pub const fn target(&self) -> &AvailabilityStateImage {
        &self.target
    }
}

fn prepare_availability_batch(
    project_instance_id: ProjectInstanceId,
    expected: AvailabilityStateImage,
    batch: &AvailabilityBatch,
    path: &Path,
) -> Result<PreparedAvailabilityBatch, AvailabilityError> {
    validate_availability_image(&expected, path)?;
    let mut target = expected.clone();
    for operation in batch.operations() {
        validate_batch_operation(operation, path)?;
        apply_batch_operation(&mut target, operation, batch.effective_at())?;
        if let AvailabilityStateImage::Present(availability) = &target {
            availability
                .validate()
                .map_err(|reason| AvailabilityError::InvalidData {
                    path: path.to_path_buf(),
                    reason,
                })?;
        }
    }

    Ok(PreparedAvailabilityBatch {
        project_instance_id,
        batch: batch.clone(),
        expected,
        target,
    })
}

fn validate_batch_operation(
    operation: &AvailabilityBatchOperation,
    path: &Path,
) -> Result<(), AvailabilityError> {
    let demand = match operation {
        AvailabilityBatchOperation::EnsureDemand(key)
        | AvailabilityBatchOperation::RenewDemand(key)
        | AvailabilityBatchOperation::RevalidateDemand(key)
        | AvailabilityBatchOperation::ReleaseDemand(key)
        | AvailabilityBatchOperation::ImportDemand { key, .. } => Some(key),
        AvailabilityBatchOperation::Initialize
        | AvailabilityBatchOperation::SetAlwaysOn(_)
        | AvailabilityBatchOperation::PauseProject
        | AvailabilityBatchOperation::SetTrustedLaunchPath(_)
        | AvailabilityBatchOperation::Retire => None,
    };
    if let Some(demand) = demand {
        demand
            .validate()
            .map_err(|reason| AvailabilityError::InvalidData {
                path: path.to_path_buf(),
                reason: format!("invalid demand operation: {reason}"),
            })?;
    }
    Ok(())
}

fn validate_availability_image(
    image: &AvailabilityStateImage,
    path: &Path,
) -> Result<(), AvailabilityError> {
    if let AvailabilityStateImage::Present(availability) = image {
        availability
            .validate()
            .map_err(|reason| AvailabilityError::InvalidData {
                path: path.to_path_buf(),
                reason,
            })?;
    }
    Ok(())
}

fn apply_batch_operation(
    image: &mut AvailabilityStateImage,
    operation: &AvailabilityBatchOperation,
    effective_at: SystemTime,
) -> Result<(), AvailabilityError> {
    match operation {
        AvailabilityBatchOperation::Initialize => {
            if matches!(image, AvailabilityStateImage::Retired) {
                *image = AvailabilityStateImage::Present(ProjectAvailability::default());
            }
        }
        AvailabilityBatchOperation::EnsureDemand(key) => {
            let availability = materialize_availability(image);
            let _result = availability.ensure_demand(key.clone(), effective_at)?;
        }
        AvailabilityBatchOperation::RenewDemand(key) => {
            if let AvailabilityStateImage::Present(availability) = image {
                let _result = availability.renew_demand(key, effective_at)?;
            }
        }
        AvailabilityBatchOperation::RevalidateDemand(key) => {
            if let AvailabilityStateImage::Present(availability) = image {
                let _result = availability.revalidate_demand(key, effective_at)?;
            }
        }
        AvailabilityBatchOperation::ImportDemand { key, acquired_at } => {
            let availability = materialize_availability(image);
            let _imported = availability.import_demand(key.clone(), *acquired_at, effective_at)?;
        }
        AvailabilityBatchOperation::ReleaseDemand(key) => {
            if let AvailabilityStateImage::Present(availability) = image {
                let _released = availability.release_demand(key, effective_at)?;
            }
        }
        AvailabilityBatchOperation::SetAlwaysOn(enabled) => {
            if *enabled {
                let availability = materialize_availability(image);
                let _changed = availability.set_always_on(true, effective_at)?;
            } else if let AvailabilityStateImage::Present(availability) = image {
                let _changed = availability.set_always_on(false, effective_at)?;
            }
        }
        AvailabilityBatchOperation::PauseProject => {
            let availability = materialize_availability(image);
            let _changed = availability.pause_project();
        }
        AvailabilityBatchOperation::SetTrustedLaunchPath(path) => match path {
            Some(path) => {
                let availability = materialize_availability(image);
                let _changed = availability.set_trusted_launch_path(Some(path.clone()));
            }
            None => {
                if let AvailabilityStateImage::Present(availability) = image {
                    let _changed = availability.set_trusted_launch_path(None);
                }
            }
        },
        AvailabilityBatchOperation::Retire => {
            *image = AvailabilityStateImage::Retired;
        }
    }
    Ok(())
}

fn materialize_availability(image: &mut AvailabilityStateImage) -> &mut ProjectAvailability {
    if matches!(image, AvailabilityStateImage::Retired) {
        *image = AvailabilityStateImage::Present(ProjectAvailability::default());
    }
    let AvailabilityStateImage::Present(availability) = image else {
        unreachable!("retired availability was materialized above");
    };
    availability
}

/// The authoritative runtime action derived from one availability snapshot.
///
/// A cooldown preserves the runtime disposition that already exists: it keeps
/// a running project alive until the deadline, but never authorizes starting
/// or restoring a stopped project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergenceDecision {
    /// Demand or Always On policy requires the project to be running.
    EnsureUp,
    /// Keep the current runtime disposition until the cooldown deadline.
    PreserveRuntimeUntil {
        /// The absolute wall-clock deadline for the next convergence pass.
        deadline: SystemTime,
    },
    /// The project should be stopped, including while explicitly paused.
    EnsureDown,
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

    #[error(
        "prepared availability batch belongs to project instance {batch_instance}; store owns {store_instance}"
    )]
    BatchInstanceMismatch {
        batch_instance: ProjectInstanceId,
        store_instance: ProjectInstanceId,
    },

    #[error("authoritative availability `{path}` changed after its batch was prepared")]
    BatchBaseMismatch { path: PathBuf },

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
    mutation_lock: Arc<MutationLock>,
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
        let data_dir = normalized_data_directory(data_dir)
            .await
            .map_err(|source| AvailabilityError::Io {
                operation: "resolve availability data directory",
                path: data_dir.to_path_buf(),
                source,
            })?;
        let path = availability_path(&data_dir, project_instance_id);
        let availability = load_availability(&path, project_instance_id).await?;
        let mutation_lock = availability_mutation_lock(&path);
        Ok(Self {
            project_instance_id,
            path,
            availability,
            mutation_lock,
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

    /// Prepare a journal-ready batch from one authoritative reload.
    ///
    /// Preparation does not publish. The returned payload captures both the
    /// exact expected image and the deterministic target image and may be
    /// serialized before [`Self::apply_prepared_batch`] is attempted.
    pub async fn prepare_batch(
        &mut self,
        batch: &AvailabilityBatch,
    ) -> Result<PreparedAvailabilityBatch, AvailabilityError> {
        let mutation_lock = Arc::clone(&self.mutation_lock);
        let _guard = mutation_lock.lock_owned().await;
        let expected = load_availability_image(&self.path, self.project_instance_id).await?;
        self.availability = expected.cached_availability();
        prepare_availability_batch(self.project_instance_id, expected, batch, &self.path)
    }

    /// Apply an ordered batch using one authoritative reload and at most one
    /// atomic directory-entry publication.
    ///
    /// This is the direct, non-journaled form. Durable transaction journals
    /// should persist [`PreparedAvailabilityBatch`] and call
    /// [`Self::apply_prepared_batch`] so restart replay compares the captured
    /// base and target instead of preparing a new transaction.
    pub async fn apply_batch(
        &mut self,
        batch: &AvailabilityBatch,
    ) -> Result<AvailabilityBatchApplyResult, AvailabilityError> {
        let mutation_lock = Arc::clone(&self.mutation_lock);
        let _guard = mutation_lock.lock_owned().await;
        let current = load_availability_image(&self.path, self.project_instance_id).await?;
        self.availability = current.cached_availability();
        let prepared = prepare_availability_batch(
            self.project_instance_id,
            current.clone(),
            batch,
            &self.path,
        )?;
        self.apply_prepared_from_current(&prepared, current).await
    }

    /// Compare and atomically publish a journaled availability transaction.
    ///
    /// If the authoritative image equals the prepared target, the earlier
    /// publication already succeeded and replay returns `AlreadyApplied`. If
    /// it equals the captured base, the target is published. Any third state
    /// fails closed without modifying the authoritative file.
    pub async fn apply_prepared_batch(
        &mut self,
        prepared: &PreparedAvailabilityBatch,
    ) -> Result<AvailabilityBatchApplyResult, AvailabilityError> {
        if prepared.project_instance_id != self.project_instance_id {
            return Err(AvailabilityError::BatchInstanceMismatch {
                batch_instance: prepared.project_instance_id,
                store_instance: self.project_instance_id,
            });
        }
        prepared.validate()?;

        let mutation_lock = Arc::clone(&self.mutation_lock);
        let _guard = mutation_lock.lock_owned().await;
        let current = load_availability_image(&self.path, self.project_instance_id).await?;
        self.availability = current.cached_availability();
        self.apply_prepared_from_current(prepared, current).await
    }

    /// Explicitly acquire, renew, or resume one demand.
    pub async fn ensure_demand(
        &mut self,
        key: DemandKey,
    ) -> Result<EnsureDemandResult, AvailabilityError> {
        self.mutate(|candidate, now| Ok((candidate.ensure_demand(key, now)?, true)))
            .await
    }

    /// Passively renew a live demand without advancing or resuming a generation.
    pub async fn renew_demand(
        &mut self,
        key: &DemandKey,
    ) -> Result<RenewDemandResult, AvailabilityError> {
        self.mutate(|candidate, now| {
            let result = candidate.renew_demand(key, now)?;
            let changed = !matches!(result, RenewDemandResult::Missing);
            Ok((result, changed))
        })
        .await
    }

    /// Release one owner while preserving every other demand.
    pub async fn release_demand(&mut self, key: &DemandKey) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, now| {
            let released = candidate.release_demand(key, now)?;
            Ok((released, released))
        })
        .await
    }

    /// Remove every demand whose absolute deadline has been reached.
    pub async fn expire_demands(&mut self) -> Result<usize, AvailabilityError> {
        self.mutate(|candidate, now| {
            let expired = candidate.expire_demands(now)?;
            Ok((expired, expired > 0))
        })
        .await
    }

    /// Expire due demands and derive one authoritative convergence decision.
    ///
    /// Expiry, cooldown arming, and decision derivation share one clock reading
    /// and one serialized mutation. Daemon convergence should use this method
    /// instead of composing the observational [`Self::desired_up`] and
    /// [`Self::shutdown_deferred`] queries.
    pub async fn sweep_and_decide(&mut self) -> Result<ConvergenceDecision, AvailabilityError> {
        self.mutate(|candidate, now| {
            let expired = candidate.expire_demands(now)?;
            let decision = candidate.convergence_decision_at(now);
            Ok((decision, expired > 0))
        })
        .await
    }

    /// Enable or disable durable Always On policy.
    ///
    /// Enabling a paused policy is explicit activity and advances beyond the
    /// pause barrier. Disabling the policy arms the normal shutdown cooldown
    /// when no live demand remains. Returns whether durable state changed.
    pub async fn set_always_on(&mut self, enabled: bool) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, now| {
            let changed = candidate.set_always_on(enabled, now)?;
            Ok((changed, changed))
        })
        .await
    }

    /// Pause the project through its current activity generation.
    ///
    /// Returns whether durable state changed.
    pub async fn pause_project(&mut self) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, _now| {
            let changed = candidate.pause_project();
            Ok((changed, changed))
        })
        .await
    }

    /// Replace the trusted launch `PATH` after an explicit trusted CLI ensure.
    ///
    /// Returns whether durable state changed.
    pub async fn replace_trusted_launch_path(
        &mut self,
        path: String,
    ) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, _now| {
            let changed = candidate.set_trusted_launch_path(Some(path));
            Ok((changed, changed))
        })
        .await
    }

    /// Seed a trusted launch `PATH` only when no earlier trusted caller did so.
    ///
    /// Returns whether this call stored the seed.
    pub async fn seed_trusted_launch_path_if_missing(
        &mut self,
        path: String,
    ) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, _now| {
            let changed = if candidate.trusted_launch_path().is_none() {
                candidate.set_trusted_launch_path(Some(path))
            } else {
                false
            };
            Ok((changed, changed))
        })
        .await
    }

    /// Clear launch context that is no longer trusted or usable.
    ///
    /// Returns whether durable state changed.
    pub async fn clear_trusted_launch_path(&mut self) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, _now| {
            let changed = candidate.set_trusted_launch_path(None);
            Ok((changed, changed))
        })
        .await
    }

    /// Record the last convergence failure shown by status surfaces.
    ///
    /// Returns whether durable state changed.
    pub async fn record_convergence_error(
        &mut self,
        error: String,
    ) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, _now| {
            let changed = candidate.set_last_convergence_error(Some(error));
            Ok((changed, changed))
        })
        .await
    }

    /// Clear the last convergence failure after a successful convergence.
    ///
    /// Returns whether durable state changed.
    pub async fn clear_convergence_error(&mut self) -> Result<bool, AvailabilityError> {
        self.mutate(|candidate, _now| {
            let changed = candidate.set_last_convergence_error(None);
            Ok((changed, changed))
        })
        .await
    }

    /// Load and return the latest authoritative availability snapshot.
    pub async fn snapshot(&mut self) -> Result<ProjectAvailability, AvailabilityError> {
        self.refresh().await?;
        Ok(self.availability.clone())
    }

    /// Derive desired availability from the latest authoritative snapshot.
    pub async fn desired_up(&mut self) -> Result<bool, AvailabilityError> {
        self.refresh().await?;
        Ok(self.availability.desired_up_at(self.clock.now()))
    }

    /// Whether convergence may keep this already-running instance alive.
    pub async fn shutdown_deferred(&mut self) -> Result<bool, AvailabilityError> {
        self.refresh().await?;
        Ok(self.availability.shutdown_deferred_at(self.clock.now()))
    }

    async fn refresh(&mut self) -> Result<(), AvailabilityError> {
        let mutation_lock = Arc::clone(&self.mutation_lock);
        let _guard = mutation_lock.lock_owned().await;
        self.availability = load_availability(&self.path, self.project_instance_id).await?;
        Ok(())
    }

    async fn mutate<Output, Transition>(
        &mut self,
        transition: Transition,
    ) -> Result<Output, AvailabilityError>
    where
        Transition: FnOnce(
            &mut ProjectAvailability,
            SystemTime,
        ) -> Result<(Output, bool), AvailabilityError>,
    {
        let mutation_lock = Arc::clone(&self.mutation_lock);
        let _guard = mutation_lock.lock_owned().await;

        let current = load_availability(&self.path, self.project_instance_id).await?;
        self.availability = current.clone();
        let mut candidate = current;
        let (result, changed) = transition(&mut candidate, self.clock.now())?;
        if changed {
            self.commit(candidate).await?;
        }
        Ok(result)
    }

    async fn apply_prepared_from_current(
        &mut self,
        prepared: &PreparedAvailabilityBatch,
        current: AvailabilityStateImage,
    ) -> Result<AvailabilityBatchApplyResult, AvailabilityError> {
        if current == prepared.target {
            repair_availability_image_durability(&prepared.target, &self.path).await?;
            return Ok(AvailabilityBatchApplyResult {
                disposition: AvailabilityBatchDisposition::AlreadyApplied,
                target: prepared.target.clone(),
            });
        }
        if current != prepared.expected {
            return Err(AvailabilityError::BatchBaseMismatch {
                path: self.path.clone(),
            });
        }

        let result =
            publish_availability_image(&prepared.target, self.project_instance_id, &self.path)
                .await;
        if result.is_ok() || matches!(&result, Err(AvailabilityError::PublishedNotDurable { .. })) {
            self.availability = prepared.target.cached_availability();
        }
        result?;
        Ok(AvailabilityBatchApplyResult {
            disposition: AvailabilityBatchDisposition::Published,
            target: prepared.target.clone(),
        })
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
    Ok(load_availability_image(path, project_instance_id)
        .await?
        .cached_availability())
}

async fn load_availability_image(
    path: &Path,
    project_instance_id: ProjectInstanceId,
) -> Result<AvailabilityStateImage, AvailabilityError> {
    let content = match tokio::fs::read(path).await {
        Ok(content) => content,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            match availability_entry_exists(path).await {
                Ok(false) => return Ok(AvailabilityStateImage::Retired),
                Ok(true) => {
                    return Err(AvailabilityError::Io {
                        operation: "read availability state",
                        path: path.to_path_buf(),
                        source,
                    });
                }
                Err(metadata_source) => {
                    return Err(AvailabilityError::Io {
                        operation: "inspect availability state after missing read",
                        path: path.to_path_buf(),
                        source: metadata_source,
                    });
                }
            }
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
    Ok(AvailabilityStateImage::Present(file.availability))
}

async fn availability_entry_exists(path: &Path) -> io::Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(source),
    }
}

async fn publish_availability_image(
    image: &AvailabilityStateImage,
    project_instance_id: ProjectInstanceId,
    path: &Path,
) -> Result<(), AvailabilityError> {
    match image {
        AvailabilityStateImage::Present(availability) => {
            replace_availability_with_parent_sync(
                availability,
                project_instance_id,
                path,
                |path| async move { sync_parent(&path).await },
            )
            .await
        }
        AvailabilityStateImage::Retired => retire_availability(path).await,
    }
}

async fn retire_availability(path: &Path) -> Result<(), AvailabilityError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => sync_parent(path)
            .await
            .map_err(|error| AvailabilityError::PublishedNotDurable {
                path: path.to_path_buf(),
                reason: error.to_string(),
            }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            repair_availability_image_durability(&AvailabilityStateImage::Retired, path).await
        }
        Err(source) => Err(AvailabilityError::Io {
            operation: "retire availability state",
            path: path.to_path_buf(),
            source,
        }),
    }
}

async fn repair_availability_image_durability(
    image: &AvailabilityStateImage,
    path: &Path,
) -> Result<(), AvailabilityError> {
    let parent = path
        .parent()
        .ok_or_else(|| AvailabilityError::InvalidData {
            path: path.to_path_buf(),
            reason: "availability path has no parent directory".to_owned(),
        })?;
    if matches!(image, AvailabilityStateImage::Retired) {
        match tokio::fs::symlink_metadata(parent).await {
            Ok(_) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(AvailabilityError::Io {
                    operation: "inspect retired availability directory",
                    path: parent.to_path_buf(),
                    source,
                });
            }
        }
    }
    sync_directory(parent)
        .await
        .map_err(|error| AvailabilityError::PublishedNotDurable {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })
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
    // A retry may see directories created by an earlier failed first
    // publication as already present. With no durable marker for the former
    // boundary, repair every directory that can own an entry in the path.
    for owning_parent in availability_hierarchy_owners(project_state_directory) {
        sync_directory(&owning_parent).await?;
    }
    Ok(())
}

fn availability_hierarchy_owners(project_state_directory: &Path) -> Vec<PathBuf> {
    project_state_directory
        .ancestors()
        .skip(1)
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect()
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

fn deadline(now: SystemTime, duration: Duration) -> Result<SystemTime, String> {
    now.checked_add(duration)
        .ok_or_else(|| "demand expiry exceeds SystemTime range".to_owned())
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

    #[test]
    fn first_publication_repairs_every_hierarchy_owner() {
        assert_eq!(
            availability_hierarchy_owners(Path::new(
                "/existing/missing/nested/instances/instance-id"
            )),
            vec![
                PathBuf::from("/existing/missing/nested/instances"),
                PathBuf::from("/existing/missing/nested"),
                PathBuf::from("/existing/missing"),
                PathBuf::from("/existing"),
                PathBuf::from("/"),
            ]
        );
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
        let manual_session = DemandKey::manual_cli_session("manual-session-123")
            .expect("construct owned Manual CLI demand");
        let serialized_manual =
            serde_json::to_string(&manual_session).expect("serialize owned Manual CLI demand");
        assert_eq!(manual_session.kind(), DemandKind::ManualCli);
        assert_eq!(manual_session.safe_label(), "Manual CLI");
        assert_ne!(manual_session, DemandKey::manual_cli());
        assert!(!serialized_manual.contains("manual-session-123"));
        assert_eq!(
            serde_json::from_str::<DemandKey>(&serialized_manual)
                .expect("deserialize owned Manual CLI demand"),
            manual_session
        );
        assert_eq!(AGENT_DEMAND_TTL, AGENT_ACTIVE_TTL + AGENT_REVIEW_GRACE);
    }

    #[test]
    fn legacy_process_lease_has_a_bounded_canonical_deadline() {
        let now = UNIX_EPOCH + Duration::from_secs(START_SECONDS);
        let lease = DemandLease::new(
            DemandKey::legacy_process_attachment("process-42")
                .expect("construct legacy process demand"),
            1,
            now,
        )
        .expect("construct legacy process lease");

        assert_eq!(lease.expires_at(), Some(now + LEGACY_PROCESS_DEMAND_TTL));
        assert!(lease.validate(1).is_ok());
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
    async fn prepared_initialization_materializes_one_authoritative_record() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(31);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let batch = AvailabilityBatch::new(clock.time())
            .with_operation(AvailabilityBatchOperation::Initialize);

        let prepared = store
            .prepare_batch(&batch)
            .await
            .expect("prepare initialization");
        assert_eq!(prepared.expected(), &AvailabilityStateImage::Retired);
        assert_eq!(
            prepared.target(),
            &AvailabilityStateImage::Present(ProjectAvailability::default())
        );
        assert!(!store.path().exists());

        let applied = store
            .apply_prepared_batch(&prepared)
            .await
            .expect("publish initialization");
        assert_eq!(
            applied.disposition(),
            AvailabilityBatchDisposition::Published
        );
        assert!(store.path().is_file());

        let replayed = store
            .apply_prepared_batch(&prepared)
            .await
            .expect("replay initialization");
        assert_eq!(
            replayed.disposition(),
            AvailabilityBatchDisposition::AlreadyApplied
        );
    }

    #[tokio::test]
    async fn serialized_prepared_replay_preserves_fixed_deadlines_and_generation() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(32);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let effective_at = clock.time();
        let batch = AvailabilityBatch::new(effective_at).with_operation(
            AvailabilityBatchOperation::EnsureDemand(DemandKey::manual_cli()),
        );
        let prepared = store
            .prepare_batch(&batch)
            .await
            .expect("prepare fixed-time ensure");
        let journal_bytes = serde_json::to_vec(&prepared).expect("serialize prepared batch");

        clock.advance(Duration::from_hours(1));
        let replay_payload: PreparedAvailabilityBatch =
            serde_json::from_slice(&journal_bytes).expect("deserialize prepared batch");
        store
            .apply_prepared_batch(&replay_payload)
            .await
            .expect("apply prepared batch after clock advance");
        let first_bytes = std::fs::read(store.path()).expect("read first publication");

        clock.advance(Duration::from_hours(1));
        let mut restarted = fake_store(&fixture, project_instance_id, clock).await;
        let replayed = restarted
            .apply_prepared_batch(&replay_payload)
            .await
            .expect("replay prepared batch after restart");
        assert_eq!(
            replayed.disposition(),
            AvailabilityBatchDisposition::AlreadyApplied
        );
        assert_eq!(
            std::fs::read(restarted.path()).expect("read replayed publication"),
            first_bytes
        );

        let snapshot = restarted.snapshot().await.expect("load replayed snapshot");
        assert_eq!(snapshot.activity_generation(), 1);
        let demand = snapshot.demands().first().expect("manual demand exists");
        assert_eq!(demand.acquired_at(), effective_at);
        assert_eq!(demand.renewed_at(), effective_at);
        assert_eq!(demand.expires_at(), Some(effective_at + MANUAL_DEMAND_TTL));
    }

    #[test]
    fn prepared_validation_rejects_invalid_keys_in_noop_demand_operations() {
        let effective_at = UNIX_EPOCH + Duration::from_secs(START_SECONDS);
        let key =
            DemandKey::legacy_process_attachment("process-42").expect("construct process demand");
        let operations = [
            AvailabilityBatchOperation::RenewDemand(key.clone()),
            AvailabilityBatchOperation::RevalidateDemand(key.clone()),
            AvailabilityBatchOperation::ReleaseDemand(key.clone()),
            AvailabilityBatchOperation::ImportDemand {
                key,
                acquired_at: effective_at - LEGACY_PROCESS_DEMAND_TTL,
            },
        ];

        for operation in operations {
            let batch = AvailabilityBatch::new(effective_at).with_operation(operation);
            let prepared = prepare_availability_batch(
                instance_id(39),
                AvailabilityStateImage::Retired,
                &batch,
                Path::new("<tampered-journal>"),
            )
            .expect("prepare valid no-op demand operation");
            let mut encoded = serde_json::to_value(prepared).expect("serialize prepared batch");
            let value = &mut encoded["batch"]["operations"][0]["value"];
            let key = if value.get("key").is_some() {
                &mut value["key"]
            } else {
                value
            };
            key["owner"] = serde_json::json!("not-a-canonical-digest");
            let tampered: PreparedAvailabilityBatch =
                serde_json::from_value(encoded).expect("decode structurally valid tampered batch");

            assert!(matches!(
                tampered.validate(),
                Err(AvailabilityError::InvalidData { .. })
            ));
        }
    }

    #[test]
    fn persisted_availability_enums_reject_unknown_fields() {
        let effective_at = UNIX_EPOCH + Duration::from_secs(START_SECONDS);
        let batch = AvailabilityBatch::new(effective_at)
            .with_operation(AvailabilityBatchOperation::Initialize);
        let prepared = prepare_availability_batch(
            instance_id(40),
            AvailabilityStateImage::Retired,
            &batch,
            Path::new("<unknown-fields>"),
        )
        .expect("prepare initialization");

        let mut unknown_operation =
            serde_json::to_value(&prepared).expect("serialize prepared operation");
        unknown_operation["batch"]["operations"][0]["unexpected"] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<PreparedAvailabilityBatch>(unknown_operation).is_err(),
            "operation payload must reject unknown fields"
        );

        let mut unknown_image = serde_json::to_value(prepared).expect("serialize prepared image");
        unknown_image["expected"]["unexpected"] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<PreparedAvailabilityBatch>(unknown_image).is_err(),
            "state image must reject unknown fields"
        );
    }

    #[tokio::test]
    async fn batch_operation_order_controls_pause_barrier_semantics() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(33);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let stop_batch = AvailabilityBatch::new(clock.time())
            .with_operation(AvailabilityBatchOperation::EnsureDemand(
                DemandKey::manual_cli(),
            ))
            .with_operation(AvailabilityBatchOperation::PauseProject);
        let stopped = store
            .apply_batch(&stop_batch)
            .await
            .expect("ensure then pause");
        let AvailabilityStateImage::Present(stopped) = stopped.target() else {
            panic!("stopped target should remain present");
        };
        assert_eq!(stopped.activity_generation(), 1);
        assert!(stopped.is_paused());
        assert!(!stopped.desired_up_at(clock.time()));

        clock.advance(Duration::from_secs(1));
        let resume_batch = AvailabilityBatch::new(clock.time())
            .with_operation(AvailabilityBatchOperation::PauseProject)
            .with_operation(AvailabilityBatchOperation::EnsureDemand(
                DemandKey::manual_cli(),
            ));
        let prepared = store
            .prepare_batch(&resume_batch)
            .await
            .expect("prepare pause then ensure");
        store
            .apply_prepared_batch(&prepared)
            .await
            .expect("publish pause then ensure");
        store
            .apply_prepared_batch(&prepared)
            .await
            .expect("replay pause then ensure");

        let resumed = store.snapshot().await.expect("load resumed snapshot");
        assert_eq!(resumed.activity_generation(), 2);
        assert!(!resumed.is_paused());
        assert!(resumed.desired_up_at(clock.time()));
    }

    #[tokio::test]
    async fn process_revalidation_extends_an_expired_lease_at_its_original_generation() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(35);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let process =
            DemandKey::legacy_process_attachment("pid-42").expect("construct process demand");
        store
            .apply_batch(
                &AvailabilityBatch::new(clock.time())
                    .with_operation(AvailabilityBatchOperation::EnsureDemand(process.clone()))
                    .with_operation(AvailabilityBatchOperation::PauseProject),
            )
            .await
            .expect("seed paused process demand");

        clock.advance(LEGACY_PROCESS_DEMAND_TTL + Duration::from_secs(1));
        store
            .apply_batch(
                &AvailabilityBatch::new(clock.time())
                    .with_operation(AvailabilityBatchOperation::RevalidateDemand(process)),
            )
            .await
            .expect("revalidate process-proven owner");

        let snapshot = store.snapshot().await.expect("load revalidated state");
        assert_eq!(snapshot.activity_generation(), 1);
        assert!(snapshot.is_paused());
        assert_eq!(snapshot.demands().len(), 1);
        assert_eq!(
            snapshot.demands()[0].expires_at(),
            Some(clock.time() + LEGACY_PROCESS_DEMAND_TTL)
        );
        assert!(!snapshot.desired_up_at(clock.time()));
    }

    #[tokio::test]
    async fn process_revalidation_does_not_recreate_a_swept_lease() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(36);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let process =
            DemandKey::legacy_process_attachment("pid-42").expect("construct process demand");
        store
            .ensure_demand(process.clone())
            .await
            .expect("seed process demand");
        clock.advance(LEGACY_PROCESS_DEMAND_TTL + Duration::from_secs(1));
        store.expire_demands().await.expect("sweep expired demand");

        store
            .apply_batch(
                &AvailabilityBatch::new(clock.time())
                    .with_operation(AvailabilityBatchOperation::RevalidateDemand(process)),
            )
            .await
            .expect("apply missing process revalidation");
        let snapshot = store.snapshot().await.expect("load swept state");
        assert_eq!(snapshot.activity_generation(), 1);
        assert!(snapshot.demands().is_empty());
    }

    #[tokio::test]
    async fn imported_hold_keeps_its_original_deadline_and_expired_holds_stay_absent() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let acquired_at = clock.time() - Duration::from_hours(3);
        let mut live = fake_store(&fixture, instance_id(37), clock.clone()).await;
        live.apply_batch(&AvailabilityBatch::new(clock.time()).with_operation(
            AvailabilityBatchOperation::ImportDemand {
                key: DemandKey::manual_cli(),
                acquired_at,
            },
        ))
        .await
        .expect("import live hold");
        let snapshot = live.snapshot().await.expect("load imported hold");
        assert_eq!(snapshot.activity_generation(), 1);
        assert_eq!(
            snapshot.demands()[0].expires_at(),
            Some(acquired_at + MANUAL_DEMAND_TTL)
        );

        let mut expired = fake_store(&fixture, instance_id(38), clock.clone()).await;
        expired
            .apply_batch(&AvailabilityBatch::new(clock.time()).with_operation(
                AvailabilityBatchOperation::ImportDemand {
                    key: DemandKey::manual_cli(),
                    acquired_at: clock.time() - MANUAL_DEMAND_TTL,
                },
            ))
            .await
            .expect("ignore expired hold");
        let snapshot = expired.snapshot().await.expect("load expired import");
        assert_eq!(snapshot.activity_generation(), 0);
        assert!(snapshot.demands().is_empty());
    }

    #[tokio::test]
    async fn prepared_retirement_removes_state_and_replays_idempotently() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(34);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let seed = AvailabilityBatch::new(clock.time())
            .with_operation(AvailabilityBatchOperation::EnsureDemand(
                DemandKey::manual_cli(),
            ))
            .with_operation(AvailabilityBatchOperation::SetAlwaysOn(true))
            .with_operation(AvailabilityBatchOperation::SetTrustedLaunchPath(Some(
                "/usr/bin".to_owned(),
            )));
        store.apply_batch(&seed).await.expect("seed availability");
        assert!(store.path().is_file());

        let retire =
            AvailabilityBatch::new(clock.time()).with_operation(AvailabilityBatchOperation::Retire);
        let prepared = store
            .prepare_batch(&retire)
            .await
            .expect("prepare retirement");
        let applied = store
            .apply_prepared_batch(&prepared)
            .await
            .expect("retire availability");
        assert_eq!(
            applied.disposition(),
            AvailabilityBatchDisposition::Published
        );
        assert!(!store.path().exists());

        let mut restarted = fake_store(&fixture, project_instance_id, clock).await;
        let replayed = restarted
            .apply_prepared_batch(&prepared)
            .await
            .expect("replay retirement");
        assert_eq!(
            replayed.disposition(),
            AvailabilityBatchDisposition::AlreadyApplied
        );
        assert_eq!(
            restarted.snapshot().await.expect("load retired state"),
            ProjectAvailability::default()
        );
    }

    #[tokio::test]
    async fn prepared_publication_fails_closed_after_base_changes() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(35);
        let mut first = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut second = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let prepared = first
            .prepare_batch(&AvailabilityBatch::new(clock.time()).with_operation(
                AvailabilityBatchOperation::EnsureDemand(DemandKey::manual_cli()),
            ))
            .await
            .expect("prepare manual demand");

        second
            .apply_batch(
                &AvailabilityBatch::new(clock.time())
                    .with_operation(AvailabilityBatchOperation::SetAlwaysOn(true)),
            )
            .await
            .expect("publish intervening policy");
        let intervening = std::fs::read(second.path()).expect("read intervening publication");

        let error = first
            .apply_prepared_batch(&prepared)
            .await
            .expect_err("reject stale prepared base");
        assert!(matches!(error, AvailabilityError::BatchBaseMismatch { .. }));
        assert_eq!(
            std::fs::read(first.path()).expect("reread authoritative state"),
            intervening
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_default_base_preserves_a_dangling_availability_symlink() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(36);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let prepared = store
            .prepare_batch(
                &AvailabilityBatch::new(clock.time())
                    .with_operation(AvailabilityBatchOperation::Initialize),
            )
            .await
            .expect("prepare availability from the retired default base");
        assert_eq!(prepared.expected(), &AvailabilityStateImage::Retired);

        let availability_path = store.path().to_path_buf();
        let parent = availability_path
            .parent()
            .expect("availability path has a parent");
        std::fs::create_dir_all(parent).expect("create availability parent");
        let missing_target = parent.join("missing-availability-target.json");
        std::os::unix::fs::symlink(&missing_target, &availability_path)
            .expect("create dangling availability symlink");

        let error = store
            .apply_prepared_batch(&prepared)
            .await
            .expect_err("dangling availability entry must not equal the retired base");
        assert!(matches!(
            error,
            AvailabilityError::Io {
                operation: "read availability state",
                source,
                ..
            } if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(
            std::fs::symlink_metadata(&availability_path)
                .expect("inspect preserved availability entry")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(&availability_path)
                .expect("read preserved dangling availability symlink"),
            missing_target
        );
        assert!(temporary_files(&availability_path).is_empty());
    }

    #[tokio::test]
    async fn batch_rejects_malformed_authoritative_state_without_rewriting_it() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(36);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let content = b"{ malformed availability";
        std::fs::create_dir_all(store.path().parent().expect("availability parent"))
            .expect("create availability parent");
        std::fs::write(store.path(), content).expect("write malformed availability");

        let error = store
            .apply_batch(
                &AvailabilityBatch::new(clock.time())
                    .with_operation(AvailabilityBatchOperation::Retire),
            )
            .await
            .expect_err("reject malformed authoritative state");
        assert!(matches!(error, AvailabilityError::InvalidData { .. }));
        assert_eq!(
            std::fs::read(store.path()).expect("read preserved malformed state"),
            content
        );
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
        assert_eq!(store.availability.activity_generation(), 1);
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
        assert_eq!(store.availability.activity_generation(), 2);
        assert_eq!(store.availability.demands().len(), 2);
        assert!(store.desired_up().await.expect("derive desired state"));
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
        assert!(store.pause_project().await.expect("pause project"));
        assert!(store.availability.is_paused());

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
        assert_eq!(store.availability.activity_generation(), 1);
        assert!(store.availability.is_paused());
        assert!(!store.desired_up().await.expect("derive paused state"));

        let resumed = store
            .ensure_demand(manual)
            .await
            .expect("explicitly resume demand");
        assert_eq!(resumed.effect, EnsureDemandEffect::Resumed);
        assert_eq!(resumed.lease.generation(), 2);
        assert!(!store.availability.is_paused());
        assert!(store.desired_up().await.expect("derive resumed state"));
    }

    #[tokio::test]
    async fn paused_owner_stays_suppressed_until_its_own_semantic_activity() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(15), clock.clone()).await;
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");
        let manual = DemandKey::manual_cli();

        store
            .ensure_demand(editor.clone())
            .await
            .expect("acquire editor demand");
        assert!(store.pause_project().await.expect("pause project"));

        let manual_resume = store
            .ensure_demand(manual.clone())
            .await
            .expect("resume through manual activity");
        assert_eq!(manual_resume.effect, EnsureDemandEffect::Resumed);
        assert_eq!(manual_resume.lease.generation(), 2);
        assert!(store.release_demand(&manual).await.expect("release manual"));

        store
            .renew_demand(&editor)
            .await
            .expect("passively renew suppressed editor");
        assert!(
            store
                .availability
                .live_demands_at(clock.time())
                .next()
                .is_some()
        );
        assert!(
            store
                .availability
                .effective_demands_at(clock.time())
                .next()
                .is_none()
        );
        assert!(!store.desired_up().await.expect("derive suppressed state"));

        let editor_resume = store
            .ensure_demand(editor)
            .await
            .expect("resume through editor activity");
        assert_eq!(editor_resume.effect, EnsureDemandEffect::Resumed);
        assert_eq!(editor_resume.lease.generation(), 3);
        assert!(store.desired_up().await.expect("derive resumed state"));
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
        assert!(store.availability.demands().is_empty());
        assert_eq!(store.availability.activity_generation(), 1);
        assert_eq!(
            store.availability.shutdown_cooldown_until(),
            Some(clock.time() + SHUTDOWN_COOLDOWN)
        );
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
        assert_eq!(store.availability.demands().len(), 1);
        assert_eq!(store.availability.shutdown_cooldown_until(), None);
        assert!(store.release_demand(&manual).await.expect("release manual"));
        assert!(
            !store
                .release_demand(&manual)
                .await
                .expect("release missing manual")
        );
        assert!(store.availability.demands().is_empty());
        assert!(!store.desired_up().await.expect("derive idle state"));
        assert_eq!(
            store.availability.shutdown_cooldown_until(),
            Some(clock.time() + SHUTDOWN_COOLDOWN)
        );
        assert!(
            store
                .shutdown_deferred()
                .await
                .expect("derive shutdown deferral")
        );
    }

    #[tokio::test]
    async fn stale_legacy_process_demand_expires() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(16), clock.clone()).await;
        let legacy = DemandKey::legacy_process_attachment("process-42")
            .expect("construct legacy process demand");

        store
            .ensure_demand(legacy)
            .await
            .expect("acquire legacy process demand");
        clock.advance(LEGACY_PROCESS_DEMAND_TTL);

        assert_eq!(
            store.expire_demands().await.expect("expire legacy demand"),
            1
        );
        assert!(store.availability.demands().is_empty());
        assert!(!store.desired_up().await.expect("derive idle state"));
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

    #[test]
    fn cooldown_defers_shutdown_without_authorizing_start_or_restore() {
        let now = UNIX_EPOCH + Duration::from_secs(START_SECONDS);
        let mut availability = ProjectAvailability {
            shutdown_cooldown_until: Some(now + SHUTDOWN_COOLDOWN),
            ..ProjectAvailability::default()
        };

        assert!(!availability.desired_up_at(now));
        assert!(availability.shutdown_deferred_at(now));
        assert!(!availability.shutdown_deferred_at(now + SHUTDOWN_COOLDOWN));

        availability.pause_through_generation = Some(0);
        assert!(!availability.shutdown_deferred_at(now));
    }

    #[tokio::test]
    async fn delayed_expiry_uses_the_lease_deadline_for_cooldown() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(20), clock.clone()).await;
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");
        let demand_expires_at = clock.time() + VSCODE_DEMAND_TTL;

        store
            .ensure_demand(editor)
            .await
            .expect("acquire editor demand");
        clock.advance(VSCODE_DEMAND_TTL + SHUTDOWN_COOLDOWN + Duration::from_secs(1));
        assert_eq!(
            store.expire_demands().await.expect("expire editor demand"),
            1
        );

        assert_eq!(
            store.availability.shutdown_cooldown_until(),
            Some(demand_expires_at + SHUTDOWN_COOLDOWN)
        );
        assert!(
            !store
                .shutdown_deferred()
                .await
                .expect("derive elapsed shutdown deferral")
        );
    }

    #[tokio::test]
    async fn convergence_sweep_arms_cooldown_at_the_exact_lease_boundary() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(30);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");
        let demand_expires_at = clock.time() + VSCODE_DEMAND_TTL;

        store
            .ensure_demand(editor)
            .await
            .expect("acquire editor demand");
        clock.advance(VSCODE_DEMAND_TTL);

        assert_eq!(
            store.sweep_and_decide().await.expect("sweep exact expiry"),
            ConvergenceDecision::PreserveRuntimeUntil {
                deadline: demand_expires_at + SHUTDOWN_COOLDOWN,
            }
        );
        let snapshot = store.snapshot().await.expect("reload swept availability");
        assert!(snapshot.demands().is_empty());
        assert_eq!(
            snapshot.shutdown_cooldown_until(),
            Some(demand_expires_at + SHUTDOWN_COOLDOWN)
        );

        let mut reopened = fake_store(&fixture, project_instance_id, clock).await;
        assert_eq!(
            reopened
                .sweep_and_decide()
                .await
                .expect("repeat authoritative decision"),
            ConvergenceDecision::PreserveRuntimeUntil {
                deadline: demand_expires_at + SHUTDOWN_COOLDOWN,
            }
        );
    }

    #[tokio::test]
    async fn late_convergence_sweep_does_not_create_a_fresh_cooldown() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(31), clock.clone()).await;
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");
        let demand_expires_at = clock.time() + VSCODE_DEMAND_TTL;

        store
            .ensure_demand(editor)
            .await
            .expect("acquire editor demand");
        clock.advance(VSCODE_DEMAND_TTL + SHUTDOWN_COOLDOWN + Duration::from_secs(1));

        assert_eq!(
            store
                .sweep_and_decide()
                .await
                .expect("sweep after cooldown"),
            ConvergenceDecision::EnsureDown
        );
        assert_eq!(
            store
                .snapshot()
                .await
                .expect("reload late sweep")
                .shutdown_cooldown_until(),
            Some(demand_expires_at + SHUTDOWN_COOLDOWN)
        );
    }

    #[tokio::test]
    async fn always_on_and_pause_produce_authoritative_convergence_decisions() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(32), clock).await;

        store.set_always_on(true).await.expect("enable Always On");
        assert_eq!(
            store
                .sweep_and_decide()
                .await
                .expect("derive Always On decision"),
            ConvergenceDecision::EnsureUp
        );

        store.pause_project().await.expect("pause project");
        assert_eq!(
            store
                .sweep_and_decide()
                .await
                .expect("derive paused decision"),
            ConvergenceDecision::EnsureDown
        );

        store
            .set_always_on(true)
            .await
            .expect("renew Always On to resume");
        assert_eq!(
            store
                .sweep_and_decide()
                .await
                .expect("derive resumed decision"),
            ConvergenceDecision::EnsureUp
        );
    }

    #[tokio::test]
    async fn convergence_sweep_reloads_newer_authoritative_demand() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(33);
        let mut observer = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut writer = fake_store(&fixture, project_instance_id, clock).await;

        writer
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("acquire demand through newer writer");

        assert_eq!(
            observer
                .sweep_and_decide()
                .await
                .expect("derive from reloaded authority"),
            ConvergenceDecision::EnsureUp
        );
    }

    #[tokio::test]
    async fn releasing_an_expired_lease_uses_its_expiry_for_cooldown() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(29), clock.clone()).await;
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");
        let demand_expires_at = clock.time() + VSCODE_DEMAND_TTL;

        store
            .ensure_demand(editor.clone())
            .await
            .expect("acquire editor demand");
        clock.advance(VSCODE_DEMAND_TTL + Duration::from_secs(1));
        assert!(
            store
                .release_demand(&editor)
                .await
                .expect("release expired editor demand")
        );

        assert_eq!(
            store.availability.shutdown_cooldown_until(),
            Some(demand_expires_at + SHUTDOWN_COOLDOWN)
        );
        assert!(
            store
                .shutdown_deferred()
                .await
                .expect("derive remaining shutdown deferral")
        );
    }

    #[tokio::test]
    async fn later_cleanup_preserves_an_already_armed_cooldown() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(21), clock.clone()).await;
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");
        let manual = DemandKey::manual_cli();

        store
            .ensure_demand(editor)
            .await
            .expect("acquire editor demand");
        clock.advance(VSCODE_DEMAND_TTL);
        store
            .ensure_demand(manual.clone())
            .await
            .expect("acquire manual demand");
        assert!(store.release_demand(&manual).await.expect("release manual"));
        let armed_deadline = store
            .availability
            .shutdown_cooldown_until()
            .expect("cooldown is armed");

        assert_eq!(
            store.expire_demands().await.expect("remove stale editor"),
            1
        );
        assert_eq!(
            store.availability.shutdown_cooldown_until(),
            Some(armed_deadline)
        );
    }

    #[tokio::test]
    async fn policy_transitions_are_generation_scoped_and_durable() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(22);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;

        assert!(store.set_always_on(true).await.expect("enable Always On"));
        assert_eq!(store.availability.activity_generation(), 1);
        assert!(store.availability.always_on());
        assert!(store.desired_up().await.expect("derive pinned state"));

        assert!(store.pause_project().await.expect("pause pinned project"));
        assert!(store.availability.is_paused());
        assert!(store.availability.always_on());
        assert!(!store.desired_up().await.expect("derive paused state"));

        assert!(
            store
                .set_always_on(true)
                .await
                .expect("renew pin to resume")
        );
        assert_eq!(store.availability.activity_generation(), 2);
        assert!(!store.availability.is_paused());
        assert!(store.desired_up().await.expect("derive resumed pin state"));

        assert!(store.set_always_on(false).await.expect("disable Always On"));
        assert!(!store.availability.always_on());
        assert_eq!(
            store.availability.shutdown_cooldown_until(),
            Some(clock.time() + SHUTDOWN_COOLDOWN)
        );

        let mut reopened = fake_store(&fixture, project_instance_id, clock).await;
        assert_eq!(
            reopened.snapshot().await.expect("load policy snapshot"),
            store.availability
        );
    }

    #[tokio::test]
    async fn launch_context_and_convergence_error_transitions_are_durable() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(23);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;

        assert!(
            store
                .replace_trusted_launch_path("/opt/homebrew/bin:/usr/bin".to_owned())
                .await
                .expect("record trusted PATH")
        );
        assert!(
            store
                .record_convergence_error("web readiness timed out".to_owned())
                .await
                .expect("record convergence error")
        );

        let mut reopened = fake_store(&fixture, project_instance_id, clock).await;
        let snapshot = reopened.snapshot().await.expect("load durable diagnostics");
        assert_eq!(
            snapshot.trusted_launch_path(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(
            snapshot.last_convergence_error(),
            Some("web readiness timed out")
        );

        assert!(
            reopened
                .clear_trusted_launch_path()
                .await
                .expect("clear trusted PATH")
        );
        assert!(
            reopened
                .clear_convergence_error()
                .await
                .expect("clear convergence error")
        );
        let cleared = reopened.snapshot().await.expect("load cleared diagnostics");
        assert_eq!(cleared.trusted_launch_path(), None);
        assert_eq!(cleared.last_convergence_error(), None);
    }

    #[tokio::test]
    async fn orthogonal_mutations_from_stale_handles_are_preserved() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(24);
        let mut policy = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut launch = fake_store(&fixture, project_instance_id, clock.clone()).await;

        let (policy_result, launch_result) = tokio::join!(
            policy.set_always_on(true),
            launch.replace_trusted_launch_path("/usr/local/bin:/usr/bin".to_owned())
        );
        assert!(policy_result.expect("persist Always On"));
        assert!(launch_result.expect("persist trusted PATH"));

        let mut reopened = fake_store(&fixture, project_instance_id, clock).await;
        let snapshot = reopened
            .snapshot()
            .await
            .expect("load combined policy state");
        assert!(snapshot.always_on());
        assert_eq!(
            snapshot.trusted_launch_path(),
            Some("/usr/local/bin:/usr/bin")
        );
    }

    #[tokio::test]
    async fn concurrent_launch_path_seeding_selects_one_trusted_value() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(25);
        let mut editor = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut agent = fake_store(&fixture, project_instance_id, clock.clone()).await;

        let (editor_result, agent_result) = tokio::join!(
            editor.seed_trusted_launch_path_if_missing("/editor/bin".to_owned()),
            agent.seed_trusted_launch_path_if_missing("/agent/bin".to_owned())
        );
        assert_ne!(
            editor_result.expect("seed editor PATH"),
            agent_result.expect("seed agent PATH")
        );

        let mut reopened = fake_store(&fixture, project_instance_id, clock).await;
        assert!(matches!(
            reopened
                .snapshot()
                .await
                .expect("load seeded PATH")
                .trusted_launch_path(),
            Some("/editor/bin" | "/agent/bin")
        ));
    }

    #[tokio::test]
    async fn stale_readers_refresh_before_availability_decisions() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(26);
        let mut observer = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut writer = fake_store(&fixture, project_instance_id, clock).await;
        let manual = DemandKey::manual_cli();

        writer
            .ensure_demand(manual.clone())
            .await
            .expect("acquire demand through writer");
        assert!(
            observer
                .desired_up()
                .await
                .expect("observe acquired demand")
        );
        assert_eq!(
            observer
                .snapshot()
                .await
                .expect("observe authoritative snapshot")
                .demands()
                .len(),
            1
        );

        assert!(
            writer
                .release_demand(&manual)
                .await
                .expect("release demand")
        );
        assert!(
            !observer
                .desired_up()
                .await
                .expect("observe released demand")
        );
        assert!(
            observer
                .shutdown_deferred()
                .await
                .expect("observe armed cooldown")
        );
    }

    #[tokio::test]
    async fn invalid_policy_inputs_preserve_the_authoritative_snapshot() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(27);
        let mut store = fake_store(&fixture, project_instance_id, clock).await;

        store
            .replace_trusted_launch_path("/usr/bin".to_owned())
            .await
            .expect("record initial trusted PATH");
        let before = std::fs::read(store.path()).expect("read authoritative snapshot");

        let path_error = store
            .replace_trusted_launch_path("/bad\0path".to_owned())
            .await
            .expect_err("reject PATH containing NUL");
        assert!(matches!(path_error, AvailabilityError::InvalidData { .. }));
        assert_eq!(
            std::fs::read(store.path()).expect("reread snapshot after invalid PATH"),
            before
        );

        let convergence_error = store
            .record_convergence_error("   ".to_owned())
            .await
            .expect_err("reject empty convergence error");
        assert!(matches!(
            convergence_error,
            AvailabilityError::InvalidData { .. }
        ));
        assert_eq!(
            std::fs::read(store.path()).expect("reread snapshot after invalid error"),
            before
        );
    }

    #[tokio::test]
    async fn authoritative_read_failure_never_reuses_cached_state() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(28);
        let mut store = fake_store(&fixture, project_instance_id, clock).await;

        store
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("persist live demand");
        std::fs::write(store.path(), b"{").expect("corrupt authoritative snapshot");

        let error = store
            .desired_up()
            .await
            .expect_err("reject corrupt authoritative read");
        assert!(matches!(error, AvailabilityError::InvalidData { .. }));
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
        assert_eq!(reopened.availability, first.availability);
        let unchanged = reopened.availability.clone();
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
    async fn relative_data_directory_publishes_through_an_absolute_store_path() {
        let relative_fixture = tempfile::Builder::new()
            .prefix(".locald-availability-relative-")
            .tempdir_in(".")
            .expect("create relative availability fixture");
        let relative_data_dir = PathBuf::from(
            relative_fixture
                .path()
                .file_name()
                .expect("relative fixture name"),
        );
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(17);
        let mut store = AvailabilityStore::load_with_clock(
            &relative_data_dir,
            project_instance_id,
            clock.clone(),
        )
        .await
        .expect("load relative availability store");

        assert!(store.path().is_absolute());
        store
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect("publish relative availability state");

        let reopened =
            AvailabilityStore::load_with_clock(&relative_data_dir, project_instance_id, clock)
                .await
                .expect("reopen relative availability store");
        assert_eq!(reopened.availability, store.availability);
    }

    #[tokio::test]
    async fn concurrent_stale_handles_preserve_independent_demands() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(18);
        let mut first = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut second = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let manual = DemandKey::manual_cli();
        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");

        let (manual_result, editor_result) = tokio::join!(
            first.ensure_demand(manual.clone()),
            second.ensure_demand(editor.clone())
        );
        let manual_result = manual_result.expect("acquire manual demand");
        let editor_result = editor_result.expect("acquire editor demand");
        let mut generations = [
            manual_result.lease.generation(),
            editor_result.lease.generation(),
        ];
        generations.sort_unstable();
        assert_eq!(generations, [1, 2]);

        let reopened = fake_store(&fixture, project_instance_id, clock.clone()).await;
        assert_eq!(reopened.availability.activity_generation(), 2);
        assert_eq!(reopened.availability.demands().len(), 2);

        assert!(
            first
                .release_demand(&manual)
                .await
                .expect("release from older handle")
        );
        let reopened = fake_store(&fixture, project_instance_id, clock).await;
        assert_eq!(reopened.availability.demands().len(), 1);
        assert_eq!(reopened.availability.demands()[0].key(), &editor);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_data_directory_shares_the_authoritative_mutation_lock() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(&fixture.data_dir).expect("create canonical data directory");
        let alias = fixture
            .data_dir
            .parent()
            .expect("data directory parent")
            .join("data-alias");
        std::os::unix::fs::symlink(&fixture.data_dir, &alias).expect("create data directory alias");
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(19);
        let mut canonical = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut aliased =
            AvailabilityStore::load_with_clock(&alias, project_instance_id, clock.clone())
                .await
                .expect("load aliased availability store");
        assert_eq!(canonical.path(), aliased.path());

        let editor = DemandKey::vs_code_window("window-1").expect("construct editor demand");
        let (manual_result, editor_result) = tokio::join!(
            canonical.ensure_demand(DemandKey::manual_cli()),
            aliased.ensure_demand(editor)
        );
        manual_result.expect("acquire canonical demand");
        editor_result.expect("acquire aliased demand");

        let reopened = fake_store(&fixture, project_instance_id, clock).await;
        assert_eq!(reopened.availability.activity_generation(), 2);
        assert_eq!(reopened.availability.demands().len(), 2);
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
        let before = store.availability.clone();
        let occupied_path = store.path().to_path_buf();

        let error = store
            .mutate(move |candidate, now| {
                let result = candidate.ensure_demand(DemandKey::manual_cli(), now)?;
                std::fs::create_dir_all(&occupied_path)
                    .expect("occupy authoritative path after reload");
                Ok((result, true))
            })
            .await
            .expect_err("fail before publishing availability");
        assert!(matches!(error, AvailabilityError::Io { .. }));
        assert_eq!(store.availability, before);
        assert!(temporary_files(store.path()).is_empty());
    }

    #[tokio::test]
    async fn parent_sync_failure_aligns_memory_with_published_state() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let project_instance_id = instance_id(12);
        let mut store = fake_store(&fixture, project_instance_id, clock.clone()).await;
        let mut candidate = store.availability.clone();
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
        assert_eq!(store.availability, candidate);
        assert!(temporary_files(store.path()).is_empty());

        let reopened = fake_store(&fixture, project_instance_id, clock).await;
        assert_eq!(reopened.availability, candidate);
    }

    #[tokio::test]
    async fn generation_overflow_preserves_state_and_disk() {
        let fixture = Fixture::new();
        let clock = FakeClock::new(START_SECONDS);
        let mut store = fake_store(&fixture, instance_id(13), clock).await;
        store.availability.activity_generation = u64::MAX;
        let exhausted = store.availability.clone();
        store
            .commit(exhausted)
            .await
            .expect("persist exhausted generation");
        let before = store.availability.clone();

        let error = store
            .ensure_demand(DemandKey::manual_cli())
            .await
            .expect_err("reject exhausted generation");
        assert!(matches!(
            error,
            AvailabilityError::GenerationExhausted { current: u64::MAX }
        ));
        assert_eq!(store.availability, before);
        let persisted = AvailabilityStore::load(&fixture.data_dir, store.project_instance_id())
            .await
            .expect("reopen exhausted generation");
        assert_eq!(persisted.availability, before);
    }
}
