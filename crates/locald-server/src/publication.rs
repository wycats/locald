//! Ephemeral authority for externally fulfilled published services.
//!
//! Durable published-service declarations live in the project catalog. This
//! module deliberately owns only daemon-lifetime authority: publisher
//! principals, attempts, retained listener capabilities, leases, and their
//! suspend-inclusive deadlines. None of these types is serializable.

#![allow(
    dead_code,
    reason = "b.3.4 builds the authority engine before b.3.5 exposes authenticated transport"
)]

use locald_core::ipc::PublicationState;
use locald_core::{ProjectInstanceId, PublishedServiceDeclaration, SemanticOrigin, ServiceKey};
use rand::{RngCore as _, rngs::OsRng};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

const LEASE_TTL: Duration = Duration::from_secs(30);
const RENEW_AFTER: Duration = Duration::from_secs(10);
const ACQUISITION_ATTEMPT_TTL: Duration = Duration::from_secs(15);
const REBIND_ATTEMPT_TTL: Duration = Duration::from_secs(15);
const WAIT_READY_TTL: Duration = Duration::from_secs(30);
const PREPARATION_TTL: Duration = Duration::from_mins(1);
const FRAME_DELIVERY_TTL: Duration = Duration::from_secs(5);

/// One daemon-lifetime, suspend-inclusive monotonic clock reading.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PublicationInstant(Duration);

impl PublicationInstant {
    #[cfg(test)]
    const fn from_duration(duration: Duration) -> Self {
        Self(duration)
    }

    fn checked_add(self, duration: Duration) -> Result<Self, PublicationRegistryError> {
        self.0
            .checked_add(duration)
            .map(Self)
            .ok_or(PublicationRegistryError::ClockOverflow)
    }

    fn duration_since(self, earlier: Self) -> Option<Duration> {
        self.0.checked_sub(earlier.0)
    }
}

impl fmt::Debug for PublicationInstant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublicationInstant(<monotonic>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
enum PublicationClockError {
    #[error("the suspend-inclusive publication clock is unavailable")]
    Unavailable,
}

trait PublicationClock: Send + Sync + fmt::Debug {
    fn now(&self) -> Result<PublicationInstant, PublicationClockError>;
}

type SharedPublicationClock = Arc<dyn PublicationClock>;

/// Production suspend-inclusive clock.
#[derive(Debug, Clone, Copy, Default)]
struct SystemPublicationClock;

#[cfg(target_os = "linux")]
impl PublicationClock for SystemPublicationClock {
    #[allow(unsafe_code)]
    fn now(&self) -> Result<PublicationInstant, PublicationClockError> {
        let mut value = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `value` is a valid writable `timespec`, and CLOCK_BOOTTIME
        // requires no additional lifetime or ownership contract.
        if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut value) } != 0 {
            return Err(PublicationClockError::Unavailable);
        }
        let seconds =
            u64::try_from(value.tv_sec).map_err(|_| PublicationClockError::Unavailable)?;
        let nanos = u32::try_from(value.tv_nsec).map_err(|_| PublicationClockError::Unavailable)?;
        Ok(PublicationInstant(Duration::new(seconds, nanos)))
    }
}

#[cfg(target_os = "macos")]
impl PublicationClock for SystemPublicationClock {
    #[allow(unsafe_code)]
    fn now(&self) -> Result<PublicationInstant, PublicationClockError> {
        #[repr(C)]
        struct MachTimebaseInfo {
            numer: u32,
            denom: u32,
        }

        unsafe extern "C" {
            fn mach_continuous_time() -> u64;
            fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
        }

        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid writable C representation and both calls
        // have no caller-owned lifetime obligations.
        let status = unsafe { mach_timebase_info(&raw mut info) };
        if status != 0 || info.denom == 0 {
            return Err(PublicationClockError::Unavailable);
        }
        // SAFETY: `mach_continuous_time` has no preconditions.
        let ticks = unsafe { mach_continuous_time() };
        let nanos = u128::from(ticks)
            .checked_mul(u128::from(info.numer))
            .and_then(|value| value.checked_div(u128::from(info.denom)))
            .ok_or(PublicationClockError::Unavailable)?;
        let seconds =
            u64::try_from(nanos / 1_000_000_000).map_err(|_| PublicationClockError::Unavailable)?;
        let subsecond =
            u32::try_from(nanos % 1_000_000_000).map_err(|_| PublicationClockError::Unavailable)?;
        Ok(PublicationInstant(Duration::new(seconds, subsecond)))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl PublicationClock for SystemPublicationClock {
    fn now(&self) -> Result<PublicationInstant, PublicationClockError> {
        Err(PublicationClockError::Unavailable)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct AuthorityToken([u8; 32]);

impl AuthorityToken {
    fn random() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    #[cfg(test)]
    const fn from_byte(byte: u8) -> Self {
        Self([byte; 32])
    }
}

impl fmt::Debug for AuthorityToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted authority token>")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct DaemonEpoch(AuthorityToken);

impl DaemonEpoch {
    fn random() -> Self {
        Self(AuthorityToken::random())
    }
}

impl fmt::Debug for DaemonEpoch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DaemonEpoch(<redacted>)")
    }
}

/// Kernel-observed process birth evidence. This is never persisted.
#[derive(Clone, PartialEq, Eq, Hash)]
enum PublisherProcessBirth {
    MacOs {
        start_seconds: u64,
        start_microseconds: u64,
    },
    Linux {
        boot_id: Box<str>,
        start_ticks: u64,
    },
    #[cfg(test)]
    Test(u64),
}

impl fmt::Debug for PublisherProcessBirth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublisherProcessBirth(<redacted>)")
    }
}

/// Exact same-user publisher authority obtained from kernel credentials.
#[derive(Clone, PartialEq, Eq, Hash)]
struct PublisherPrincipal {
    uid: u32,
    pid: u32,
    birth: PublisherProcessBirth,
}

impl PublisherPrincipal {
    fn new(uid: u32, pid: u32, birth: PublisherProcessBirth) -> Self {
        Self { uid, pid, birth }
    }
}

impl fmt::Debug for PublisherPrincipal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublisherPrincipal(<redacted>)")
    }
}

/// Kernel identity of one retained listener capability.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ListenerIdentity {
    MacOsIpv4 {
        address: [u8; 4],
        port: u16,
    },
    LinuxIpv4 {
        address: [u8; 4],
        port: u16,
        socket_cookie: u64,
        network_namespace_cookie: u64,
    },
    #[cfg(test)]
    Test(u64),
}

impl fmt::Debug for ListenerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ListenerIdentity(<redacted>)")
    }
}

/// Owned guard that preserves one validated listener's address authority.
struct RetainedListenerCapability {
    identity: ListenerIdentity,
    guard: Arc<dyn Send + Sync>,
}

impl RetainedListenerCapability {
    fn new(identity: ListenerIdentity, guard: Arc<dyn Send + Sync>) -> Self {
        Self { identity, guard }
    }

    const fn identity(&self) -> &ListenerIdentity {
        &self.identity
    }
}

impl fmt::Debug for RetainedListenerCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.guard;
        formatter.write_str("RetainedListenerCapability(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct AcquisitionAttemptHandle {
    epoch: DaemonEpoch,
    service: ServiceKey,
    generation: u64,
    token: AuthorityToken,
}

impl fmt::Debug for AcquisitionAttemptHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AcquisitionAttemptHandle(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct RebindAttemptHandle {
    epoch: DaemonEpoch,
    service: ServiceKey,
    lease_generation: u64,
    token: AuthorityToken,
}

impl fmt::Debug for RebindAttemptHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RebindAttemptHandle(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct LeaseHandle {
    epoch: DaemonEpoch,
    service: ServiceKey,
    generation: u64,
    token: AuthorityToken,
}

impl fmt::Debug for LeaseHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LeaseHandle(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptState {
    Pending,
    InFlight,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalAttemptFailure {
    EndpointUnhealthy,
    OperationCanceled,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LeaseSchedule {
    renew_after: Duration,
    expires_in: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparationFence {
    service: ServiceKey,
    generation: u64,
    principal: PublisherPrincipal,
    configuration_revision: u64,
    deadline: PublicationInstant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcquisitionFence {
    handle: AcquisitionAttemptHandle,
    principal: PublisherPrincipal,
    listener: ListenerIdentity,
    acknowledged_origin: SemanticOrigin,
    configuration_revision: u64,
    deadline: PublicationInstant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RebindFence {
    handle: RebindAttemptHandle,
    lease: LeaseHandle,
    principal: PublisherPrincipal,
    listener: ListenerIdentity,
    acknowledged_origin: SemanticOrigin,
    expected_binding_revision: u64,
    configuration_revision: u64,
    deadline: PublicationInstant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpiryFence {
    lease: LeaseHandle,
    renewal_revision: u64,
    deadline: PublicationInstant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BeginPreparation {
    Started(PreparationFence),
    Joined(PreparationFence),
    ExistingAttempt {
        handle: AcquisitionAttemptHandle,
        state: AttemptState,
        origin: SemanticOrigin,
        expires_in: Duration,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BeginAcquire {
    Started(AcquisitionFence),
    Joined(AcquisitionFence),
    Terminal(TerminalAttemptFailure),
    Replay(LeaseGrant),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BeginRebind {
    Started {
        handle: RebindAttemptHandle,
        origin: SemanticOrigin,
        expires_in: Duration,
    },
    Existing {
        handle: RebindAttemptHandle,
        state: AttemptState,
        origin: SemanticOrigin,
        expires_in: Duration,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BeginRebindCandidate {
    Started(RebindFence),
    Joined(RebindFence),
    Terminal(TerminalAttemptFailure),
    Replay(LeaseGrant),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LeaseGrant {
    lease: LeaseHandle,
    binding_revision: u64,
    origin: SemanticOrigin,
    schedule: LeaseSchedule,
    expiry_fence: ExpiryFence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicationProjection {
    state: PublicationState,
    origin: SemanticOrigin,
}

#[derive(Debug, Default)]
struct PublicationEffects {
    projection_changed: BTreeSet<ServiceKey>,
    probe_required: BTreeSet<ServiceKey>,
    retired_capabilities: Vec<RetainedListenerCapability>,
}

#[derive(Debug)]
struct PublicationOutcome<T> {
    result: Result<T, PublicationRegistryError>,
    effects: PublicationEffects,
}

impl<T> PublicationOutcome<T> {
    fn ok(value: T, effects: PublicationEffects) -> Self {
        Self {
            result: Ok(value),
            effects,
        }
    }

    fn err(error: PublicationRegistryError, effects: PublicationEffects) -> Self {
        Self {
            result: Err(error),
            effects,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
enum PublicationRegistryError {
    #[error("the publication clock is unavailable")]
    ClockUnavailable,
    #[error("the publication clock regressed")]
    ClockRegressed,
    #[error("the publication clock overflowed")]
    ClockOverflow,
    #[error("published service is not declared")]
    ServiceNotDeclared,
    #[error("published service belongs to a missing project instance")]
    InstanceMissing,
    #[error("published service already has a live publisher")]
    AlreadyPublished,
    #[error("another publisher owns the current acquisition attempt")]
    AcquisitionInProgress,
    #[error("another publisher owns the current rebind attempt")]
    RebindInProgress,
    #[error("the publication attempt is stale")]
    AttemptStale,
    #[error("the publication attempt expired")]
    AttemptExpired,
    #[error("the publication attempt does not match this request")]
    AttemptMismatch,
    #[error("the publication lease was lost")]
    LeaseLost,
    #[error("the publication binding was replaced")]
    BindingReplaced,
    #[error("the acknowledged semantic origin does not match")]
    OriginMismatch,
    #[error("the configuration revision is stale or inconsistent")]
    DeclarationConflict,
    #[error("a publication generation or revision overflowed")]
    GenerationOverflow,
    #[error("the deadline has not elapsed")]
    DeadlineNotElapsed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeclarationAuthority(PublishedServiceDeclaration);

impl DeclarationAuthority {
    fn key(&self) -> ServiceKey {
        ServiceKey::new(self.0.project_instance_id, self.0.service_name.clone())
    }

    fn routing_equivalent(&self, other: &Self) -> bool {
        self.0.origin == other.0.origin
    }

    fn health_equivalent(&self, other: &Self) -> bool {
        self.0.health_policy == other.0.health_policy
    }
}

#[derive(Debug)]
struct Preparation {
    fence: PreparationFence,
}

#[derive(Debug)]
struct AcquisitionRequest {
    fence: AcquisitionFence,
}

#[derive(Debug)]
enum AcquisitionPhase {
    Pending,
    InFlight(AcquisitionRequest),
    Terminal {
        request: AcquisitionRequest,
        failure: TerminalAttemptFailure,
    },
}

#[derive(Debug)]
struct AcquisitionAttempt {
    handle: AcquisitionAttemptHandle,
    principal: PublisherPrincipal,
    origin: SemanticOrigin,
    configuration_revision: u64,
    deadline: PublicationInstant,
    phase: AcquisitionPhase,
}

impl AcquisitionAttempt {
    fn state(&self) -> AttemptState {
        match self.phase {
            AcquisitionPhase::Pending => AttemptState::Pending,
            AcquisitionPhase::InFlight(_) => AttemptState::InFlight,
            AcquisitionPhase::Terminal { .. } => AttemptState::Terminal,
        }
    }
}

#[derive(Debug)]
struct AcquisitionReplay {
    handle: AcquisitionAttemptHandle,
    listener: ListenerIdentity,
    acknowledged_origin: SemanticOrigin,
}

#[derive(Debug)]
struct RebindRequest {
    fence: RebindFence,
}

#[derive(Debug)]
enum RebindPhase {
    Pending,
    InFlight(RebindRequest),
    TerminalFailure {
        request: RebindRequest,
        failure: TerminalAttemptFailure,
    },
    TerminalSuccess {
        request: RebindRequest,
        installed_binding_revision: u64,
    },
}

#[derive(Debug)]
struct RebindAttempt {
    handle: RebindAttemptHandle,
    principal: PublisherPrincipal,
    expected_binding_revision: u64,
    deadline: PublicationInstant,
    phase: RebindPhase,
}

impl RebindAttempt {
    fn state(&self) -> AttemptState {
        match self.phase {
            RebindPhase::Pending => AttemptState::Pending,
            RebindPhase::InFlight(_) => AttemptState::InFlight,
            RebindPhase::TerminalFailure { .. } | RebindPhase::TerminalSuccess { .. } => {
                AttemptState::Terminal
            }
        }
    }
}

#[derive(Debug)]
struct LiveLease {
    handle: LeaseHandle,
    principal: PublisherPrincipal,
    binding_revision: u64,
    renewal_revision: u64,
    renew_at: PublicationInstant,
    deadline: PublicationInstant,
    capability: RetainedListenerCapability,
    acknowledged_origin: SemanticOrigin,
    acquisition_replay: AcquisitionReplay,
    rebind: Option<RebindAttempt>,
}

#[derive(Debug)]
enum SlotState {
    Vacant,
    Preparing(Preparation),
    Attempt(Box<AcquisitionAttempt>),
    Live(Box<LiveLease>),
}

#[derive(Debug)]
struct PublicationSlot {
    declaration: DeclarationAuthority,
    paused: bool,
    missing: bool,
    last_generation: u64,
    state: SlotState,
}

/// Pure, synchronous, constant-space publication authority registry.
struct PublicationRegistry {
    clock: SharedPublicationClock,
    epoch: DaemonEpoch,
    last_now: Option<PublicationInstant>,
    configuration_revisions: BTreeMap<ProjectInstanceId, u64>,
    slots: BTreeMap<ServiceKey, PublicationSlot>,
}

impl fmt::Debug for PublicationRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicationRegistry")
            .field("epoch", &"<redacted>")
            .field("slot_count", &self.slots.len())
            .finish_non_exhaustive()
    }
}

impl PublicationRegistry {
    fn new(clock: SharedPublicationClock) -> Self {
        Self {
            clock,
            epoch: DaemonEpoch::random(),
            last_now: None,
            configuration_revisions: BTreeMap::new(),
            slots: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    fn with_epoch(clock: SharedPublicationClock, epoch: DaemonEpoch) -> Self {
        Self {
            clock,
            epoch,
            last_now: None,
            configuration_revisions: BTreeMap::new(),
            slots: BTreeMap::new(),
        }
    }

    fn epoch(&self) -> DaemonEpoch {
        self.epoch.clone()
    }

    fn declared_len(&self) -> usize {
        self.slots.len()
    }

    fn projection(&self, key: &ServiceKey) -> Option<PublicationProjection> {
        self.slots.get(key).map(|slot| PublicationProjection {
            state: if slot.missing {
                PublicationState::InstanceMissing
            } else if slot.paused {
                PublicationState::RoutePaused
            } else if matches!(slot.state, SlotState::Live(_)) {
                PublicationState::CheckingEndpoint
            } else {
                PublicationState::WaitingForPublisher
            },
            origin: slot.declaration.0.origin.clone(),
        })
    }

    /// Read one public projection after enforcing all elapsed authority.
    fn snapshot(&mut self, key: &ServiceKey) -> PublicationOutcome<Option<PublicationProjection>> {
        let (_, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        PublicationOutcome::ok(self.projection(key), effects)
    }

    fn reconcile_declarations(
        &mut self,
        instance: ProjectInstanceId,
        configuration_revision: u64,
        declarations: impl IntoIterator<Item = PublishedServiceDeclaration>,
    ) -> PublicationOutcome<()> {
        let (_, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let mut candidates = BTreeMap::new();
        for declaration in declarations.into_iter().map(DeclarationAuthority) {
            if declaration.0.project_instance_id != instance
                || declaration.0.configuration_revision != configuration_revision
                || configuration_revision == 0
            {
                return PublicationOutcome::err(
                    PublicationRegistryError::DeclarationConflict,
                    effects,
                );
            }
            let key = declaration.key();
            if candidates
                .insert(key, declaration.clone())
                .is_some_and(|existing| existing != declaration)
            {
                return PublicationOutcome::err(
                    PublicationRegistryError::DeclarationConflict,
                    effects,
                );
            }
        }

        let current_revision = self.configuration_revisions.get(&instance).copied();
        if current_revision.is_some_and(|current| configuration_revision < current) {
            return PublicationOutcome::err(PublicationRegistryError::DeclarationConflict, effects);
        }
        if current_revision == Some(configuration_revision) {
            let current = self
                .slots
                .iter()
                .filter(|(key, _)| key.instance() == instance)
                .map(|(key, slot)| (key.clone(), slot.declaration.clone()))
                .collect::<BTreeMap<_, _>>();
            if current != candidates {
                return PublicationOutcome::err(
                    PublicationRegistryError::DeclarationConflict,
                    effects,
                );
            }
            return PublicationOutcome::ok((), effects);
        }
        let may_transfer_authority = current_revision.and_then(|current| current.checked_add(1))
            == Some(configuration_revision);

        let removed = self
            .slots
            .keys()
            .filter(|key| key.instance() == instance && !candidates.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed {
            if let Some(mut slot) = self.slots.remove(&key) {
                Self::retire_slot_state(&key, &mut slot, &mut effects);
                effects.projection_changed.insert(key);
            }
        }

        for (key, candidate) in candidates {
            let Some(slot) = self.slots.get_mut(&key) else {
                self.slots.insert(
                    key.clone(),
                    PublicationSlot {
                        declaration: candidate,
                        paused: false,
                        missing: false,
                        last_generation: 0,
                        state: SlotState::Vacant,
                    },
                );
                effects.projection_changed.insert(key);
                continue;
            };

            if candidate == slot.declaration {
                continue;
            }

            let routing_equivalent = slot.declaration.routing_equivalent(&candidate);
            let health_equivalent = slot.declaration.health_equivalent(&candidate);
            match &mut slot.state {
                SlotState::Live(lease) if may_transfer_authority && routing_equivalent => {
                    lease.rebind = None;
                    if !health_equivalent {
                        effects.probe_required.insert(key.clone());
                    }
                }
                SlotState::Vacant if may_transfer_authority && routing_equivalent => {}
                SlotState::Vacant
                | SlotState::Preparing(_)
                | SlotState::Attempt(_)
                | SlotState::Live(_) => Self::retire_slot_state(&key, slot, &mut effects),
            }
            slot.declaration = candidate;
            effects.projection_changed.insert(key);
        }

        self.configuration_revisions
            .insert(instance, configuration_revision);

        PublicationOutcome::ok((), effects)
    }

    fn begin_preparation(
        &mut self,
        key: &ServiceKey,
        principal: PublisherPrincipal,
    ) -> PublicationOutcome<BeginPreparation> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(key) else {
            return PublicationOutcome::err(PublicationRegistryError::ServiceNotDeclared, effects);
        };
        if slot.missing {
            return PublicationOutcome::err(PublicationRegistryError::InstanceMissing, effects);
        }
        let result = match &slot.state {
            SlotState::Vacant => {
                let Some(generation) = slot.last_generation.checked_add(1) else {
                    return PublicationOutcome::err(
                        PublicationRegistryError::GenerationOverflow,
                        effects,
                    );
                };
                let deadline = match now.checked_add(PREPARATION_TTL) {
                    Ok(deadline) => deadline,
                    Err(error) => return PublicationOutcome::err(error, effects),
                };
                let fence = PreparationFence {
                    service: key.clone(),
                    generation,
                    principal,
                    configuration_revision: slot.declaration.0.configuration_revision,
                    deadline,
                };
                slot.last_generation = generation;
                slot.state = SlotState::Preparing(Preparation {
                    fence: fence.clone(),
                });
                BeginPreparation::Started(fence)
            }
            SlotState::Preparing(preparation) if preparation.fence.principal == principal => {
                BeginPreparation::Joined(preparation.fence.clone())
            }
            SlotState::Preparing(_) => {
                return PublicationOutcome::err(
                    PublicationRegistryError::AcquisitionInProgress,
                    effects,
                );
            }
            SlotState::Attempt(attempt) if attempt.principal == principal => {
                BeginPreparation::ExistingAttempt {
                    handle: attempt.handle.clone(),
                    state: attempt.state(),
                    origin: attempt.origin.clone(),
                    expires_in: attempt.deadline.duration_since(now).unwrap_or_default(),
                }
            }
            SlotState::Attempt(_) => {
                return PublicationOutcome::err(
                    PublicationRegistryError::AcquisitionInProgress,
                    effects,
                );
            }
            SlotState::Live(_) => {
                return PublicationOutcome::err(
                    PublicationRegistryError::AlreadyPublished,
                    effects,
                );
            }
        };
        PublicationOutcome::ok(result, effects)
    }

    fn complete_preparation(
        &mut self,
        fence: &PreparationFence,
    ) -> PublicationOutcome<AcquisitionAttemptHandle> {
        let (now, effects) = match self.begin_transition_except(&fence.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Preparing(current) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if current.fence != *fence
            || current.fence.configuration_revision != slot.declaration.0.configuration_revision
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        if current.fence.deadline <= now {
            slot.state = SlotState::Vacant;
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        let deadline = match now.checked_add(ACQUISITION_ATTEMPT_TTL) {
            Ok(deadline) => deadline,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        let handle = AcquisitionAttemptHandle {
            epoch: self.epoch.clone(),
            service: fence.service.clone(),
            generation: fence.generation,
            token: AuthorityToken::random(),
        };
        slot.state = SlotState::Attempt(Box::new(AcquisitionAttempt {
            handle: handle.clone(),
            principal: fence.principal.clone(),
            origin: slot.declaration.0.origin.clone(),
            configuration_revision: slot.declaration.0.configuration_revision,
            deadline,
            phase: AcquisitionPhase::Pending,
        }));
        PublicationOutcome::ok(handle, effects)
    }

    fn fail_preparation(&mut self, fence: &PreparationFence) -> PublicationOutcome<()> {
        let (_, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if !matches!(&slot.state, SlotState::Preparing(current) if current.fence == *fence) {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        slot.state = SlotState::Vacant;
        PublicationOutcome::ok((), effects)
    }

    fn begin_acquire(
        &mut self,
        handle: &AcquisitionAttemptHandle,
        principal: &PublisherPrincipal,
        acknowledged_origin: &SemanticOrigin,
        listener: &ListenerIdentity,
    ) -> PublicationOutcome<BeginAcquire> {
        let (now, mut effects) = match self.begin_transition_except(&handle.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        if handle.epoch != self.epoch {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        let Some(slot) = self.slots.get_mut(&handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if slot.declaration.0.origin != *acknowledged_origin {
            return PublicationOutcome::err(PublicationRegistryError::OriginMismatch, effects);
        }

        if matches!(&slot.state, SlotState::Live(lease) if lease.deadline <= now) {
            Self::retire_slot_state(&handle.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }

        if let SlotState::Live(lease) = &slot.state {
            let replay = &lease.acquisition_replay;
            if replay.handle == *handle
                && lease.principal == *principal
                && replay.listener == *listener
                && replay.acknowledged_origin == *acknowledged_origin
            {
                if lease.binding_revision != 1 {
                    return PublicationOutcome::err(
                        PublicationRegistryError::BindingReplaced,
                        effects,
                    );
                }
                return PublicationOutcome::ok(
                    BeginAcquire::Replay(Self::lease_grant(lease, now)),
                    effects,
                );
            }
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }

        let SlotState::Attempt(attempt) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if attempt.handle != *handle || attempt.principal != *principal {
            return PublicationOutcome::err(PublicationRegistryError::AttemptMismatch, effects);
        }
        if attempt.deadline <= now {
            slot.state = SlotState::Vacant;
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        let fence = AcquisitionFence {
            handle: handle.clone(),
            principal: principal.clone(),
            listener: listener.clone(),
            acknowledged_origin: acknowledged_origin.clone(),
            configuration_revision: attempt.configuration_revision,
            deadline: attempt.deadline,
        };
        let result = match &attempt.phase {
            AcquisitionPhase::Pending => {
                attempt.phase = AcquisitionPhase::InFlight(AcquisitionRequest {
                    fence: fence.clone(),
                });
                BeginAcquire::Started(fence)
            }
            AcquisitionPhase::InFlight(request) if request.fence == fence => {
                BeginAcquire::Joined(request.fence.clone())
            }
            AcquisitionPhase::Terminal { request, failure } if request.fence == fence => {
                BeginAcquire::Terminal(*failure)
            }
            AcquisitionPhase::InFlight(_) | AcquisitionPhase::Terminal { .. } => {
                return PublicationOutcome::err(PublicationRegistryError::AttemptMismatch, effects);
            }
        };
        PublicationOutcome::ok(result, effects)
    }

    fn commit_acquire(
        &mut self,
        fence: &AcquisitionFence,
        capability: RetainedListenerCapability,
    ) -> PublicationOutcome<LeaseGrant> {
        let (now, mut effects) = match self.begin_transition_except(&fence.handle.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        if capability.identity() != &fence.listener {
            return PublicationOutcome::err(PublicationRegistryError::AttemptMismatch, effects);
        }
        let Some(slot) = self.slots.get_mut(&fence.handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Attempt(attempt) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if attempt.handle != fence.handle
            || attempt.principal != fence.principal
            || attempt.deadline != fence.deadline
            || attempt.configuration_revision != fence.configuration_revision
            || slot.declaration.0.configuration_revision != fence.configuration_revision
            || slot.declaration.0.origin != fence.acknowledged_origin
            || !matches!(&attempt.phase, AcquisitionPhase::InFlight(request) if request.fence == *fence)
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        if attempt.deadline <= now {
            slot.state = SlotState::Vacant;
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        let deadline = match now.checked_add(LEASE_TTL) {
            Ok(deadline) => deadline,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        let renew_at = match now.checked_add(RENEW_AFTER) {
            Ok(renew_at) => renew_at,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        let lease_handle = LeaseHandle {
            epoch: self.epoch.clone(),
            service: fence.handle.service.clone(),
            generation: fence.handle.generation,
            token: AuthorityToken::random(),
        };
        let lease = LiveLease {
            handle: lease_handle,
            principal: fence.principal.clone(),
            binding_revision: 1,
            renewal_revision: 0,
            renew_at,
            deadline,
            capability,
            acknowledged_origin: fence.acknowledged_origin.clone(),
            acquisition_replay: AcquisitionReplay {
                handle: fence.handle.clone(),
                listener: fence.listener.clone(),
                acknowledged_origin: fence.acknowledged_origin.clone(),
            },
            rebind: None,
        };
        let grant = Self::lease_grant(&lease, now);
        slot.state = SlotState::Live(Box::new(lease));
        effects
            .projection_changed
            .insert(fence.handle.service.clone());
        effects.probe_required.insert(fence.handle.service.clone());
        PublicationOutcome::ok(grant, effects)
    }

    fn fail_acquire(
        &mut self,
        fence: &AcquisitionFence,
        failure: TerminalAttemptFailure,
    ) -> PublicationOutcome<()> {
        let (now, effects) = match self.begin_transition_except(&fence.handle.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Attempt(attempt) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if !matches!(&attempt.phase, AcquisitionPhase::InFlight(request) if request.fence == *fence)
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        if attempt.deadline <= now {
            slot.state = SlotState::Vacant;
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        attempt.phase = AcquisitionPhase::Terminal {
            request: AcquisitionRequest {
                fence: fence.clone(),
            },
            failure,
        };
        PublicationOutcome::ok((), effects)
    }

    fn replace_terminal_acquisition(
        &mut self,
        current: &AcquisitionAttemptHandle,
        principal: &PublisherPrincipal,
    ) -> PublicationOutcome<AcquisitionAttemptHandle> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&current.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Attempt(attempt) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if attempt.handle != *current
            || attempt.principal != *principal
            || !matches!(attempt.phase, AcquisitionPhase::Terminal { .. })
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        let Some(generation) = slot.last_generation.checked_add(1) else {
            return PublicationOutcome::err(PublicationRegistryError::GenerationOverflow, effects);
        };
        let deadline = match now.checked_add(ACQUISITION_ATTEMPT_TTL) {
            Ok(deadline) => deadline,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        let handle = AcquisitionAttemptHandle {
            epoch: self.epoch.clone(),
            service: current.service.clone(),
            generation,
            token: AuthorityToken::random(),
        };
        slot.last_generation = generation;
        slot.state = SlotState::Attempt(Box::new(AcquisitionAttempt {
            handle: handle.clone(),
            principal: principal.clone(),
            origin: slot.declaration.0.origin.clone(),
            configuration_revision: slot.declaration.0.configuration_revision,
            deadline,
            phase: AcquisitionPhase::Pending,
        }));
        PublicationOutcome::ok(handle, effects)
    }

    fn renew(
        &mut self,
        handle: &LeaseHandle,
        principal: &PublisherPrincipal,
    ) -> PublicationOutcome<LeaseGrant> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        let SlotState::Live(lease) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if lease.handle != *handle || lease.principal != *principal || lease.deadline <= now {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        let Some(renewal_revision) = lease.renewal_revision.checked_add(1) else {
            return PublicationOutcome::err(PublicationRegistryError::GenerationOverflow, effects);
        };
        let deadline = match now.checked_add(LEASE_TTL) {
            Ok(deadline) => deadline,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        let renew_at = match now.checked_add(RENEW_AFTER) {
            Ok(renew_at) => renew_at,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        lease.renewal_revision = renewal_revision;
        lease.renew_at = renew_at;
        lease.deadline = deadline;
        PublicationOutcome::ok(Self::lease_grant(lease, now), effects)
    }

    fn release(
        &mut self,
        handle: &LeaseHandle,
        principal: &PublisherPrincipal,
    ) -> PublicationOutcome<()> {
        let (_, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if !matches!(&slot.state, SlotState::Live(lease) if lease.handle == *handle && lease.principal == *principal)
        {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        Self::retire_slot_state(&handle.service, slot, &mut effects);
        PublicationOutcome::ok((), effects)
    }

    fn expire(&mut self, fence: &ExpiryFence) -> PublicationOutcome<bool> {
        let (now, mut effects) = match self.begin_transition_except(&fence.lease.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.lease.service) else {
            return PublicationOutcome::ok(false, effects);
        };
        let SlotState::Live(lease) = &slot.state else {
            return PublicationOutcome::ok(false, effects);
        };
        let matches_lease = lease.handle == fence.lease;
        let matches_renewal = lease.renewal_revision == fence.renewal_revision;
        let matches_deadline = lease.deadline == fence.deadline;
        if !(matches_lease && matches_renewal && matches_deadline) {
            return PublicationOutcome::ok(false, effects);
        }
        if fence.deadline > now {
            return PublicationOutcome::err(PublicationRegistryError::DeadlineNotElapsed, effects);
        }
        Self::retire_slot_state(&fence.lease.service, slot, &mut effects);
        PublicationOutcome::ok(true, effects)
    }

    fn begin_rebind(
        &mut self,
        lease_handle: &LeaseHandle,
        principal: &PublisherPrincipal,
        expected_binding_revision: u64,
    ) -> PublicationOutcome<BeginRebind> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&lease_handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        let SlotState::Live(lease) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if lease.handle != *lease_handle || lease.principal != *principal || lease.deadline <= now {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        if lease.binding_revision != expected_binding_revision {
            return PublicationOutcome::err(PublicationRegistryError::BindingReplaced, effects);
        }
        let result = match &lease.rebind {
            Some(attempt)
                if attempt.principal == *principal
                    && attempt.expected_binding_revision == expected_binding_revision =>
            {
                BeginRebind::Existing {
                    handle: attempt.handle.clone(),
                    state: attempt.state(),
                    origin: slot.declaration.0.origin.clone(),
                    expires_in: attempt.deadline.duration_since(now).unwrap_or_default(),
                }
            }
            Some(_) => {
                return PublicationOutcome::err(
                    PublicationRegistryError::RebindInProgress,
                    effects,
                );
            }
            None => {
                let deadline = match now.checked_add(REBIND_ATTEMPT_TTL) {
                    Ok(deadline) => deadline,
                    Err(error) => return PublicationOutcome::err(error, effects),
                };
                let handle = RebindAttemptHandle {
                    epoch: self.epoch.clone(),
                    service: lease_handle.service.clone(),
                    lease_generation: lease_handle.generation,
                    token: AuthorityToken::random(),
                };
                lease.rebind = Some(RebindAttempt {
                    handle: handle.clone(),
                    principal: principal.clone(),
                    expected_binding_revision,
                    deadline,
                    phase: RebindPhase::Pending,
                });
                BeginRebind::Started {
                    handle,
                    origin: slot.declaration.0.origin.clone(),
                    expires_in: REBIND_ATTEMPT_TTL,
                }
            }
        };
        PublicationOutcome::ok(result, effects)
    }

    fn begin_rebind_candidate(
        &mut self,
        handle: &RebindAttemptHandle,
        principal: &PublisherPrincipal,
        acknowledged_origin: &SemanticOrigin,
        listener: &ListenerIdentity,
    ) -> PublicationOutcome<BeginRebindCandidate> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        if handle.epoch != self.epoch {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        let Some(slot) = self.slots.get_mut(&handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if slot.declaration.0.origin != *acknowledged_origin {
            return PublicationOutcome::err(PublicationRegistryError::OriginMismatch, effects);
        }
        let SlotState::Live(lease) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if lease.principal != *principal || lease.deadline <= now {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        let Some(attempt) = &mut lease.rebind else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if attempt.handle != *handle || attempt.principal != *principal {
            return PublicationOutcome::err(PublicationRegistryError::AttemptMismatch, effects);
        }
        let fence = RebindFence {
            handle: handle.clone(),
            lease: lease.handle.clone(),
            principal: principal.clone(),
            listener: listener.clone(),
            acknowledged_origin: acknowledged_origin.clone(),
            expected_binding_revision: attempt.expected_binding_revision,
            configuration_revision: slot.declaration.0.configuration_revision,
            deadline: attempt.deadline,
        };
        let result = match &attempt.phase {
            RebindPhase::Pending => {
                attempt.phase = RebindPhase::InFlight(RebindRequest {
                    fence: fence.clone(),
                });
                BeginRebindCandidate::Started(fence)
            }
            RebindPhase::InFlight(request) if request.fence == fence => {
                BeginRebindCandidate::Joined(request.fence.clone())
            }
            RebindPhase::TerminalFailure { request, failure } if request.fence == fence => {
                BeginRebindCandidate::Terminal(*failure)
            }
            RebindPhase::TerminalSuccess {
                request,
                installed_binding_revision,
            } if request.fence == fence
                && lease.binding_revision == *installed_binding_revision =>
            {
                BeginRebindCandidate::Replay(Self::lease_grant(lease, now))
            }
            RebindPhase::InFlight(_)
            | RebindPhase::TerminalFailure { .. }
            | RebindPhase::TerminalSuccess { .. } => {
                return PublicationOutcome::err(PublicationRegistryError::AttemptMismatch, effects);
            }
        };
        PublicationOutcome::ok(result, effects)
    }

    fn commit_rebind(
        &mut self,
        fence: &RebindFence,
        capability: RetainedListenerCapability,
    ) -> PublicationOutcome<LeaseGrant> {
        let (now, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        if capability.identity() != &fence.listener {
            return PublicationOutcome::err(PublicationRegistryError::AttemptMismatch, effects);
        }
        let Some(slot) = self.slots.get_mut(&fence.handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        let SlotState::Live(lease) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        let matches_lease = lease.handle == fence.lease;
        let matches_principal = lease.principal == fence.principal;
        let matches_binding = lease.binding_revision == fence.expected_binding_revision;
        let matches_declaration = slot.declaration.0.configuration_revision
            == fence.configuration_revision
            && slot.declaration.0.origin == fence.acknowledged_origin;
        let matches_rebind = matches!(
            &lease.rebind,
            Some(attempt)
                if attempt.handle == fence.handle
                    && matches!(
                        &attempt.phase,
                        RebindPhase::InFlight(request) if request.fence == *fence
                    )
        );
        if !(matches_lease
            && matches_principal
            && matches_binding
            && lease.deadline > now
            && matches_declaration
            && matches_rebind)
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        let Some(binding_revision) = lease.binding_revision.checked_add(1) else {
            return PublicationOutcome::err(PublicationRegistryError::GenerationOverflow, effects);
        };
        effects
            .retired_capabilities
            .push(std::mem::replace(&mut lease.capability, capability));
        lease.binding_revision = binding_revision;
        lease.acknowledged_origin = fence.acknowledged_origin.clone();
        lease.rebind = Some(RebindAttempt {
            handle: fence.handle.clone(),
            principal: fence.principal.clone(),
            expected_binding_revision: fence.expected_binding_revision,
            deadline: fence.deadline,
            phase: RebindPhase::TerminalSuccess {
                request: RebindRequest {
                    fence: fence.clone(),
                },
                installed_binding_revision: binding_revision,
            },
        });
        effects.probe_required.insert(fence.handle.service.clone());
        PublicationOutcome::ok(Self::lease_grant(lease, now), effects)
    }

    fn fail_rebind(
        &mut self,
        fence: &RebindFence,
        failure: TerminalAttemptFailure,
    ) -> PublicationOutcome<()> {
        let (_, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        let SlotState::Live(lease) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        let Some(attempt) = &mut lease.rebind else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if !matches!(&attempt.phase, RebindPhase::InFlight(request) if request.fence == *fence) {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        attempt.phase = RebindPhase::TerminalFailure {
            request: RebindRequest {
                fence: fence.clone(),
            },
            failure,
        };
        PublicationOutcome::ok((), effects)
    }

    fn replace_terminal_rebind(
        &mut self,
        lease_handle: &LeaseHandle,
        principal: &PublisherPrincipal,
        current: &RebindAttemptHandle,
        expected_binding_revision: u64,
    ) -> PublicationOutcome<RebindAttemptHandle> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&lease_handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        let SlotState::Live(lease) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if lease.handle != *lease_handle
            || lease.principal != *principal
            || lease.binding_revision != expected_binding_revision
            || lease.deadline <= now
        {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        let Some(attempt) = &lease.rebind else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if attempt.handle != *current
            || attempt.principal != *principal
            || !matches!(
                attempt.phase,
                RebindPhase::TerminalFailure { .. } | RebindPhase::TerminalSuccess { .. }
            )
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        let deadline = match now.checked_add(REBIND_ATTEMPT_TTL) {
            Ok(deadline) => deadline,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        let handle = RebindAttemptHandle {
            epoch: self.epoch.clone(),
            service: lease_handle.service.clone(),
            lease_generation: lease_handle.generation,
            token: AuthorityToken::random(),
        };
        lease.rebind = Some(RebindAttempt {
            handle: handle.clone(),
            principal: principal.clone(),
            expected_binding_revision,
            deadline,
            phase: RebindPhase::Pending,
        });
        PublicationOutcome::ok(handle, effects)
    }

    fn set_paused(&mut self, instance: ProjectInstanceId, paused: bool) -> PublicationOutcome<()> {
        let (_, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        for (key, slot) in &mut self.slots {
            if key.instance() != instance || slot.paused == paused {
                continue;
            }
            slot.paused = paused;
            if let SlotState::Live(lease) = &mut slot.state {
                lease.rebind = None;
                effects.probe_required.insert(key.clone());
            }
            effects.projection_changed.insert(key.clone());
        }
        PublicationOutcome::ok((), effects)
    }

    fn set_missing(
        &mut self,
        instance: ProjectInstanceId,
        missing: bool,
    ) -> PublicationOutcome<()> {
        let (_, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        for (key, slot) in &mut self.slots {
            if key.instance() != instance || slot.missing == missing {
                continue;
            }
            if missing {
                Self::retire_slot_state(key, slot, &mut effects);
            }
            slot.missing = missing;
            effects.projection_changed.insert(key.clone());
        }
        PublicationOutcome::ok((), effects)
    }

    fn retire_instance(&mut self, instance: ProjectInstanceId) -> PublicationOutcome<()> {
        let (_, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let keys = self
            .slots
            .keys()
            .filter(|key| key.instance() == instance)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(mut slot) = self.slots.remove(&key) {
                Self::retire_slot_state(&key, &mut slot, &mut effects);
                effects.projection_changed.insert(key);
            }
        }
        PublicationOutcome::ok((), effects)
    }

    fn retire_principal(&mut self, principal: &PublisherPrincipal) -> PublicationOutcome<()> {
        let (_, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        for (key, slot) in &mut self.slots {
            let owned = match &slot.state {
                SlotState::Preparing(preparation) => preparation.fence.principal == *principal,
                SlotState::Attempt(attempt) => attempt.principal == *principal,
                SlotState::Live(lease) => lease.principal == *principal,
                SlotState::Vacant => false,
            };
            if owned {
                Self::retire_slot_state(key, slot, &mut effects);
            }
        }
        PublicationOutcome::ok((), effects)
    }

    fn wake_barrier(&mut self, trustworthy: bool) -> PublicationOutcome<()> {
        if !trustworthy {
            let effects = self.retire_all_authority();
            self.last_now = None;
            return PublicationOutcome::err(PublicationRegistryError::ClockUnavailable, effects);
        }
        let (_, mut effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        for (key, slot) in &mut self.slots {
            if matches!(
                &slot.state,
                SlotState::Attempt(attempt)
                    if matches!(attempt.phase, AcquisitionPhase::InFlight(_))
            ) {
                slot.state = SlotState::Vacant;
                continue;
            }
            match &mut slot.state {
                SlotState::Live(lease) => {
                    lease.rebind = None;
                    effects.probe_required.insert(key.clone());
                }
                SlotState::Vacant | SlotState::Preparing(_) | SlotState::Attempt(_) => {}
            }
        }
        PublicationOutcome::ok((), effects)
    }

    fn shutdown(&mut self) -> PublicationEffects {
        self.retire_all_authority()
    }

    fn lease_grant(lease: &LiveLease, now: PublicationInstant) -> LeaseGrant {
        let expires_in = lease.deadline.duration_since(now).unwrap_or_default();
        LeaseGrant {
            lease: lease.handle.clone(),
            binding_revision: lease.binding_revision,
            origin: lease.acknowledged_origin.clone(),
            schedule: LeaseSchedule {
                renew_after: lease
                    .renew_at
                    .duration_since(now)
                    .unwrap_or_default()
                    .min(expires_in),
                expires_in,
            },
            expiry_fence: ExpiryFence {
                lease: lease.handle.clone(),
                renewal_revision: lease.renewal_revision,
                deadline: lease.deadline,
            },
        }
    }

    fn begin_transition<T>(
        &mut self,
    ) -> Result<(PublicationInstant, PublicationEffects), PublicationOutcome<T>> {
        self.begin_transition_inner(None)
    }

    fn begin_transition_except<T>(
        &mut self,
        excluded: &ServiceKey,
    ) -> Result<(PublicationInstant, PublicationEffects), PublicationOutcome<T>> {
        self.begin_transition_inner(Some(excluded))
    }

    fn begin_transition_inner<T>(
        &mut self,
        excluded: Option<&ServiceKey>,
    ) -> Result<(PublicationInstant, PublicationEffects), PublicationOutcome<T>> {
        match self.observe_now() {
            Ok(now) => Ok((now, self.expire_elapsed(now, excluded))),
            Err((error, effects)) => Err(PublicationOutcome::err(error, effects)),
        }
    }

    fn observe_now(
        &mut self,
    ) -> Result<PublicationInstant, (PublicationRegistryError, PublicationEffects)> {
        let now = match self.clock.now() {
            Ok(now) => now,
            Err(PublicationClockError::Unavailable) => {
                let effects = self.retire_all_authority();
                self.last_now = None;
                return Err((PublicationRegistryError::ClockUnavailable, effects));
            }
        };
        if self.last_now.is_some_and(|last| now < last) {
            let effects = self.retire_all_authority();
            self.last_now = Some(now);
            return Err((PublicationRegistryError::ClockRegressed, effects));
        }
        self.last_now = Some(now);
        Ok(now)
    }

    fn expire_elapsed(
        &mut self,
        now: PublicationInstant,
        excluded: Option<&ServiceKey>,
    ) -> PublicationEffects {
        let mut effects = PublicationEffects::default();
        for (key, slot) in &mut self.slots {
            if excluded == Some(key) {
                continue;
            }
            let elapsed = match &slot.state {
                SlotState::Preparing(preparation) => preparation.fence.deadline <= now,
                SlotState::Attempt(attempt) => attempt.deadline <= now,
                SlotState::Live(lease) => lease.deadline <= now,
                SlotState::Vacant => false,
            };
            if elapsed {
                Self::retire_slot_state(key, slot, &mut effects);
            } else if let SlotState::Live(lease) = &mut slot.state {
                if lease
                    .rebind
                    .as_ref()
                    .is_some_and(|attempt| attempt.deadline <= now)
                {
                    lease.rebind = None;
                }
            }
        }
        effects
    }

    fn retire_slot_state(
        key: &ServiceKey,
        slot: &mut PublicationSlot,
        effects: &mut PublicationEffects,
    ) {
        let previous = std::mem::replace(&mut slot.state, SlotState::Vacant);
        if let SlotState::Live(lease) = previous {
            let lease = *lease;
            effects.retired_capabilities.push(lease.capability);
            effects.projection_changed.insert(key.clone());
        }
    }

    fn retire_all_authority(&mut self) -> PublicationEffects {
        let mut effects = PublicationEffects::default();
        for (key, slot) in &mut self.slots {
            Self::retire_slot_state(key, slot, &mut effects);
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locald_core::{DomainName, PublishedHttpHealthPolicy};
    use std::collections::BTreeSet;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Clone)]
    struct FakePublicationClock {
        state: Arc<StdMutex<Result<PublicationInstant, PublicationClockError>>>,
    }

    impl FakePublicationClock {
        fn new(now: Duration) -> Self {
            Self {
                state: Arc::new(StdMutex::new(Ok(PublicationInstant(now)))),
            }
        }

        fn set(&self, now: Duration) {
            *self.state.lock().expect("fake publication clock lock") = Ok(PublicationInstant(now));
        }

        fn advance(&self, duration: Duration) {
            let mut state = self.state.lock().expect("fake publication clock lock");
            let now = state.as_ref().copied().expect("fake clock is available");
            *state = Ok(now.checked_add(duration).expect("fake clock advance"));
        }

        fn fail(&self) {
            *self.state.lock().expect("fake publication clock lock") =
                Err(PublicationClockError::Unavailable);
        }
    }

    impl PublicationClock for FakePublicationClock {
        fn now(&self) -> Result<PublicationInstant, PublicationClockError> {
            *self.state.lock().expect("fake publication clock lock")
        }
    }

    #[derive(Debug)]
    struct DropCounter(Arc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn instance(suffix: u8) -> ProjectInstanceId {
        format!("00000000-0000-4000-8000-{suffix:012x}")
            .parse()
            .expect("valid project instance id")
    }

    fn declaration(
        instance: ProjectInstanceId,
        revision: u64,
        host: &str,
    ) -> PublishedServiceDeclaration {
        let domain: DomainName = host.parse().expect("valid domain");
        PublishedServiceDeclaration {
            project_instance_id: instance,
            service_name: "workbench".into(),
            configuration_revision: revision,
            origin: SemanticOrigin::https(&domain, 443),
            domain_claims: BTreeSet::new(),
            health_policy: PublishedHttpHealthPolicy::new("/", 1, 5).expect("valid health policy"),
        }
    }

    fn principal(birth: u64) -> PublisherPrincipal {
        PublisherPrincipal::new(501, 42, PublisherProcessBirth::Test(birth))
    }

    fn registry(
        now: Duration,
        declaration: PublishedServiceDeclaration,
    ) -> (PublicationRegistry, FakePublicationClock, ServiceKey) {
        let clock = FakePublicationClock::new(now);
        let key = ServiceKey::new(
            declaration.project_instance_id,
            declaration.service_name.clone(),
        );
        let configuration_revision = declaration.configuration_revision;
        let mut registry = PublicationRegistry::with_epoch(
            Arc::new(clock.clone()),
            DaemonEpoch(AuthorityToken::from_byte(7)),
        );
        registry
            .reconcile_declarations(key.instance(), configuration_revision, [declaration])
            .result
            .expect("admit declaration");
        (registry, clock, key)
    }

    fn begin_attempt(
        registry: &mut PublicationRegistry,
        key: &ServiceKey,
        principal: &PublisherPrincipal,
    ) -> AcquisitionAttemptHandle {
        let BeginPreparation::Started(preparation) = registry
            .begin_preparation(key, principal.clone())
            .result
            .expect("begin preparation")
        else {
            panic!("expected fresh preparation");
        };
        registry
            .complete_preparation(&preparation)
            .result
            .expect("complete preparation")
    }

    fn capability(identity: u64, drops: &Arc<AtomicUsize>) -> RetainedListenerCapability {
        RetainedListenerCapability::new(
            ListenerIdentity::Test(identity),
            Arc::new(DropCounter(drops.clone())),
        )
    }

    fn publish(
        registry: &mut PublicationRegistry,
        key: &ServiceKey,
        principal: &PublisherPrincipal,
        identity: u64,
        drops: &Arc<AtomicUsize>,
    ) -> (AcquisitionAttemptHandle, LeaseGrant) {
        let attempt = begin_attempt(registry, key, principal);
        let origin = registry
            .projection(key)
            .expect("published projection")
            .origin;
        let BeginAcquire::Started(fence) = registry
            .begin_acquire(
                &attempt,
                principal,
                &origin,
                &ListenerIdentity::Test(identity),
            )
            .result
            .expect("begin acquisition")
        else {
            panic!("expected fresh acquisition");
        };
        let grant = registry
            .commit_acquire(&fence, capability(identity, drops))
            .result
            .expect("commit acquisition");
        (attempt, grant)
    }

    #[test]
    fn system_clock_is_suspend_inclusive_and_available_on_supported_macos() {
        let first = SystemPublicationClock.now().expect("system clock");
        let second = SystemPublicationClock.now().expect("system clock");
        assert!(second >= first);
    }

    #[test]
    fn preparation_joins_by_exact_principal_and_expires_at_the_boundary() {
        let declaration = declaration(instance(1), 1, "one.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);

        let BeginPreparation::Started(first) = registry
            .begin_preparation(&key, owner.clone())
            .result
            .expect("first preparation")
        else {
            panic!("expected started preparation");
        };
        assert_eq!(
            registry
                .begin_preparation(&key, owner.clone())
                .result
                .expect("joined preparation"),
            BeginPreparation::Joined(first.clone())
        );
        assert_eq!(
            registry
                .begin_preparation(&key, principal(2))
                .result
                .expect_err("competing principal must fail"),
            PublicationRegistryError::AcquisitionInProgress
        );

        clock.advance(PREPARATION_TTL);
        let BeginPreparation::Started(successor) = registry
            .begin_preparation(&key, owner)
            .result
            .expect("expired preparation vacates")
        else {
            panic!("expected successor preparation");
        };
        assert_eq!(successor.generation, first.generation + 1);
    }

    #[test]
    fn terminal_acquisition_replays_and_replacement_is_compare_and_swap() {
        let declaration = declaration(instance(2), 1, "two.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let attempt = begin_attempt(&mut registry, &key, &owner);
        let origin = registry.projection(&key).expect("projection").origin;
        let BeginAcquire::Started(fence) = registry
            .begin_acquire(&attempt, &owner, &origin, &ListenerIdentity::Test(1))
            .result
            .expect("begin acquisition")
        else {
            panic!("expected started acquisition");
        };
        registry
            .fail_acquire(&fence, TerminalAttemptFailure::EndpointUnhealthy)
            .result
            .expect("record terminal result");
        assert_eq!(
            registry
                .begin_acquire(&attempt, &owner, &origin, &ListenerIdentity::Test(1),)
                .result
                .expect("terminal replay"),
            BeginAcquire::Terminal(TerminalAttemptFailure::EndpointUnhealthy)
        );

        let replacement = registry
            .replace_terminal_acquisition(&attempt, &owner)
            .result
            .expect("replace terminal attempt");
        assert_ne!(replacement, attempt);
        assert_eq!(
            registry
                .replace_terminal_acquisition(&attempt, &owner)
                .result
                .expect_err("stale replacement must fail"),
            PublicationRegistryError::AttemptStale
        );
    }

    #[test]
    fn renewal_uses_commit_time_and_stale_expiry_cannot_retire_successor_deadline() {
        let declaration = declaration(instance(3), 1, "three.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, first) = publish(&mut registry, &key, &owner, 1, &drops);
        assert_eq!(first.schedule.expires_in, LEASE_TTL);

        clock.advance(Duration::from_secs(20));
        let renewed = registry
            .renew(&first.lease, &owner)
            .result
            .expect("renew lease");
        assert_eq!(renewed.schedule.expires_in, LEASE_TTL);

        clock.advance(Duration::from_secs(10));
        assert!(
            !registry
                .expire(&first.expiry_fence)
                .result
                .expect("stale expiry is harmless")
        );
        assert_eq!(
            registry.projection(&key).expect("projection").state,
            PublicationState::CheckingEndpoint
        );

        clock.advance(Duration::from_secs(20));
        assert_eq!(
            registry
                .renew(&renewed.lease, &owner)
                .result
                .expect_err("deadline is strict"),
            PublicationRegistryError::LeaseLost
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn acquisition_replay_does_not_extend_the_deadline() {
        let declaration = declaration(instance(4), 1, "four.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (attempt, grant) = publish(&mut registry, &key, &owner, 4, &drops);
        clock.advance(Duration::from_secs(25));
        let origin = registry.projection(&key).expect("projection").origin;
        let BeginAcquire::Replay(replay) = registry
            .begin_acquire(&attempt, &owner, &origin, &ListenerIdentity::Test(4))
            .result
            .expect("replay acquisition")
        else {
            panic!("expected acquisition replay");
        };
        assert_eq!(replay.schedule.expires_in, Duration::from_secs(5));
        assert_eq!(replay.schedule.renew_after, Duration::ZERO);
        assert_eq!(replay.expiry_fence, grant.expiry_fence);

        clock.advance(Duration::from_secs(5));
        let expired = registry.begin_acquire(&attempt, &owner, &origin, &ListenerIdentity::Test(4));
        assert_eq!(
            expired
                .result
                .expect_err("replay cannot preserve an expired lease"),
            PublicationRegistryError::AttemptExpired
        );
        drop(expired.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pid_reuse_and_wrong_birth_cannot_mutate_authority() {
        let declaration = declaration(instance(5), 1, "five.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let reused_pid = principal(2);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 5, &drops);

        assert_eq!(
            registry
                .release(&grant.lease, &reused_pid)
                .result
                .expect_err("reused pid lacks authority"),
            PublicationRegistryError::LeaseLost
        );
        registry
            .retire_principal(&reused_pid)
            .result
            .expect("stale reaper is harmless");
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn pause_and_wake_preserve_policy_but_never_fabricate_readiness() {
        let declaration = declaration(instance(6), 1, "six.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 6, &drops);

        registry
            .set_paused(key.instance(), true)
            .result
            .expect("pause route");
        assert_eq!(
            registry.projection(&key).expect("projection").state,
            PublicationState::RoutePaused
        );
        registry
            .renew(&grant.lease, &owner)
            .result
            .expect("passive renewal remains allowed");
        assert_eq!(
            registry.projection(&key).expect("projection").state,
            PublicationState::RoutePaused
        );

        let resumed = registry.set_paused(key.instance(), false);
        resumed.result.expect("resume route policy");
        assert!(resumed.effects.probe_required.contains(&key));
        assert_eq!(
            registry.projection(&key).expect("projection").state,
            PublicationState::CheckingEndpoint
        );
        let wake = registry.wake_barrier(true);
        wake.result.expect("trustworthy wake");
        assert!(wake.effects.probe_required.contains(&key));
    }

    #[test]
    fn wake_retires_in_flight_attempt_and_fences_its_work_against_a_successor() {
        let declaration = declaration(instance(14), 1, "fourteen.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let attempt = begin_attempt(&mut registry, &key, &owner);
        let origin = registry.projection(&key).expect("projection").origin;
        let BeginAcquire::Started(stale_fence) = registry
            .begin_acquire(&attempt, &owner, &origin, &ListenerIdentity::Test(14))
            .result
            .expect("begin candidate before wake")
        else {
            panic!("expected in-flight acquisition");
        };

        registry
            .wake_barrier(true)
            .result
            .expect("trustworthy wake barrier");
        assert_eq!(
            registry
                .begin_acquire(&attempt, &owner, &origin, &ListenerIdentity::Test(15),)
                .result
                .expect_err("pre-wake handle must be retired"),
            PublicationRegistryError::AttemptStale
        );

        let successor = begin_attempt(&mut registry, &key, &owner);
        let BeginAcquire::Started(successor_fence) = registry
            .begin_acquire(&successor, &owner, &origin, &ListenerIdentity::Test(15))
            .result
            .expect("fresh attempt begins successor work")
        else {
            panic!("expected successor acquisition");
        };
        assert_eq!(
            registry
                .commit_acquire(&stale_fence, capability(14, &drops))
                .result
                .expect_err("pre-wake fence cannot become current again"),
            PublicationRegistryError::AttemptStale
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(
            registry
                .commit_acquire(&successor_fence, capability(15, &drops))
                .result
                .is_ok()
        );
    }

    #[test]
    fn clock_regression_and_failure_retire_all_authority() {
        let declaration = declaration(instance(7), 1, "seven.localhost");
        let (mut registry, clock, key) = registry(Duration::from_secs(100), declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 7, &drops);

        clock.set(Duration::from_secs(99));
        let outcome = registry.renew(&grant.lease, &owner);
        assert_eq!(
            outcome.result.expect_err("clock regression fails closed"),
            PublicationRegistryError::ClockRegressed
        );
        drop(outcome.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry.projection(&key).expect("projection").state,
            PublicationState::WaitingForPublisher
        );

        clock.fail();
        assert_eq!(
            registry
                .begin_preparation(&key, owner)
                .result
                .expect_err("clock failure fails closed"),
            PublicationRegistryError::ClockUnavailable
        );
    }

    #[test]
    fn declaration_transfer_preserves_only_compatible_live_authority() {
        let first = declaration(instance(8), 1, "eight.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, first.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 8, &drops);

        let mut alias_only = first.clone();
        alias_only.configuration_revision = 2;
        registry
            .reconcile_declarations(key.instance(), 2, [alias_only.clone()])
            .result
            .expect("alias-only transfer");
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());

        let mut policy = alias_only;
        policy.configuration_revision = 3;
        policy.health_policy =
            PublishedHttpHealthPolicy::new("/health", 2, 5).expect("valid policy");
        let policy_effects = registry.reconcile_declarations(key.instance(), 3, [policy.clone()]);
        policy_effects.result.expect("health-policy transfer");
        assert!(policy_effects.effects.probe_required.contains(&key));
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());

        let changed_origin = declaration(instance(8), 4, "replacement.localhost");
        let outcome = registry.reconcile_declarations(key.instance(), 4, [changed_origin]);
        outcome.result.expect("origin replacement is authoritative");
        drop(outcome.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry
                .renew(&grant.lease, &owner)
                .result
                .expect_err("old lease retired"),
            PublicationRegistryError::LeaseLost
        );
    }

    #[test]
    fn stale_or_conflicting_declaration_batches_leave_current_authority_unchanged() {
        let current = declaration(instance(15), 2, "fifteen.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, current.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 15, &drops);

        let mut stale = current.clone();
        stale.configuration_revision = 1;
        assert_eq!(
            registry
                .reconcile_declarations(key.instance(), 1, [stale])
                .result
                .expect_err("stale batch must be rejected"),
            PublicationRegistryError::DeclarationConflict
        );
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        let mut conflicting_duplicate = current.clone();
        conflicting_duplicate.configuration_revision = 3;
        conflicting_duplicate.origin = SemanticOrigin::https(
            &"replacement-fifteen.localhost"
                .parse()
                .expect("replacement domain"),
            443,
        );
        let mut same_key = current;
        same_key.configuration_revision = 3;
        assert_eq!(
            registry
                .reconcile_declarations(key.instance(), 3, [same_key, conflicting_duplicate],)
                .result
                .expect_err("conflicting duplicate key must fail atomically"),
            PublicationRegistryError::DeclarationConflict
        );
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn instance_revision_fences_disjoint_empty_and_forgotten_snapshot_aba() {
        let instance = instance(16);
        let first = declaration(instance, 2, "sixteen.localhost");
        let (mut registry, _clock, first_key) = registry(Duration::ZERO, first.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_first_attempt, first_grant) = publish(&mut registry, &first_key, &owner, 16, &drops);

        let mut replacement = declaration(instance, 3, "preview-sixteen.localhost");
        replacement.service_name = "preview".into();
        let replacement_key = ServiceKey::new(instance, replacement.service_name.clone());
        let replaced = registry.reconcile_declarations(instance, 3, [replacement.clone()]);
        replaced.result.expect("admit disjoint replacement");
        drop(replaced.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry
                .renew(&first_grant.lease, &owner)
                .result
                .expect_err("disjoint replacement retires old lease"),
            PublicationRegistryError::LeaseLost
        );
        let (_replacement_attempt, replacement_grant) =
            publish(&mut registry, &replacement_key, &owner, 17, &drops);

        assert_eq!(
            registry
                .reconcile_declarations(instance, 2, [first.clone()])
                .result
                .expect_err("delayed disjoint snapshot cannot revive old service"),
            PublicationRegistryError::DeclarationConflict
        );
        assert!(
            registry
                .renew(&replacement_grant.lease, &owner)
                .result
                .is_ok()
        );

        let removed = registry.reconcile_declarations(
            instance,
            4,
            std::iter::empty::<PublishedServiceDeclaration>(),
        );
        removed.result.expect("admit empty generation");
        drop(removed.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 2);

        let mut readded = first.clone();
        readded.configuration_revision = 5;
        registry
            .reconcile_declarations(instance, 5, [readded.clone()])
            .result
            .expect("re-add service under a fresh generation");
        let (_readded_attempt, readded_grant) =
            publish(&mut registry, &first_key, &owner, 18, &drops);
        assert_eq!(
            registry
                .reconcile_declarations(
                    instance,
                    4,
                    std::iter::empty::<PublishedServiceDeclaration>(),
                )
                .result
                .expect_err("delayed empty generation cannot remove re-added service"),
            PublicationRegistryError::DeclarationConflict
        );
        assert!(registry.renew(&readded_grant.lease, &owner).result.is_ok());

        let forgotten = registry.retire_instance(instance);
        forgotten.result.expect("forget exact instance");
        drop(forgotten.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 3);
        assert_eq!(
            registry
                .reconcile_declarations(instance, 5, [readded])
                .result
                .expect_err("forgotten generation remains a tombstone"),
            PublicationRegistryError::DeclarationConflict
        );
    }

    #[test]
    fn skipped_instance_revision_applies_latest_declaration_but_retires_authority() {
        let current = declaration(instance(17), 2, "seventeen.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, current.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 17, &drops);

        let mut jumped = current;
        jumped.configuration_revision = 4;
        let outcome = registry.reconcile_declarations(key.instance(), 4, [jumped]);
        outcome.result.expect("apply latest admitted generation");
        drop(outcome.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry
                .renew(&grant.lease, &owner)
                .result
                .expect_err("unobserved generation prevents authority transfer"),
            PublicationRegistryError::LeaseLost
        );
        assert_eq!(
            registry.projection(&key).expect("latest projection").state,
            PublicationState::WaitingForPublisher
        );
    }

    #[test]
    fn missing_and_exact_instance_retirement_do_not_cross_worktrees() {
        let first = declaration(instance(9), 1, "nine.localhost");
        let second = declaration(instance(10), 1, "ten.localhost");
        let clock = FakePublicationClock::new(Duration::ZERO);
        let mut registry = PublicationRegistry::with_epoch(
            Arc::new(clock),
            DaemonEpoch(AuthorityToken::from_byte(3)),
        );
        registry
            .reconcile_declarations(first.project_instance_id, 1, [first.clone()])
            .result
            .expect("admit first worktree");
        registry
            .reconcile_declarations(second.project_instance_id, 1, [second.clone()])
            .result
            .expect("admit second worktree");
        let first_key = ServiceKey::new(first.project_instance_id, first.service_name.clone());
        let second_key = ServiceKey::new(second.project_instance_id, second.service_name.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        publish(&mut registry, &first_key, &owner, 9, &drops);
        publish(&mut registry, &second_key, &owner, 10, &drops);

        registry
            .set_missing(first_key.instance(), true)
            .result
            .expect("mark exact instance missing");
        assert_eq!(
            registry
                .projection(&first_key)
                .expect("first projection")
                .state,
            PublicationState::InstanceMissing
        );
        assert_eq!(
            registry
                .projection(&second_key)
                .expect("second projection")
                .state,
            PublicationState::CheckingEndpoint
        );
        registry
            .set_missing(first_key.instance(), false)
            .result
            .expect("reactivate exact instance");
        assert_eq!(
            registry
                .projection(&first_key)
                .expect("first projection")
                .state,
            PublicationState::WaitingForPublisher
        );
        registry
            .retire_instance(first_key.instance())
            .result
            .expect("forget exact instance");
        assert!(registry.projection(&first_key).is_none());
        assert!(registry.projection(&second_key).is_some());
    }

    #[test]
    fn rebind_is_exact_replayable_and_retires_the_old_capability() {
        let declaration = declaration(instance(11), 1, "eleven.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (acquisition, first) = publish(&mut registry, &key, &owner, 1, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&first.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let BeginRebindCandidate::Started(fence) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(2))
            .result
            .expect("begin rebind candidate")
        else {
            panic!("expected fresh candidate");
        };
        let committed = registry.commit_rebind(&fence, capability(2, &drops));
        let grant = committed.result.expect("commit rebind");
        assert_eq!(grant.binding_revision, 2);
        drop(committed.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        assert_eq!(
            registry
                .begin_acquire(&acquisition, &owner, &origin, &ListenerIdentity::Test(1),)
                .result
                .expect_err("acquisition replay is superseded"),
            PublicationRegistryError::BindingReplaced
        );
        assert!(matches!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(2))
                .result
                .expect("replay committed rebind"),
            BeginRebindCandidate::Replay(LeaseGrant {
                binding_revision: 2,
                ..
            })
        ));

        let successor = registry
            .replace_terminal_rebind(&grant.lease, &owner, &handle, 2)
            .result
            .expect("replace terminal rebind");
        assert_ne!(successor, handle);
    }

    #[test]
    fn repeated_cycles_keep_one_constant_space_slot_and_restart_loses_authority() {
        let declaration = declaration(instance(12), 1, "twelve.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let mut last_handle = None;
        for cycle in 0..100 {
            let (_attempt, grant) = publish(
                &mut registry,
                &key,
                &owner,
                u64::try_from(cycle).expect("cycle fits") + 1,
                &drops,
            );
            last_handle = Some(grant.lease.clone());
            let outcome = registry.release(&grant.lease, &owner);
            outcome.result.expect("release cycle");
            drop(outcome.effects);
            assert_eq!(registry.declared_len(), 1);
            clock.advance(Duration::from_millis(1));
        }
        assert_eq!(drops.load(Ordering::SeqCst), 100);

        let mut restarted = PublicationRegistry::with_epoch(
            Arc::new(clock),
            DaemonEpoch(AuthorityToken::from_byte(99)),
        );
        restarted
            .reconcile_declarations(key.instance(), 1, [declaration])
            .result
            .expect("restore durable declaration only");
        assert_eq!(
            restarted.projection(&key).expect("projection").state,
            PublicationState::WaitingForPublisher
        );
        assert_eq!(
            restarted
                .renew(&last_handle.expect("last lease handle"), &owner)
                .result
                .expect_err("old daemon authority is gone"),
            PublicationRegistryError::LeaseLost
        );
    }

    #[test]
    fn debug_output_redacts_every_private_authority_type() {
        let principal = principal(42);
        let epoch = DaemonEpoch(AuthorityToken::from_byte(1));
        let key = ServiceKey::new(instance(13), "workbench");
        let attempt = AcquisitionAttemptHandle {
            epoch: epoch.clone(),
            service: key.clone(),
            generation: 9,
            token: AuthorityToken::from_byte(2),
        };
        let lease = LeaseHandle {
            epoch,
            service: key,
            generation: 9,
            token: AuthorityToken::from_byte(3),
        };
        assert_eq!(format!("{principal:?}"), "PublisherPrincipal(<redacted>)");
        assert_eq!(
            format!("{attempt:?}"),
            "AcquisitionAttemptHandle(<redacted>)"
        );
        assert_eq!(format!("{lease:?}"), "LeaseHandle(<redacted>)");
        assert_eq!(
            format!("{:?}", ListenerIdentity::Test(12)),
            "ListenerIdentity(<redacted>)"
        );
    }
}
