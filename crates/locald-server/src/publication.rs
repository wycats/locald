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
#![allow(clippy::redundant_pub_crate)] // Sibling modules share this crate-internal authority API.

use locald_core::ipc::PublicationState;
use locald_core::{
    CatalogPresence, ProjectInstanceId, PublishedServiceDeclaration, Registry, SemanticOrigin,
    ServiceKey,
};
use locald_publisher_client::{
    SystemWakeMonitor, WakeError, WakeMonitor, WakeRegistration, WakeSink,
};
use locald_publisher_protocol as protocol;
use rand::{RngCore as _, rngs::OsRng};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{Mutex, mpsc, watch};

const LEASE_TTL: Duration = Duration::from_secs(30);
const RENEW_AFTER: Duration = Duration::from_secs(10);
const ACQUISITION_ATTEMPT_TTL: Duration = Duration::from_secs(15);
const REBIND_ATTEMPT_TTL: Duration = Duration::from_secs(15);
const WAIT_READY_TTL: Duration = Duration::from_secs(30);
const PREPARATION_TTL: Duration = Duration::from_mins(1);
const FRAME_DELIVERY_TTL: Duration = Duration::from_secs(5);

/// No-op half of the explicit Linux-sandbox no-host-suspend guarantee.
///
/// This private monitor observes nothing. It is reachable only after the
/// composite monitor has found system wake observation unavailable and the
/// daemon has authenticated the dedicated no-host-suspend marker.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct LinuxSandboxNoHostSuspendWakeMonitor;

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct LinuxSandboxNoHostSuspendWakeRegistration;

#[cfg(target_os = "linux")]
impl WakeRegistration for LinuxSandboxNoHostSuspendWakeRegistration {}

#[cfg(target_os = "linux")]
impl WakeMonitor for LinuxSandboxNoHostSuspendWakeMonitor {
    fn register(&self, _sink: Arc<dyn WakeSink>) -> Result<Box<dyn WakeRegistration>, WakeError> {
        Ok(Box::new(LinuxSandboxNoHostSuspendWakeRegistration))
    }
}

/// Prefer real Linux wake observation, falling back only when an explicitly
/// guaranteed sandbox has no system wake service.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct LinuxSandboxExplicitNoHostSuspendWakeMonitor<System, Fallback> {
    system: System,
    fallback: Fallback,
}

#[cfg(target_os = "linux")]
impl<System, Fallback> WakeMonitor
    for LinuxSandboxExplicitNoHostSuspendWakeMonitor<System, Fallback>
where
    System: WakeMonitor,
    Fallback: WakeMonitor,
{
    fn register(&self, sink: Arc<dyn WakeSink>) -> Result<Box<dyn WakeRegistration>, WakeError> {
        match self.system.register(Arc::clone(&sink)) {
            Ok(registration) => Ok(registration),
            Err(WakeError::Unavailable) => self.fallback.register(sink),
            Err(error @ WakeError::Failed(_)) => Err(error),
        }
    }
}

/// One daemon-lifetime, suspend-inclusive monotonic clock reading.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PublicationInstant(Duration);

#[derive(Clone, Copy)]
struct ReadinessWaitFence {
    deadline: PublicationInstant,
    pause_generation: u64,
    paused_at_capture: bool,
}

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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
struct DaemonEpoch([u8; 16]);

impl DaemonEpoch {
    fn random() -> Self {
        let mut bytes = [0_u8; 16];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    #[cfg(test)]
    const fn from_byte(byte: u8) -> Self {
        Self([byte; 16])
    }
}

impl fmt::Debug for DaemonEpoch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DaemonEpoch(<redacted>)")
    }
}

/// Kernel-observed process birth evidence. This is never persisted.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum PublisherProcessBirth {
    MacOs {
        process_id_version: i32,
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
pub(crate) struct PublisherPrincipal {
    uid: u32,
    pid: u32,
    birth: PublisherProcessBirth,
}

impl PublisherPrincipal {
    pub(crate) fn new(uid: u32, pid: u32, birth: PublisherProcessBirth) -> Self {
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
pub(crate) enum ListenerIdentity {
    MacOsIpv4 {
        address: [u8; 4],
        port: u16,
        /// Darwin `in_sockinfo.insi_gencnt`, obtained with
        /// `PROC_PIDFDSOCKETINFO` from the received descriptor.
        pcb_generation: u64,
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
pub(crate) struct RetainedListenerCapability {
    identity: ListenerIdentity,
    guard: Arc<dyn Send + Sync>,
}

/// Registry-owned cancellation for one currently executing operation.
///
/// The controller never leaves the registry. The worker receives only an
/// observer, so cancellation remains a consequence of serialized authority
/// transitions rather than a caller-controlled mutation.
struct OperationCancellationController {
    sender: watch::Sender<bool>,
}

impl OperationCancellationController {
    fn pair() -> (Self, OperationCancellationObserver) {
        let (sender, receiver) = watch::channel(false);
        (
            Self { sender },
            OperationCancellationObserver {
                identity: AuthorityToken::random(),
                receiver,
            },
        )
    }

    fn cancel(&self) {
        self.sender.send_replace(true);
    }
}

impl fmt::Debug for OperationCancellationController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperationCancellationController(<redacted>)")
    }
}

#[derive(Clone)]
struct OperationCancellationObserver {
    identity: AuthorityToken,
    receiver: watch::Receiver<bool>,
}

impl OperationCancellationObserver {
    fn is_cancelled(&self) -> bool {
        *self.receiver.borrow()
    }

    async fn cancelled(&mut self) {
        while !self.is_cancelled() {
            if self.receiver.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

impl PartialEq for OperationCancellationObserver {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Eq for OperationCancellationObserver {}

impl fmt::Debug for OperationCancellationObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperationCancellationObserver(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OperationPermit<F> {
    fence: F,
    cancellation: OperationCancellationObserver,
}

impl<F> std::ops::Deref for OperationPermit<F> {
    type Target = F;

    fn deref(&self) -> &Self::Target {
        &self.fence
    }
}

impl RetainedListenerCapability {
    pub(crate) fn new(identity: ListenerIdentity, guard: Arc<dyn Send + Sync>) -> Self {
        Self { identity, guard }
    }

    pub(crate) const fn identity(&self) -> &ListenerIdentity {
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

impl AcquisitionAttemptHandle {
    fn wire(&self) -> locald_publisher_protocol::AcquisitionAttemptHandle {
        locald_publisher_protocol::AcquisitionAttemptHandle::from_bytes(self.token.0)
    }
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

impl RebindAttemptHandle {
    fn wire(&self) -> locald_publisher_protocol::RebindAttemptHandle {
        locald_publisher_protocol::RebindAttemptHandle::from_bytes(self.token.0)
    }
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

impl LeaseHandle {
    fn wire(&self) -> locald_publisher_protocol::LeaseHandle {
        locald_publisher_protocol::LeaseHandle::from_bytes(self.token.0)
    }
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
    epoch: DaemonEpoch,
    service: ServiceKey,
    generation: u64,
    token: AuthorityToken,
    principal: PublisherPrincipal,
    configuration_revision: u64,
    deadline: PublicationInstant,
}

/// Opaque authority clock fence minted when the manager admits a Begin
/// request. Candidate reservation must carry this exact value so cold config
/// loading and host convergence consume the same 60-second preparation
/// budget instead of starting a fresh clock after validation.
#[derive(Debug, Clone)]
pub(crate) struct PublisherPreparationDeadline {
    epoch: DaemonEpoch,
    deadline: PublicationInstant,
    #[cfg(test)]
    forced_expired: Arc<AtomicBool>,
}

impl PublisherPreparationDeadline {
    fn is_forced_expired(&self) -> bool {
        #[cfg(test)]
        {
            return self.forced_expired.load(Ordering::Acquire);
        }
        #[cfg(not(test))]
        {
            let _ = self;
            false
        }
    }
}

/// Authority reserved from a fully validated catalog candidate before that
/// candidate's hosts projection or durable catalog image is installed.
///
/// Candidate preparation deliberately lives outside `PublicationSlot`: the
/// currently published declaration must retain its lease/attempt authority
/// until the journaled catalog transition reaches its commit point.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidatePreparationFence {
    epoch: DaemonEpoch,
    service: ServiceKey,
    token: AuthorityToken,
    principal: PublisherPrincipal,
    declaration: DeclarationAuthority,
    replacement: Option<locald_publisher_protocol::AcquisitionAttemptHandle>,
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
    Started(OperationPermit<PreparationFence>),
    Joined(PreparationFence),
    Terminal {
        fence: PreparationFence,
        failure: TerminalAttemptFailure,
    },
    ExistingAttempt {
        handle: AcquisitionAttemptHandle,
        state: AttemptState,
        origin: SemanticOrigin,
        expires_in: Duration,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BeginCandidatePreparation {
    Started(OperationPermit<CandidatePreparationFence>),
    Joined(CandidatePreparationFence),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BeginAcquire {
    Started(OperationPermit<AcquisitionFence>),
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
    Started(OperationPermit<RebindFence>),
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
    retired_preparations: BTreeSet<AuthorityToken>,
    timed_out_preparations: BTreeSet<AuthorityToken>,
}

impl PublicationEffects {
    fn has_changes(&self) -> bool {
        !self.projection_changed.is_empty()
            || !self.probe_required.is_empty()
            || !self.retired_capabilities.is_empty()
            || !self.retired_preparations.is_empty()
            || !self.timed_out_preparations.is_empty()
    }

    fn merge(&mut self, mut other: Self) {
        self.projection_changed
            .append(&mut other.projection_changed);
        self.probe_required.append(&mut other.probe_required);
        self.retired_capabilities
            .append(&mut other.retired_capabilities);
        self.retired_preparations
            .append(&mut other.retired_preparations);
        self.timed_out_preparations
            .append(&mut other.timed_out_preparations);
    }
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
    #[error("the readiness wait reached its deadline")]
    WaitDeadlineElapsed,
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
enum PreparationPhase {
    InFlight(OperationCancellationController),
    Terminal(TerminalAttemptFailure),
}

#[derive(Debug)]
struct Preparation {
    fence: PreparationFence,
    phase: PreparationPhase,
}

#[derive(Debug)]
struct AcquisitionRequest {
    fence: AcquisitionFence,
}

#[derive(Debug)]
enum AcquisitionPhase {
    Pending,
    InFlight {
        request: AcquisitionRequest,
        cancellation: OperationCancellationController,
    },
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
            AcquisitionPhase::InFlight { .. } => AttemptState::InFlight,
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
    InFlight {
        request: RebindRequest,
        cancellation: OperationCancellationController,
    },
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
            RebindPhase::InFlight { .. } => AttemptState::InFlight,
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
    /// Monotonic fence for exact readiness waits. Resume clears current policy
    /// but never erases the fact that a wait crossed a pause transition.
    pause_generation: u64,
    missing: bool,
    last_generation: u64,
    state: SlotState,
}

#[derive(Debug)]
struct CandidatePreparation {
    fence: CandidatePreparationFence,
    cancellation: OperationCancellationController,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstanceConfigurationState {
    Active(u64),
    Retired(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclarationAdmission {
    Ordinary,
    AdmittedReregistration,
}

/// Pure, synchronous, constant-space publication authority registry.
struct PublicationRegistry {
    clock: SharedPublicationClock,
    epoch: DaemonEpoch,
    last_now: Option<PublicationInstant>,
    configuration_states: BTreeMap<ProjectInstanceId, InstanceConfigurationState>,
    /// Current project-level route policy, retained across declaration changes.
    paused_instances: BTreeSet<ProjectInstanceId>,
    /// Current project-instance presence policy, retained across declarations.
    missing_instances: BTreeSet<ProjectInstanceId>,
    candidate_preparations: BTreeMap<ServiceKey, CandidatePreparation>,
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
            configuration_states: BTreeMap::new(),
            paused_instances: BTreeSet::new(),
            missing_instances: BTreeSet::new(),
            candidate_preparations: BTreeMap::new(),
            slots: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    fn with_epoch(clock: SharedPublicationClock, epoch: DaemonEpoch) -> Self {
        Self {
            clock,
            epoch,
            last_now: None,
            configuration_states: BTreeMap::new(),
            paused_instances: BTreeSet::new(),
            missing_instances: BTreeSet::new(),
            candidate_preparations: BTreeMap::new(),
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

    fn acquisition_handle_for_wire(
        &self,
        wire: &locald_publisher_protocol::AcquisitionAttemptHandle,
    ) -> Option<AcquisitionAttemptHandle> {
        self.slots.values().find_map(|slot| match &slot.state {
            SlotState::Attempt(attempt) if attempt.handle.wire() == *wire => {
                Some(attempt.handle.clone())
            }
            SlotState::Live(lease) if lease.acquisition_replay.handle.wire() == *wire => {
                Some(lease.acquisition_replay.handle.clone())
            }
            SlotState::Vacant
            | SlotState::Preparing(_)
            | SlotState::Attempt(_)
            | SlotState::Live(_) => None,
        })
    }

    fn rebind_handle_for_wire(
        &self,
        wire: &locald_publisher_protocol::RebindAttemptHandle,
    ) -> Option<RebindAttemptHandle> {
        self.slots.values().find_map(|slot| match &slot.state {
            SlotState::Live(lease) => lease
                .rebind
                .as_ref()
                .filter(|attempt| attempt.handle.wire() == *wire)
                .map(|attempt| attempt.handle.clone()),
            SlotState::Vacant | SlotState::Preparing(_) | SlotState::Attempt(_) => None,
        })
    }

    fn lease_handle_for_wire(
        &self,
        wire: &locald_publisher_protocol::LeaseHandle,
    ) -> Option<LeaseHandle> {
        self.slots.values().find_map(|slot| match &slot.state {
            SlotState::Live(lease) if lease.handle.wire() == *wire => Some(lease.handle.clone()),
            SlotState::Vacant
            | SlotState::Preparing(_)
            | SlotState::Attempt(_)
            | SlotState::Live(_) => None,
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
        self.reconcile_declarations_with_admission(
            instance,
            configuration_revision,
            declarations,
            DeclarationAdmission::Ordinary,
        )
    }

    /// Registry half of the outer catalog transition that atomically admits
    /// this exact retired project instance and its complete declaration set.
    fn reconcile_admitted_reregistration(
        &mut self,
        instance: ProjectInstanceId,
        configuration_revision: u64,
        declarations: impl IntoIterator<Item = PublishedServiceDeclaration>,
    ) -> PublicationOutcome<()> {
        self.reconcile_declarations_with_admission(
            instance,
            configuration_revision,
            declarations,
            DeclarationAdmission::AdmittedReregistration,
        )
    }

    fn reconcile_declarations_with_admission(
        &mut self,
        instance: ProjectInstanceId,
        configuration_revision: u64,
        declarations: impl IntoIterator<Item = PublishedServiceDeclaration>,
        admission: DeclarationAdmission,
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

        let current_state = self.configuration_states.get(&instance).copied();
        let current_revision = match (current_state, admission) {
            (Some(InstanceConfigurationState::Active(current)), DeclarationAdmission::Ordinary) => {
                Some(current)
            }
            (
                Some(InstanceConfigurationState::Retired(current)),
                DeclarationAdmission::AdmittedReregistration,
            ) if configuration_revision > current => Some(current),
            (None, DeclarationAdmission::Ordinary) => None,
            (Some(InstanceConfigurationState::Retired(_)), DeclarationAdmission::Ordinary)
            | (
                Some(InstanceConfigurationState::Active(_)) | None,
                DeclarationAdmission::AdmittedReregistration,
            ) => {
                return PublicationOutcome::err(
                    PublicationRegistryError::DeclarationConflict,
                    effects,
                );
            }
            (
                Some(InstanceConfigurationState::Retired(_)),
                DeclarationAdmission::AdmittedReregistration,
            ) => {
                return PublicationOutcome::err(
                    PublicationRegistryError::DeclarationConflict,
                    effects,
                );
            }
        };
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
            self.retain_exact_candidate_preparations(instance, &candidates, &mut effects);
            return PublicationOutcome::ok((), effects);
        }
        let may_transfer_authority = admission == DeclarationAdmission::Ordinary
            && current_revision.and_then(|current| current.checked_add(1))
                == Some(configuration_revision);

        // A Begin operation may have reserved the exact validated candidate
        // before journaled hosts/catalog convergence. Preserve only that exact
        // declaration; every older or divergent candidate loses authority.
        self.retain_exact_candidate_preparations(instance, &candidates, &mut effects);

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
                        paused: self.paused_instances.contains(&instance),
                        pause_generation: 0,
                        missing: self.missing_instances.contains(&instance),
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
                    Self::transfer_compatible_rebind_replay(
                        lease,
                        candidate.0.configuration_revision,
                        health_equivalent,
                    );
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

        self.configuration_states.insert(
            instance,
            InstanceConfigurationState::Active(configuration_revision),
        );

        PublicationOutcome::ok((), effects)
    }

    fn begin_candidate_preparation(
        &mut self,
        admission_deadline: &PublisherPreparationDeadline,
        declaration: PublishedServiceDeclaration,
        principal: PublisherPrincipal,
        replacement: Option<locald_publisher_protocol::AcquisitionAttemptHandle>,
    ) -> PublicationOutcome<BeginCandidatePreparation> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        if admission_deadline.epoch != self.epoch {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        if admission_deadline.is_forced_expired() || admission_deadline.deadline <= now {
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        let declaration = DeclarationAuthority(declaration);
        let key = declaration.key();
        if declaration.0.configuration_revision == 0 {
            return PublicationOutcome::err(PublicationRegistryError::DeclarationConflict, effects);
        }
        if let Some(current) = self.candidate_preparations.get(&key) {
            if current.fence.principal == principal
                && current.fence.declaration == declaration
                && current.fence.replacement == replacement
            {
                return PublicationOutcome::ok(
                    BeginCandidatePreparation::Joined(current.fence.clone()),
                    effects,
                );
            }
            return PublicationOutcome::err(
                PublicationRegistryError::AcquisitionInProgress,
                effects,
            );
        }
        if let Some(replacement) = &replacement {
            let Some(current) = self.acquisition_handle_for_wire(replacement) else {
                return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
            };
            if current.service != key {
                return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
            }
        }
        if let Some(slot) = self
            .slots
            .get(&key)
            .filter(|slot| slot.declaration == declaration)
        {
            match &slot.state {
                SlotState::Preparing(preparation) if preparation.fence.principal != principal => {
                    return PublicationOutcome::err(
                        PublicationRegistryError::AcquisitionInProgress,
                        effects,
                    );
                }
                SlotState::Attempt(attempt) if attempt.principal != principal => {
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
                SlotState::Vacant | SlotState::Preparing(_) | SlotState::Attempt(_) => {}
            }
        }
        let fence = CandidatePreparationFence {
            epoch: self.epoch.clone(),
            service: key.clone(),
            token: AuthorityToken::random(),
            principal,
            declaration,
            replacement,
            deadline: admission_deadline.deadline,
        };
        let (cancellation, cancellation_observer) = OperationCancellationController::pair();
        self.candidate_preparations.insert(
            key,
            CandidatePreparation {
                fence: fence.clone(),
                cancellation,
            },
        );
        PublicationOutcome::ok(
            BeginCandidatePreparation::Started(OperationPermit {
                fence,
                cancellation: cancellation_observer,
            }),
            effects,
        )
    }

    fn begin_candidate_preparation_deadline(
        &mut self,
    ) -> PublicationOutcome<PublisherPreparationDeadline> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let deadline = match now.checked_add(PREPARATION_TTL) {
            Ok(deadline) => deadline,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        PublicationOutcome::ok(
            PublisherPreparationDeadline {
                epoch: self.epoch.clone(),
                deadline,
                #[cfg(test)]
                forced_expired: Arc::new(AtomicBool::new(false)),
            },
            effects,
        )
    }

    fn candidate_preparation_deadline_remaining(
        &mut self,
        deadline: &PublisherPreparationDeadline,
    ) -> PublicationOutcome<Duration> {
        let (now, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        if deadline.epoch != self.epoch {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        if deadline.is_forced_expired() {
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        let Some(remaining) = deadline.deadline.duration_since(now) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        };
        if remaining.is_zero() {
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        PublicationOutcome::ok(remaining, effects)
    }

    fn take_candidate_preparation(
        &mut self,
        fence: &CandidatePreparationFence,
    ) -> PublicationOutcome<()> {
        let (now, mut effects) = match self.begin_transition_except(&fence.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(current) = self.candidate_preparations.get(&fence.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if current.fence.deadline <= now {
            let current = self
                .candidate_preparations
                .remove(&fence.service)
                .expect("candidate preparation exists");
            current.cancellation.cancel();
            effects.timed_out_preparations.insert(current.fence.token);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        let declaration_is_current = self
            .slots
            .get(&fence.service)
            .is_some_and(|slot| !slot.missing && slot.declaration == fence.declaration);
        let revision_is_current = matches!(
            self.configuration_states.get(&fence.service.instance()),
            Some(InstanceConfigurationState::Active(revision))
                if *revision == fence.declaration.0.configuration_revision
        );
        if current.fence != *fence || !declaration_is_current || !revision_is_current {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        self.candidate_preparations.remove(&fence.service);
        PublicationOutcome::ok((), effects)
    }

    fn fail_candidate_preparation(
        &mut self,
        fence: &CandidatePreparationFence,
    ) -> PublicationOutcome<()> {
        let (_, effects) = match self.begin_transition_except(&fence.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(current) = self.candidate_preparations.get(&fence.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if current.fence != *fence {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        self.candidate_preparations.remove(&fence.service);
        PublicationOutcome::ok((), effects)
    }

    fn retain_exact_candidate_preparations(
        &mut self,
        instance: ProjectInstanceId,
        declarations: &BTreeMap<ServiceKey, DeclarationAuthority>,
        effects: &mut PublicationEffects,
    ) {
        let retired = self
            .candidate_preparations
            .iter()
            .filter(|(key, preparation)| {
                key.instance() == instance
                    && declarations.get(*key) != Some(&preparation.fence.declaration)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in retired {
            if let Some(preparation) = self.candidate_preparations.remove(&key) {
                preparation.cancellation.cancel();
                effects.retired_preparations.insert(preparation.fence.token);
            }
        }
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
                    epoch: self.epoch.clone(),
                    service: key.clone(),
                    generation,
                    token: AuthorityToken::random(),
                    principal,
                    configuration_revision: slot.declaration.0.configuration_revision,
                    deadline,
                };
                let (cancellation, cancellation_observer) = OperationCancellationController::pair();
                slot.last_generation = generation;
                slot.state = SlotState::Preparing(Preparation {
                    fence: fence.clone(),
                    phase: PreparationPhase::InFlight(cancellation),
                });
                BeginPreparation::Started(OperationPermit {
                    fence,
                    cancellation: cancellation_observer,
                })
            }
            SlotState::Preparing(preparation) if preparation.fence.principal == principal => {
                match &preparation.phase {
                    PreparationPhase::InFlight(_) => {
                        BeginPreparation::Joined(preparation.fence.clone())
                    }
                    PreparationPhase::Terminal(failure) => BeginPreparation::Terminal {
                        fence: preparation.fence.clone(),
                        failure: *failure,
                    },
                }
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
        let (now, mut effects) = match self.begin_transition_except(&fence.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Preparing(current) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if current.fence.deadline <= now {
            Self::retire_slot_state(&fence.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        if current.fence != *fence
            || current.fence.configuration_revision != slot.declaration.0.configuration_revision
            || !matches!(current.phase, PreparationPhase::InFlight(_))
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
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

    fn fail_preparation(
        &mut self,
        fence: &PreparationFence,
        failure: TerminalAttemptFailure,
    ) -> PublicationOutcome<()> {
        let (now, mut effects) = match self.begin_transition_except(&fence.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Preparing(current) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if current.fence.deadline <= now {
            Self::retire_slot_state(&fence.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        if current.fence != *fence || !matches!(current.phase, PreparationPhase::InFlight(_)) {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        current.phase = PreparationPhase::Terminal(failure);
        PublicationOutcome::ok((), effects)
    }

    fn replace_terminal_preparation(
        &mut self,
        current: &PreparationFence,
        principal: &PublisherPrincipal,
    ) -> PublicationOutcome<OperationPermit<PreparationFence>> {
        let (now, mut effects) = match self.begin_transition_except(&current.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&current.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Preparing(preparation) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if preparation.fence.deadline <= now {
            Self::retire_slot_state(&current.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        if preparation.fence != *current
            || preparation.fence.principal != *principal
            || !matches!(preparation.phase, PreparationPhase::Terminal(_))
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        let Some(generation) = slot.last_generation.checked_add(1) else {
            return PublicationOutcome::err(PublicationRegistryError::GenerationOverflow, effects);
        };
        let deadline = match now.checked_add(PREPARATION_TTL) {
            Ok(deadline) => deadline,
            Err(error) => return PublicationOutcome::err(error, effects),
        };
        let replacement = PreparationFence {
            epoch: self.epoch.clone(),
            service: current.service.clone(),
            generation,
            token: AuthorityToken::random(),
            principal: principal.clone(),
            configuration_revision: slot.declaration.0.configuration_revision,
            deadline,
        };
        let (cancellation, cancellation_observer) = OperationCancellationController::pair();
        slot.last_generation = generation;
        slot.state = SlotState::Preparing(Preparation {
            fence: replacement.clone(),
            phase: PreparationPhase::InFlight(cancellation),
        });
        PublicationOutcome::ok(
            OperationPermit {
                fence: replacement,
                cancellation: cancellation_observer,
            },
            effects,
        )
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
        if attempt.deadline <= now {
            Self::retire_slot_state(&handle.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        if attempt.handle != *handle || attempt.principal != *principal {
            return PublicationOutcome::err(PublicationRegistryError::AttemptMismatch, effects);
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
                let (cancellation, cancellation_observer) = OperationCancellationController::pair();
                attempt.phase = AcquisitionPhase::InFlight {
                    request: AcquisitionRequest {
                        fence: fence.clone(),
                    },
                    cancellation,
                };
                BeginAcquire::Started(OperationPermit {
                    fence,
                    cancellation: cancellation_observer,
                })
            }
            AcquisitionPhase::InFlight { request, .. } if request.fence == fence => {
                BeginAcquire::Joined(request.fence.clone())
            }
            AcquisitionPhase::Terminal { request, failure } if request.fence == fence => {
                BeginAcquire::Terminal(*failure)
            }
            AcquisitionPhase::InFlight { .. } | AcquisitionPhase::Terminal { .. } => {
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
        if attempt.deadline <= now {
            Self::retire_slot_state(&fence.handle.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        if attempt.handle != fence.handle
            || attempt.principal != fence.principal
            || attempt.deadline != fence.deadline
            || attempt.configuration_revision != fence.configuration_revision
            || slot.declaration.0.configuration_revision != fence.configuration_revision
            || slot.declaration.0.origin != fence.acknowledged_origin
            || !matches!(&attempt.phase, AcquisitionPhase::InFlight { request, .. } if request.fence == *fence)
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
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
        let (now, mut effects) = match self.begin_transition_except(&fence.handle.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Attempt(attempt) = &mut slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if attempt.deadline <= now {
            Self::retire_slot_state(&fence.handle.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::AttemptExpired, effects);
        }
        if !matches!(&attempt.phase, AcquisitionPhase::InFlight { request, .. } if request.fence == *fence)
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
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

    fn begin_wait_ready(
        &mut self,
        handle: &LeaseHandle,
        principal: &PublisherPrincipal,
        expected_binding_revision: u64,
    ) -> PublicationOutcome<ReadinessWaitFence> {
        let (now, mut effects) = match self.begin_transition_except(&handle.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if matches!(&slot.state, SlotState::Live(lease) if lease.deadline <= now) {
            Self::retire_slot_state(&handle.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        let SlotState::Live(lease) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if lease.handle != *handle || lease.principal != *principal {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        if lease.binding_revision != expected_binding_revision {
            return PublicationOutcome::err(PublicationRegistryError::BindingReplaced, effects);
        }
        match now.checked_add(WAIT_READY_TTL) {
            Ok(deadline) => PublicationOutcome::ok(
                ReadinessWaitFence {
                    deadline,
                    pause_generation: slot.pause_generation,
                    paused_at_capture: slot.paused,
                },
                effects,
            ),
            Err(error) => PublicationOutcome::err(error, effects),
        }
    }

    fn wait_projection(
        &mut self,
        handle: &LeaseHandle,
        principal: &PublisherPrincipal,
        expected_binding_revision: u64,
        wait_fence: &ReadinessWaitFence,
    ) -> PublicationOutcome<(PublicationProjection, Duration)> {
        let (now, mut effects) = match self.begin_transition_except(&handle.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&handle.service) else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if matches!(&slot.state, SlotState::Live(lease) if lease.deadline <= now) {
            Self::retire_slot_state(&handle.service, slot, &mut effects);
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        let SlotState::Live(lease) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        };
        if lease.handle != *handle || lease.principal != *principal {
            return PublicationOutcome::err(PublicationRegistryError::LeaseLost, effects);
        }
        if lease.binding_revision != expected_binding_revision {
            return PublicationOutcome::err(PublicationRegistryError::BindingReplaced, effects);
        }
        let pause_generation_changed = slot.pause_generation != wait_fence.pause_generation;
        let pause_observed =
            wait_fence.paused_at_capture || slot.paused || pause_generation_changed;
        let wait_remaining = if pause_observed {
            Duration::ZERO
        } else {
            let Some(wait_remaining) = wait_fence.deadline.duration_since(now) else {
                return PublicationOutcome::err(
                    PublicationRegistryError::WaitDeadlineElapsed,
                    effects,
                );
            };
            if wait_remaining.is_zero() {
                return PublicationOutcome::err(
                    PublicationRegistryError::WaitDeadlineElapsed,
                    effects,
                );
            }
            wait_remaining
        };
        let projection = PublicationProjection {
            state: if slot.missing {
                PublicationState::InstanceMissing
            } else if pause_observed {
                PublicationState::RoutePaused
            } else {
                // b.3.5 deliberately installs no route authorization. b.3.6
                // replaces this with exact health- and route-scoped state.
                PublicationState::CheckingEndpoint
            },
            origin: slot.declaration.0.origin.clone(),
        };
        PublicationOutcome::ok((projection, wait_remaining), effects)
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
        if lease.handle != fence.lease {
            return PublicationOutcome::ok(false, effects);
        }
        if lease.deadline <= now {
            Self::retire_slot_state(&fence.lease.service, slot, &mut effects);
            return PublicationOutcome::ok(true, effects);
        }
        let matches_renewal = lease.renewal_revision == fence.renewal_revision;
        let matches_deadline = lease.deadline == fence.deadline;
        if !(matches_renewal && matches_deadline) {
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
                let (cancellation, cancellation_observer) = OperationCancellationController::pair();
                attempt.phase = RebindPhase::InFlight {
                    request: RebindRequest {
                        fence: fence.clone(),
                    },
                    cancellation,
                };
                BeginRebindCandidate::Started(OperationPermit {
                    fence,
                    cancellation: cancellation_observer,
                })
            }
            RebindPhase::InFlight { request, .. } if request.fence == fence => {
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
            RebindPhase::InFlight { .. }
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
                        RebindPhase::InFlight { request, .. } if request.fence == *fence
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
        if !matches!(&attempt.phase, RebindPhase::InFlight { request, .. } if request.fence == *fence)
        {
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
        if paused
            && self.slots.iter().any(|(key, slot)| {
                key.instance() == instance && !slot.paused && slot.pause_generation == u64::MAX
            })
        {
            return PublicationOutcome::err(PublicationRegistryError::GenerationOverflow, effects);
        }
        if paused {
            self.paused_instances.insert(instance);
        } else {
            self.paused_instances.remove(&instance);
        }
        for (key, slot) in &mut self.slots {
            if key.instance() != instance || slot.paused == paused {
                continue;
            }
            if paused {
                slot.pause_generation = slot
                    .pause_generation
                    .checked_add(1)
                    .expect("pause generation was preflighted");
            }
            slot.paused = paused;
            if let SlotState::Live(lease) = &mut slot.state {
                Self::retire_nonreplayable_rebind(lease);
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
        if missing {
            self.missing_instances.insert(instance);
            let candidate_keys = self
                .candidate_preparations
                .keys()
                .filter(|key| key.instance() == instance)
                .cloned()
                .collect::<Vec<_>>();
            for key in candidate_keys {
                if let Some(preparation) = self.candidate_preparations.remove(&key) {
                    preparation.cancellation.cancel();
                    effects.retired_preparations.insert(preparation.fence.token);
                }
            }
        } else {
            self.missing_instances.remove(&instance);
        }
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
        self.paused_instances.remove(&instance);
        self.missing_instances.remove(&instance);
        let high_water_revision = match self.configuration_states.get(&instance).copied() {
            Some(
                InstanceConfigurationState::Active(revision)
                | InstanceConfigurationState::Retired(revision),
            ) => revision,
            None => 0,
        };
        self.configuration_states.insert(
            instance,
            InstanceConfigurationState::Retired(high_water_revision),
        );
        let candidate_keys = self
            .candidate_preparations
            .keys()
            .filter(|key| key.instance() == instance)
            .cloned()
            .collect::<Vec<_>>();
        for key in candidate_keys {
            if let Some(preparation) = self.candidate_preparations.remove(&key) {
                preparation.cancellation.cancel();
                effects.retired_preparations.insert(preparation.fence.token);
            }
        }
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
        let candidate_keys = self
            .candidate_preparations
            .iter()
            .filter(|(_, preparation)| preparation.fence.principal == *principal)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in candidate_keys {
            if let Some(preparation) = self.candidate_preparations.remove(&key) {
                preparation.cancellation.cancel();
                effects.retired_preparations.insert(preparation.fence.token);
            }
        }
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
        let candidate_keys = self
            .candidate_preparations
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for key in candidate_keys {
            if let Some(preparation) = self.candidate_preparations.remove(&key) {
                preparation.cancellation.cancel();
                effects.retired_preparations.insert(preparation.fence.token);
            }
        }
        for (key, slot) in &mut self.slots {
            let preparation_in_flight = matches!(
                &slot.state,
                SlotState::Preparing(Preparation {
                    phase: PreparationPhase::InFlight(_),
                    ..
                })
            );
            let acquisition_in_flight = matches!(
                &slot.state,
                SlotState::Attempt(attempt)
                    if matches!(attempt.phase, AcquisitionPhase::InFlight { .. })
            );
            if preparation_in_flight || acquisition_in_flight {
                Self::retire_slot_state(key, slot, &mut effects);
                continue;
            }
            match &mut slot.state {
                SlotState::Live(lease) => {
                    Self::retire_nonreplayable_rebind(lease);
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

    fn preparation_tokens(&self) -> BTreeSet<AuthorityToken> {
        self.slots
            .values()
            .filter_map(|slot| match &slot.state {
                SlotState::Preparing(preparation) => Some(preparation.fence.token.clone()),
                SlotState::Vacant | SlotState::Attempt(_) | SlotState::Live(_) => None,
            })
            .chain(
                self.candidate_preparations
                    .values()
                    .map(|preparation| preparation.fence.token.clone()),
            )
            .collect()
    }

    fn next_deadline(&mut self) -> PublicationOutcome<Option<Duration>> {
        let now = match self.observe_now() {
            Ok(now) => now,
            Err(failure) => {
                let (error, effects) = *failure;
                return PublicationOutcome::err(error, effects);
            }
        };
        let deadline = self
            .slots
            .values()
            .filter_map(|slot| match &slot.state {
                SlotState::Vacant => None,
                SlotState::Preparing(preparation) => Some(preparation.fence.deadline),
                SlotState::Attempt(attempt) => Some(attempt.deadline),
                SlotState::Live(lease) => {
                    Some(lease.rebind.as_ref().map_or(lease.deadline, |attempt| {
                        lease.deadline.min(attempt.deadline)
                    }))
                }
            })
            .chain(
                self.candidate_preparations
                    .values()
                    .map(|preparation| preparation.fence.deadline),
            )
            .min();
        PublicationOutcome::ok(
            deadline.map(|deadline| deadline.duration_since(now).unwrap_or_default()),
            PublicationEffects::default(),
        )
    }

    fn sweep_deadlines(&mut self) -> PublicationOutcome<()> {
        let (_, effects) = match self.begin_transition() {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        PublicationOutcome::ok((), effects)
    }

    #[cfg(test)]
    fn timeout_candidate_preparation(&mut self, key: &ServiceKey) -> PublicationOutcome<bool> {
        let (_, mut effects) = match self.begin_transition_except(key) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(preparation) = self.candidate_preparations.remove(key) else {
            return PublicationOutcome::ok(false, effects);
        };
        preparation.cancellation.cancel();
        effects
            .timed_out_preparations
            .insert(preparation.fence.token);
        PublicationOutcome::ok(true, effects)
    }

    fn vacate_terminal_preparation(&mut self, fence: &PreparationFence) -> PublicationOutcome<()> {
        let (_, effects) = match self.begin_transition_except(&fence.service) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        let Some(slot) = self.slots.get_mut(&fence.service) else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        let SlotState::Preparing(preparation) = &slot.state else {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        };
        if preparation.fence != *fence
            || !matches!(preparation.phase, PreparationPhase::Terminal(_))
        {
            return PublicationOutcome::err(PublicationRegistryError::AttemptStale, effects);
        }
        slot.state = SlotState::Vacant;
        PublicationOutcome::ok((), effects)
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
            Err(failure) => {
                let (error, effects) = *failure;
                Err(PublicationOutcome::err(error, effects))
            }
        }
    }

    fn observe_now(
        &mut self,
    ) -> Result<PublicationInstant, Box<(PublicationRegistryError, PublicationEffects)>> {
        let now = match self.clock.now() {
            Ok(now) => now,
            Err(PublicationClockError::Unavailable) => {
                let effects = self.retire_all_authority();
                self.last_now = None;
                return Err(Box::new((
                    PublicationRegistryError::ClockUnavailable,
                    effects,
                )));
            }
        };
        if self.last_now.is_some_and(|last| now < last) {
            let effects = self.retire_all_authority();
            self.last_now = Some(now);
            return Err(Box::new((
                PublicationRegistryError::ClockRegressed,
                effects,
            )));
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
        let expired_candidates = self
            .candidate_preparations
            .iter()
            .filter(|(key, preparation)| {
                excluded != Some(*key) && preparation.fence.deadline <= now
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired_candidates {
            if let Some(preparation) = self.candidate_preparations.remove(&key) {
                preparation.cancellation.cancel();
                effects
                    .timed_out_preparations
                    .insert(preparation.fence.token);
            }
        }
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
                let timed_out_preparation = match &slot.state {
                    SlotState::Preparing(preparation) => Some(preparation.fence.token.clone()),
                    SlotState::Vacant | SlotState::Attempt(_) | SlotState::Live(_) => None,
                };
                Self::retire_slot_state(key, slot, &mut effects);
                if let Some(token) = timed_out_preparation {
                    effects.retired_preparations.remove(&token);
                    effects.timed_out_preparations.insert(token);
                }
            } else if let SlotState::Live(lease) = &mut slot.state {
                if lease
                    .rebind
                    .as_ref()
                    .is_some_and(|attempt| attempt.deadline <= now)
                {
                    Self::clear_rebind(lease);
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
        match previous {
            SlotState::Preparing(preparation) => {
                effects
                    .retired_preparations
                    .insert(preparation.fence.token.clone());
                Self::cancel_preparation_if_in_flight(&preparation);
            }
            SlotState::Attempt(attempt) => {
                Self::cancel_acquisition_if_in_flight(&attempt);
            }
            SlotState::Live(lease) => {
                let mut lease = *lease;
                Self::clear_rebind(&mut lease);
                effects.retired_capabilities.push(lease.capability);
                effects.projection_changed.insert(key.clone());
            }
            SlotState::Vacant => {}
        }
    }

    fn cancel_preparation_if_in_flight(preparation: &Preparation) {
        if let PreparationPhase::InFlight(cancellation) = &preparation.phase {
            cancellation.cancel();
        }
    }

    fn cancel_acquisition_if_in_flight(attempt: &AcquisitionAttempt) {
        if let AcquisitionPhase::InFlight { cancellation, .. } = &attempt.phase {
            cancellation.cancel();
        }
    }

    fn cancel_rebind_if_in_flight(attempt: &RebindAttempt) {
        if let RebindPhase::InFlight { cancellation, .. } = &attempt.phase {
            cancellation.cancel();
        }
    }

    fn clear_rebind(lease: &mut LiveLease) {
        if let Some(attempt) = lease.rebind.take() {
            Self::cancel_rebind_if_in_flight(&attempt);
        }
    }

    fn retire_nonreplayable_rebind(lease: &mut LiveLease) {
        let terminal_replay_is_current =
            lease
                .rebind
                .as_ref()
                .is_some_and(|attempt| match &attempt.phase {
                    RebindPhase::TerminalFailure { .. } => true,
                    RebindPhase::TerminalSuccess {
                        installed_binding_revision,
                        ..
                    } => *installed_binding_revision == lease.binding_revision,
                    RebindPhase::Pending | RebindPhase::InFlight { .. } => false,
                });
        if !terminal_replay_is_current {
            Self::clear_rebind(lease);
        }
    }

    fn transfer_compatible_rebind_replay(
        lease: &mut LiveLease,
        configuration_revision: u64,
        health_equivalent: bool,
    ) {
        let Some(mut attempt) = lease.rebind.take() else {
            return;
        };
        let targets_current_binding = attempt.expected_binding_revision == lease.binding_revision;
        let preserve = match &mut attempt.phase {
            RebindPhase::TerminalSuccess {
                request,
                installed_binding_revision,
            } if *installed_binding_revision == lease.binding_revision => {
                request.fence.configuration_revision = configuration_revision;
                true
            }
            RebindPhase::TerminalFailure { request, .. }
                if health_equivalent && targets_current_binding =>
            {
                request.fence.configuration_revision = configuration_revision;
                true
            }
            RebindPhase::Pending
            | RebindPhase::InFlight { .. }
            | RebindPhase::TerminalFailure { .. }
            | RebindPhase::TerminalSuccess { .. } => false,
        };
        if preserve {
            lease.rebind = Some(attempt);
        } else {
            Self::cancel_rebind_if_in_flight(&attempt);
        }
    }

    fn retire_all_authority(&mut self) -> PublicationEffects {
        let mut effects = PublicationEffects::default();
        let candidate_keys = self
            .candidate_preparations
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for key in candidate_keys {
            if let Some(preparation) = self.candidate_preparations.remove(&key) {
                preparation.cancellation.cancel();
                effects.retired_preparations.insert(preparation.fence.token);
            }
        }
        for (key, slot) in &mut self.slots {
            Self::retire_slot_state(key, slot, &mut effects);
        }
        effects
    }
}

/// Daemon-owned publication authority exposed by the dedicated publisher
/// transport. Durable declarations remain in the catalog; this value owns only
/// daemon-lifetime handles, leases, listener guards, and change observation.
#[derive(Clone)]
pub(crate) struct PublisherAuthority {
    inner: Arc<PublisherAuthorityInner>,
}

#[derive(Debug)]
enum PublisherWakeObservation {
    Inactive,
    Registering { suspending: bool, resumed: bool },
    Active(Box<dyn WakeRegistration>),
    Sleeping(Box<dyn WakeRegistration>),
    BarrierPending(Box<dyn WakeRegistration>),
    Failed(WakeError),
}

#[derive(Debug, Clone, Copy)]
enum PublisherWakeSignal {
    Resumed,
    Failed,
}

#[cfg(test)]
#[derive(Clone)]
struct WaitReadyCaptureHook {
    reached: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}

struct PublisherAuthorityInner {
    registry: Arc<Mutex<PublicationRegistry>>,
    changes: watch::Sender<u64>,
    preparation_waiters: std::sync::Mutex<HashMap<AuthorityToken, PreparationSender>>,
    wake_activation: std::sync::Mutex<()>,
    wake_barrier_gate: std::sync::RwLock<()>,
    wake_observation: std::sync::Mutex<PublisherWakeObservation>,
    wake_signals: mpsc::UnboundedSender<PublisherWakeSignal>,
    wake_receiver: std::sync::Mutex<Option<mpsc::UnboundedReceiver<PublisherWakeSignal>>>,
    deadline_driver_started: AtomicBool,
    shutdown: AtomicBool,
    #[cfg(test)]
    wait_ready_capture_hook: std::sync::Mutex<Option<WaitReadyCaptureHook>>,
}

#[derive(Debug)]
struct PublisherAuthorityWakeSink {
    inner: Weak<PublisherAuthorityInner>,
}

impl PublisherAuthorityWakeSink {
    fn signal(&self, signal: PublisherWakeSignal) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        if inner.wake_signals.send(signal).is_err() {
            let _barrier = inner
                .wake_barrier_gate
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut observation = inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *observation = PublisherWakeObservation::Failed(WakeError::Failed(
                "server wake barrier is unavailable".to_owned(),
            ));
        }
        inner.changes.send_modify(|revision| {
            *revision = revision.wrapping_add(1);
        });
    }
}

impl WakeSink for PublisherAuthorityWakeSink {
    fn suspending(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let changed = {
            // Publisher transitions take this gate for their complete registry
            // mutation. Waiting for the write side here fences existing work
            // and prevents successors before the platform acknowledges sleep.
            let _barrier = inner
                .wake_barrier_gate
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut observation = inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::mem::replace(&mut *observation, PublisherWakeObservation::Inactive);
            match previous {
                PublisherWakeObservation::Active(registration) => {
                    *observation = PublisherWakeObservation::Sleeping(registration);
                    true
                }
                PublisherWakeObservation::Registering { resumed, .. } => {
                    *observation = PublisherWakeObservation::Registering {
                        suspending: true,
                        resumed,
                    };
                    true
                }
                PublisherWakeObservation::Sleeping(registration) => {
                    *observation = PublisherWakeObservation::Sleeping(registration);
                    false
                }
                PublisherWakeObservation::BarrierPending(registration) => {
                    *observation = PublisherWakeObservation::BarrierPending(registration);
                    false
                }
                PublisherWakeObservation::Inactive => {
                    *observation = PublisherWakeObservation::Inactive;
                    false
                }
                PublisherWakeObservation::Failed(error) => {
                    *observation = PublisherWakeObservation::Failed(error);
                    false
                }
            }
        };
        if changed {
            inner.changes.send_modify(|revision| {
                *revision = revision.wrapping_add(1);
            });
        }
    }

    fn resumed(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let signal = {
            let _barrier = inner
                .wake_barrier_gate
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut observation = inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::mem::replace(&mut *observation, PublisherWakeObservation::Inactive);
            match previous {
                PublisherWakeObservation::Active(registration) => {
                    *observation = PublisherWakeObservation::BarrierPending(registration);
                    true
                }
                PublisherWakeObservation::Sleeping(registration) => {
                    *observation = PublisherWakeObservation::BarrierPending(registration);
                    true
                }
                PublisherWakeObservation::BarrierPending(registration) => {
                    *observation = PublisherWakeObservation::BarrierPending(registration);
                    false
                }
                PublisherWakeObservation::Registering { suspending, .. } => {
                    *observation = PublisherWakeObservation::Registering {
                        suspending,
                        resumed: true,
                    };
                    false
                }
                PublisherWakeObservation::Inactive => {
                    *observation = PublisherWakeObservation::Inactive;
                    false
                }
                PublisherWakeObservation::Failed(error) => {
                    *observation = PublisherWakeObservation::Failed(error);
                    false
                }
            }
        };
        if signal {
            drop(inner);
            self.signal(PublisherWakeSignal::Resumed);
        }
    }

    fn failed(&self, error: WakeError) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        {
            let _barrier = inner
                .wake_barrier_gate
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut observation = inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *observation = PublisherWakeObservation::Failed(error);
        }
        drop(inner);
        self.signal(PublisherWakeSignal::Failed);
    }
}

pub(crate) type PreparationCompletion =
    Result<protocol::BeginAcquisitionResult, protocol::ProtocolError>;
type PreparationSender = watch::Sender<Option<PreparationCompletion>>;

/// Outcome of reserving one acquisition preparation slot.
#[derive(Debug)]
pub(crate) enum PublisherAcquisitionPreparation {
    /// The caller owns host convergence for this exact reservation.
    Required(PublisherPreparationPermit),
    /// Preparation already completed for this principal and attempt.
    Ready(protocol::BeginAcquisitionResult),
}

/// Outcome of reserving authority for an exact validated catalog candidate.
#[derive(Debug)]
pub(crate) enum PublisherCandidateAcquisitionPreparation {
    /// The caller owns the candidate's journaled hosts/catalog convergence.
    Required(Box<PublisherCandidatePreparationPermit>),
    /// Another exact request already owns convergence; observe its result.
    Joined(PublisherPreparationWaiter),
}

#[derive(Debug, Clone)]
pub(crate) struct PublisherPreparationWaiter {
    receiver: watch::Receiver<Option<PreparationCompletion>>,
}

/// Single-use candidate authority fenced by declaration, principal, token,
/// replacement request, daemon epoch, and suspend-inclusive deadline.
#[derive(Debug)]
pub(crate) struct PublisherCandidatePreparationPermit {
    permit: OperationPermit<CandidatePreparationFence>,
    waiter: PublisherPreparationWaiter,
}

impl PublisherCandidatePreparationPermit {
    #[must_use]
    pub(crate) fn completion_waiter(&self) -> PublisherPreparationWaiter {
        self.waiter.clone()
    }

    /// Whether this exact candidate generation was retired by expiry,
    /// catalog reconciliation, wake fencing, or shutdown.
    #[must_use]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.permit.cancellation.is_cancelled()
    }

    /// Wait until this exact candidate generation is no longer authorized to
    /// continue cold convergence.
    pub(crate) async fn cancelled(&mut self) {
        self.permit.cancellation.cancelled().await;
    }
}

/// Single-use authority to complete or fail one acquisition preparation.
#[derive(Debug)]
pub(crate) struct PublisherPreparationPermit {
    permit: OperationPermit<PreparationFence>,
}

impl PublisherPreparationPermit {
    /// Whether catalog reconciliation, expiry, wake, or shutdown canceled this
    /// preparation while the manager was converging hosts.
    #[must_use]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.permit.cancellation.is_cancelled()
    }

    /// Wait until this exact preparation is canceled.
    pub(crate) async fn cancelled(&mut self) {
        self.permit.cancellation.cancelled().await;
    }
}

impl fmt::Debug for PublisherAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublisherAuthority")
            .field("registry", &"<redacted authority registry>")
            .finish_non_exhaustive()
    }
}

impl PublisherAuthority {
    pub(crate) fn new(catalog: &Registry) -> Result<Self, protocol::ProtocolError> {
        let mut registry = PublicationRegistry::new(Arc::new(SystemPublicationClock));
        Self::registry_outcome(Self::reconcile_catalog_locked(&mut registry, catalog))?;
        let (changes, _) = watch::channel(0_u64);
        let (wake_signals, wake_receiver) = mpsc::unbounded_channel();
        Ok(Self {
            inner: Arc::new(PublisherAuthorityInner {
                registry: Arc::new(Mutex::new(registry)),
                changes,
                preparation_waiters: std::sync::Mutex::new(HashMap::new()),
                wake_activation: std::sync::Mutex::new(()),
                wake_barrier_gate: std::sync::RwLock::new(()),
                wake_observation: std::sync::Mutex::new(PublisherWakeObservation::Inactive),
                wake_signals,
                wake_receiver: std::sync::Mutex::new(Some(wake_receiver)),
                deadline_driver_started: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
                #[cfg(test)]
                wait_ready_capture_hook: std::sync::Mutex::new(None),
            }),
        })
    }

    /// Establish production wake observation before publisher discovery is
    /// advertised. A registration failure leaves ordinary locald service and
    /// status behavior available, but the publisher socket must remain
    /// undiscoverable for this daemon lifetime.
    pub(crate) fn activate_system_wake_monitor(&self) -> Result<(), protocol::ProtocolError> {
        self.activate_wake_monitor(&SystemWakeMonitor)
    }

    /// Activate the explicit Linux-sandbox no-host-suspend guarantee.
    ///
    /// The caller must require both parsed sandbox mode and the dedicated
    /// no-host-suspend marker. Real system wake observation is preferred; the
    /// no-op fallback is used only when the system monitor is unavailable.
    #[cfg(target_os = "linux")]
    pub(super) fn activate_linux_sandbox_explicit_no_suspend_wake_monitor(
        &self,
    ) -> Result<(), protocol::ProtocolError> {
        self.activate_wake_monitor(&LinuxSandboxExplicitNoHostSuspendWakeMonitor {
            system: SystemWakeMonitor,
            fallback: LinuxSandboxNoHostSuspendWakeMonitor,
        })
    }

    fn activate_wake_monitor(
        &self,
        monitor: &dyn WakeMonitor,
    ) -> Result<(), protocol::ProtocolError> {
        self.ensure_active()?;
        let _activation = self
            .inner
            .wake_activation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        {
            let mut observation = self
                .inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &*observation {
                PublisherWakeObservation::Active(_) => return Ok(()),
                PublisherWakeObservation::Registering { .. }
                | PublisherWakeObservation::Sleeping(_)
                | PublisherWakeObservation::BarrierPending(_) => {
                    return Err(Self::wake_barrier_pending());
                }
                PublisherWakeObservation::Failed(error) => {
                    return Err(Self::wake_unavailable(error));
                }
                PublisherWakeObservation::Inactive => {}
            }
            *observation = PublisherWakeObservation::Registering {
                suspending: false,
                resumed: false,
            };
        }
        let sink = Arc::new(PublisherAuthorityWakeSink {
            inner: Arc::downgrade(&self.inner),
        });
        let registration = match monitor.register(Arc::clone(&sink) as Arc<dyn WakeSink>) {
            Ok(registration) => registration,
            Err(error) => {
                let mut observation = self
                    .inner
                    .wake_observation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if matches!(&*observation, PublisherWakeObservation::Registering { .. }) {
                    *observation = PublisherWakeObservation::Inactive;
                }
                return Err(match &*observation {
                    PublisherWakeObservation::Failed(observed) => Self::wake_unavailable(observed),
                    PublisherWakeObservation::Inactive
                    | PublisherWakeObservation::Registering { .. }
                    | PublisherWakeObservation::Active(_)
                    | PublisherWakeObservation::Sleeping(_)
                    | PublisherWakeObservation::BarrierPending(_) => Self::wake_unavailable(&error),
                });
            }
        };
        let mut signal_resume = false;
        let mut registration = Some(registration);
        let activation_result = {
            let mut observation = self
                .inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &*observation {
                PublisherWakeObservation::Registering {
                    suspending,
                    resumed,
                } => {
                    signal_resume = *resumed;
                    let registration = registration.take().expect("wake registration present");
                    *observation = if signal_resume {
                        PublisherWakeObservation::BarrierPending(registration)
                    } else if *suspending {
                        PublisherWakeObservation::Sleeping(registration)
                    } else {
                        PublisherWakeObservation::Active(registration)
                    };
                    Ok(())
                }
                PublisherWakeObservation::Failed(error) => Err(Self::wake_unavailable(error)),
                PublisherWakeObservation::Inactive
                | PublisherWakeObservation::Active(_)
                | PublisherWakeObservation::Sleeping(_)
                | PublisherWakeObservation::BarrierPending(_) => Err(Self::internal(
                    "publisher wake registration changed state unexpectedly",
                )),
            }
        };
        drop(registration);
        activation_result?;
        // Release any registry transition that observed the synchronous
        // `Registering` fence while the platform monitor was being installed.
        // A resume edge publishes its own pending-barrier notification.
        self.notify_change();
        self.ensure_deadline_driver();
        if signal_resume {
            sink.signal(PublisherWakeSignal::Resumed);
        }
        Ok(())
    }

    pub(crate) fn subscribe_changes(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    pub(crate) async fn protocol_info(
        &self,
        publisher_socket: protocol::AbsolutePath,
    ) -> protocol::PublishedEndpointProtocolInfo {
        let epoch = self.inner.registry.lock().await.epoch();
        protocol::PublishedEndpointProtocolInfo::v1(
            protocol::DaemonEpoch::from_bytes(epoch.0),
            publisher_socket,
        )
    }

    pub(crate) async fn epoch(&self) -> protocol::DaemonEpoch {
        let epoch = self.inner.registry.lock().await.epoch();
        protocol::DaemonEpoch::from_bytes(epoch.0)
    }

    pub(crate) async fn shutdown(&self) {
        if self.inner.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut registry = self.inner.registry.lock().await;
        let wake_barrier = self
            .inner
            .wake_barrier_gate
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let registration = {
            let mut observation = self
                .inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *observation, PublisherWakeObservation::Inactive)
        };
        let effects = registry.shutdown();
        drop(effects);
        drop(wake_barrier);
        drop(registry);
        drop(registration);
        self.finish_all_preparations(&Err(Self::inactive_error()));
        self.notify_change();
    }

    pub(crate) async fn reconcile_catalog(
        &self,
        catalog: &Registry,
    ) -> Result<(), protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut changes = self.subscribe_changes();
        loop {
            let mut registry = self.inner.registry.lock().await;
            self.ensure_active()?;
            let Some(wake_barrier) = self.try_enter_registry_transition() else {
                drop(registry);
                changes
                    .changed()
                    .await
                    .map_err(|_| Self::inactive_error())?;
                continue;
            };
            let before = registry.preparation_tokens();
            let result = self.outcome_with_preparation_error(
                Self::reconcile_catalog_locked(&mut registry, catalog),
                &Self::operation_canceled(),
            );
            let after = registry.preparation_tokens();
            drop(registry);
            drop(wake_barrier);
            self.finish_removed_preparations(&before, &after, &Self::operation_canceled());
            self.notify_change();
            return result;
        }
    }

    /// Publish one project's durable availability pause policy into the
    /// daemon-lifetime endpoint authority.
    ///
    /// The manager calls this only while it owns the lifecycle/publication
    /// serialization boundary. Keeping the policy in the authority as well as
    /// the durable availability store makes acquisition, renewal, and
    /// readiness observation agree with ordinary locald lifecycle state.
    pub(crate) async fn set_project_paused(
        &self,
        instance: ProjectInstanceId,
        paused: bool,
    ) -> Result<(), protocol::ProtocolError> {
        let mut changes = self.subscribe_changes();
        loop {
            let mut registry = self.inner.registry.lock().await;
            self.ensure_active()?;
            let Some(wake_barrier) = self.try_enter_registry_transition() else {
                drop(registry);
                changes
                    .changed()
                    .await
                    .map_err(|_| Self::inactive_error())?;
                continue;
            };
            let result = self.outcome(registry.set_paused(instance, paused));
            drop(registry);
            drop(wake_barrier);
            return result;
        }
    }

    fn reconcile_catalog_locked(
        registry: &mut PublicationRegistry,
        catalog: &Registry,
    ) -> PublicationOutcome<()> {
        let mut effects = PublicationEffects::default();
        let active_instances = catalog.instances.keys().copied().collect::<BTreeSet<_>>();
        let known_instances = registry
            .configuration_states
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for instance in known_instances {
            if !active_instances.contains(&instance) {
                if let Err(error) = Self::accumulate_registry_outcome(
                    &mut effects,
                    registry.retire_instance(instance),
                ) {
                    return PublicationOutcome::err(error, effects);
                }
            }
        }

        for (instance, record) in &catalog.instances {
            if record.configuration_revision == 0 {
                continue;
            }
            let declarations = catalog
                .published_declarations_for_instance(*instance)
                .into_iter()
                .flat_map(|declarations| declarations.values().cloned())
                .collect::<Vec<_>>();
            let outcome = if matches!(
                registry.configuration_states.get(instance),
                Some(InstanceConfigurationState::Retired(_))
            ) {
                registry.reconcile_admitted_reregistration(
                    *instance,
                    record.configuration_revision,
                    declarations,
                )
            } else {
                registry.reconcile_declarations(
                    *instance,
                    record.configuration_revision,
                    declarations,
                )
            };
            if let Err(error) = Self::accumulate_registry_outcome(&mut effects, outcome) {
                return PublicationOutcome::err(error, effects);
            }
            if let Err(error) = Self::accumulate_registry_outcome(
                &mut effects,
                registry.set_missing(*instance, record.presence == CatalogPresence::Missing),
            ) {
                return PublicationOutcome::err(error, effects);
            }
        }

        for (instance, revision) in catalog.retired_configuration_revisions() {
            if registry.configuration_states.contains_key(&instance) || revision == 0 {
                continue;
            }
            if let Err(error) = Self::accumulate_registry_outcome(
                &mut effects,
                registry.reconcile_declarations(instance, revision, []),
            ) {
                return PublicationOutcome::err(error, effects);
            }
            if let Err(error) =
                Self::accumulate_registry_outcome(&mut effects, registry.retire_instance(instance))
            {
                return PublicationOutcome::err(error, effects);
            }
        }
        PublicationOutcome::ok((), effects)
    }

    /// Reserve an exact, fully validated catalog candidate before its
    /// journaled hosts/catalog transition begins. The current published slot
    /// remains untouched until `reconcile_catalog` commits that candidate.
    pub(crate) async fn begin_candidate_preparation_deadline(
        &self,
    ) -> Result<PublisherPreparationDeadline, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let wake_barrier = self.enter_publisher_transition()?;
        let deadline = self.outcome(registry.begin_candidate_preparation_deadline())?;
        drop(registry);
        drop(wake_barrier);
        self.notify_change();
        Ok(deadline)
    }

    /// Wait for the exact manager-admission deadline to expire or for daemon
    /// authority to become unusable. The deadline is intentionally not a
    /// replayable registry slot: the manager's bounded coordinator owns the
    /// provisional service name until declaration validation succeeds.
    pub(crate) async fn wait_for_candidate_preparation_deadline(
        &self,
        deadline: PublisherPreparationDeadline,
    ) -> protocol::ProtocolError {
        self.ensure_deadline_driver();
        let mut changes = self.subscribe_changes();
        loop {
            let remaining = loop {
                let mut registry = self.inner.registry.lock().await;
                if let Err(error) = self.ensure_active() {
                    return error;
                }
                let Some(wake_barrier) = self.try_enter_registry_transition() else {
                    drop(registry);
                    if changes.changed().await.is_err() {
                        return Self::inactive_error();
                    }
                    continue;
                };
                if let Err(error) = self.ensure_wake_trustworthy() {
                    drop(registry);
                    drop(wake_barrier);
                    return error;
                }
                let result =
                    self.outcome(registry.candidate_preparation_deadline_remaining(&deadline));
                drop(registry);
                drop(wake_barrier);
                break match result {
                    Ok(remaining) => remaining,
                    Err(error) if error.code() == protocol::StableErrorCode::AttemptExpired => {
                        return Self::preparation_timed_out();
                    }
                    Err(error) if error.code() == protocol::StableErrorCode::AttemptStale => {
                        return Self::operation_canceled();
                    }
                    Err(error) => return error,
                };
            };
            tokio::select! {
                () = tokio::time::sleep(remaining) => {}
                change = changes.changed() => {
                    if change.is_err() {
                        return Self::inactive_error();
                    }
                }
            }
        }
    }

    pub(crate) async fn reserve_candidate_acquisition(
        &self,
        admission_deadline: &PublisherPreparationDeadline,
        declaration: PublishedServiceDeclaration,
        principal: PublisherPrincipal,
        replacement: Option<&protocol::AcquisitionAttemptHandle>,
    ) -> Result<PublisherCandidateAcquisitionPreparation, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let wake_barrier = self.enter_publisher_transition()?;
        let preparation = match self.outcome(registry.begin_candidate_preparation(
            admission_deadline,
            declaration,
            principal,
            replacement.cloned(),
        )) {
            Ok(preparation) => preparation,
            Err(error) if error.code() == protocol::StableErrorCode::AttemptExpired => {
                return Err(Self::preparation_timed_out());
            }
            Err(error) => return Err(error),
        };
        let result = match preparation {
            BeginCandidatePreparation::Started(permit) => {
                let (sender, receiver) = watch::channel(None);
                let previous = self
                    .inner
                    .preparation_waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(permit.fence.token.clone(), sender);
                if previous.is_some() {
                    return Err(Self::internal(
                        "publisher candidate preparation authority unexpectedly collided",
                    ));
                }
                PublisherCandidateAcquisitionPreparation::Required(Box::new(
                    PublisherCandidatePreparationPermit {
                        permit,
                        waiter: PublisherPreparationWaiter { receiver },
                    },
                ))
            }
            BeginCandidatePreparation::Joined(fence) => {
                let receiver = self
                    .inner
                    .preparation_waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&fence.token)
                    .map(watch::Sender::subscribe)
                    .ok_or_else(|| {
                        Self::internal(
                            "joined publisher candidate preparation has no completion channel",
                        )
                    })?;
                PublisherCandidateAcquisitionPreparation::Joined(PublisherPreparationWaiter {
                    receiver,
                })
            }
        };
        drop(registry);
        drop(wake_barrier);
        self.notify_change();
        Ok(result)
    }

    pub(crate) async fn complete_candidate_acquisition_preparation(
        &self,
        permit: PublisherCandidatePreparationPermit,
    ) -> Result<protocol::BeginAcquisitionResult, protocol::ProtocolError> {
        let token = permit.permit.fence.token.clone();
        let fence = permit.permit.fence;
        let mut registry = self.inner.registry.lock().await;
        if let Err(error) = self.ensure_active() {
            drop(registry);
            self.finish_preparation(&token, Err(error.clone()));
            return Err(error);
        }
        let wake_barrier = match self.enter_publisher_transition() {
            Ok(barrier) => barrier,
            Err(error) => {
                drop(registry);
                self.finish_preparation(&token, Err(error.clone()));
                return Err(error);
            }
        };
        if let Err(error) = self.outcome(registry.take_candidate_preparation(&fence)) {
            let error = if error.code() == protocol::StableErrorCode::AttemptExpired {
                Self::preparation_timed_out()
            } else {
                error
            };
            drop(registry);
            drop(wake_barrier);
            self.finish_preparation(&token, Err(error.clone()));
            return Err(error);
        }

        if let Some(replacement) = &fence.replacement {
            let current = match registry.acquisition_handle_for_wire(replacement) {
                Some(current) if current.service == fence.service => current,
                Some(_) | None => {
                    let error = Self::error(&PublicationRegistryError::AttemptStale);
                    drop(registry);
                    drop(wake_barrier);
                    self.finish_preparation(&token, Err(error.clone()));
                    return Err(error);
                }
            };
            if let Err(error) =
                self.outcome(registry.replace_terminal_acquisition(&current, &fence.principal))
            {
                drop(registry);
                drop(wake_barrier);
                self.finish_preparation(&token, Err(error.clone()));
                return Err(error);
            }
        }

        let begun = match self
            .outcome(registry.begin_preparation(&fence.service, fence.principal.clone()))
        {
            Ok(begun) => begun,
            Err(error) => {
                drop(registry);
                drop(wake_barrier);
                self.finish_preparation(&token, Err(error.clone()));
                return Err(error);
            }
        };
        let result = match begun {
            BeginPreparation::Started(preparation) => {
                let handle = match self.outcome(registry.complete_preparation(&preparation.fence)) {
                    Ok(handle) => handle,
                    Err(error) => {
                        drop(registry);
                        drop(wake_barrier);
                        self.finish_preparation(&token, Err(error.clone()));
                        return Err(error);
                    }
                };
                Self::origin_for(&registry, &handle.service).and_then(|origin| {
                    Self::begin_acquisition_result(
                        &handle,
                        AttemptState::Pending,
                        &origin,
                        ACQUISITION_ATTEMPT_TTL,
                    )
                })
            }
            BeginPreparation::ExistingAttempt {
                handle,
                state,
                origin,
                expires_in,
            } => Self::begin_acquisition_result(&handle, state, &origin, expires_in),
            BeginPreparation::Joined(joined) => {
                let receiver = self
                    .inner
                    .preparation_waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&joined.token)
                    .map(watch::Sender::subscribe)
                    .ok_or_else(|| {
                        Self::internal("joined publisher preparation has no completion channel")
                    });
                drop(registry);
                drop(wake_barrier);
                let result = match receiver {
                    Ok(receiver) => self.wait_for_preparation(receiver).await,
                    Err(error) => Err(error),
                };
                self.finish_preparation(&token, result.clone());
                return result;
            }
            BeginPreparation::Terminal { failure, .. } => Err(Self::terminal_error(failure)),
        };
        drop(registry);
        drop(wake_barrier);
        self.finish_preparation(&token, result.clone());
        self.notify_change();
        result
    }

    pub(crate) async fn fail_candidate_acquisition_preparation(
        &self,
        permit: PublisherCandidatePreparationPermit,
        error: protocol::ProtocolError,
    ) -> Result<(), protocol::ProtocolError> {
        let token = permit.permit.fence.token.clone();
        let mut registry = self.inner.registry.lock().await;
        let result = match self
            .ensure_active()
            .and_then(|()| self.enter_publisher_transition())
        {
            Ok(_wake_barrier) => {
                self.outcome(registry.fail_candidate_preparation(&permit.permit.fence))
            }
            Err(registry_error) => Err(registry_error),
        };
        drop(registry);
        let completion = match &result {
            Ok(()) => error,
            Err(registry_error) => registry_error.clone(),
        };
        self.finish_preparation(&token, Err(completion));
        self.notify_change();
        result
    }

    pub(crate) async fn wait_for_candidate_preparation(
        &self,
        waiter: PublisherPreparationWaiter,
    ) -> PreparationCompletion {
        self.wait_for_preparation(waiter.receiver).await
    }

    pub(crate) async fn reserve_acquisition(
        &self,
        key: ServiceKey,
        principal: PublisherPrincipal,
        replacement: Option<&protocol::AcquisitionAttemptHandle>,
    ) -> Result<PublisherAcquisitionPreparation, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let wake_barrier = self.enter_publisher_transition()?;
        let before = registry.preparation_tokens();
        if let Some(replacement) = replacement {
            let current = registry
                .acquisition_handle_for_wire(replacement)
                .ok_or_else(|| Self::error(&PublicationRegistryError::AttemptStale))?;
            if current.service != key {
                return Err(Self::error(&PublicationRegistryError::AttemptStale));
            }
            self.outcome(registry.replace_terminal_acquisition(&current, &principal))?;
        }

        let preparation = self.outcome(registry.begin_preparation(&key, principal.clone()))?;
        let result = match preparation {
            BeginPreparation::Started(permit) => {
                let (sender, _) = watch::channel(None);
                let previous = self
                    .inner
                    .preparation_waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(permit.fence.token.clone(), sender);
                if previous.is_some() {
                    return Err(Self::internal(
                        "publisher preparation authority unexpectedly collided",
                    ));
                }
                PublisherAcquisitionPreparation::Required(PublisherPreparationPermit { permit })
            }
            BeginPreparation::Joined(fence) => {
                let receiver = self
                    .inner
                    .preparation_waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&fence.token)
                    .map(watch::Sender::subscribe)
                    .ok_or_else(|| {
                        Self::internal("joined publisher preparation has no completion channel")
                    })?;
                let after = registry.preparation_tokens();
                drop(registry);
                drop(wake_barrier);
                self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
                return self
                    .wait_for_preparation(receiver)
                    .await
                    .map(PublisherAcquisitionPreparation::Ready);
            }
            BeginPreparation::ExistingAttempt {
                handle,
                state,
                origin,
                expires_in,
            } => PublisherAcquisitionPreparation::Ready(Self::begin_acquisition_result(
                &handle, state, &origin, expires_in,
            )?),
            BeginPreparation::Terminal { failure, .. } => {
                return Err(Self::terminal_error(failure));
            }
        };
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        self.notify_change();
        Ok(result)
    }

    pub(crate) async fn complete_acquisition_preparation(
        &self,
        permit: PublisherPreparationPermit,
    ) -> Result<protocol::BeginAcquisitionResult, protocol::ProtocolError> {
        let token = permit.permit.fence.token.clone();
        let mut registry = self.inner.registry.lock().await;
        if let Err(error) = self.ensure_active() {
            drop(registry);
            self.finish_preparation(&token, Err(error.clone()));
            return Err(error);
        }
        let _wake_barrier = match self.enter_publisher_transition() {
            Ok(barrier) => barrier,
            Err(error) => {
                drop(registry);
                self.finish_preparation(&token, Err(error.clone()));
                return Err(error);
            }
        };
        let handle = match self.outcome(registry.complete_preparation(&permit.permit.fence)) {
            Ok(handle) => handle,
            Err(error) => {
                drop(registry);
                self.finish_preparation(&token, Err(error.clone()));
                return Err(error);
            }
        };
        let result = Self::begin_acquisition_result(
            &handle,
            AttemptState::Pending,
            &Self::origin_for(&registry, &handle.service)?,
            ACQUISITION_ATTEMPT_TTL,
        );
        drop(registry);
        match result {
            Ok(result) => {
                self.finish_preparation(&token, Ok(result.clone()));
                self.notify_change();
                Ok(result)
            }
            Err(error) => {
                self.finish_preparation(&token, Err(error.clone()));
                self.notify_change();
                Err(error)
            }
        }
    }

    pub(crate) async fn fail_acquisition_preparation(
        &self,
        permit: PublisherPreparationPermit,
        error: protocol::ProtocolError,
    ) -> Result<(), protocol::ProtocolError> {
        let token = permit.permit.fence.token.clone();
        let mut registry = self.inner.registry.lock().await;
        if let Err(registry_error) = self.ensure_active() {
            drop(registry);
            self.finish_preparation(&token, Err(registry_error.clone()));
            return Err(registry_error);
        }
        let _wake_barrier = match self.enter_publisher_transition() {
            Ok(barrier) => barrier,
            Err(registry_error) => {
                drop(registry);
                self.finish_preparation(&token, Err(registry_error.clone()));
                return Err(registry_error);
            }
        };
        let failure = if error.code() == protocol::StableErrorCode::OperationCanceled {
            TerminalAttemptFailure::OperationCanceled
        } else {
            TerminalAttemptFailure::Internal
        };
        let failed = self.outcome(registry.fail_preparation(&permit.permit.fence, failure));
        let result = match failed {
            Ok(()) => self.outcome(registry.vacate_terminal_preparation(&permit.permit.fence)),
            Err(registry_error) => Err(registry_error),
        };
        drop(registry);
        let completion = match &result {
            Ok(()) => error,
            Err(registry_error) => registry_error.clone(),
        };
        self.finish_preparation(&token, Err(completion));
        self.notify_change();
        result
    }

    pub(crate) async fn acquire(
        &self,
        handle: &protocol::AcquisitionAttemptHandle,
        principal: &PublisherPrincipal,
        acknowledged_origin: &protocol::SemanticOrigin,
        capability: RetainedListenerCapability,
    ) -> Result<protocol::AcquireResult, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_publisher_transition()?;
        let before = registry.preparation_tokens();
        let handle = registry
            .acquisition_handle_for_wire(handle)
            .ok_or_else(|| Self::error(&PublicationRegistryError::AttemptStale))?;
        let origin = SemanticOrigin::parse(acknowledged_origin.as_str())
            .map_err(|error| Self::internal(error.to_string()))?;
        let listener = capability.identity().clone();
        let grant =
            match self.outcome(registry.begin_acquire(&handle, principal, &origin, &listener))? {
                BeginAcquire::Started(permit) => {
                    self.outcome(registry.commit_acquire(&permit.fence, capability))?
                }
                BeginAcquire::Replay(grant) => grant,
                BeginAcquire::Terminal(failure) => return Err(Self::terminal_error(failure)),
                BeginAcquire::Joined(_) => {
                    return Err(Self::error(
                        &PublicationRegistryError::AcquisitionInProgress,
                    ));
                }
            };
        let state = Self::state_for(&registry, &grant.lease.service)?;
        let result = protocol::AcquireResult::new(
            grant.lease.wire(),
            protocol::BindingRevision::new(grant.binding_revision)
                .map_err(|error| Self::internal(error.to_string()))?,
            Self::wire_origin(&grant.origin)?,
            Self::duration_ms(grant.schedule.renew_after)?,
            Self::duration_ms(grant.schedule.expires_in)?,
            Self::wire_publication_state(state),
        )
        .map_err(|error| Self::internal(error.to_string()))?;
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        self.notify_change();
        Ok(result)
    }

    pub(crate) async fn renew(
        &self,
        handle: &protocol::LeaseHandle,
        principal: &PublisherPrincipal,
    ) -> Result<protocol::RenewResult, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_publisher_transition()?;
        let before = registry.preparation_tokens();
        let handle = registry
            .lease_handle_for_wire(handle)
            .ok_or_else(|| Self::error(&PublicationRegistryError::LeaseLost))?;
        let grant = self.outcome(registry.renew(&handle, principal))?;
        let state = Self::state_for(&registry, &grant.lease.service)?;
        let result = protocol::RenewResult::new(
            protocol::BindingRevision::new(grant.binding_revision)
                .map_err(|error| Self::internal(error.to_string()))?,
            Self::duration_ms(grant.schedule.renew_after)?,
            Self::duration_ms(grant.schedule.expires_in)?,
            Self::wire_publication_state(state),
        )
        .map_err(|error| Self::internal(error.to_string()))?;
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        self.notify_change();
        Ok(result)
    }

    pub(crate) async fn begin_rebind(
        &self,
        lease: &protocol::LeaseHandle,
        principal: &PublisherPrincipal,
        expected_binding_revision: protocol::BindingRevision,
        replacement: Option<&protocol::RebindAttemptHandle>,
    ) -> Result<protocol::BeginRebindResult, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_publisher_transition()?;
        let before = registry.preparation_tokens();
        let lease = registry
            .lease_handle_for_wire(lease)
            .ok_or_else(|| Self::error(&PublicationRegistryError::LeaseLost))?;
        if let Some(replacement) = replacement {
            let current = registry
                .rebind_handle_for_wire(replacement)
                .ok_or_else(|| Self::error(&PublicationRegistryError::AttemptStale))?;
            self.outcome(registry.replace_terminal_rebind(
                &lease,
                principal,
                &current,
                expected_binding_revision.get(),
            ))?;
        }
        let begin = self.outcome(registry.begin_rebind(
            &lease,
            principal,
            expected_binding_revision.get(),
        ))?;
        let (handle, state, origin, expires_in) = match begin {
            BeginRebind::Started {
                handle,
                origin,
                expires_in,
            } => (handle, AttemptState::Pending, origin, expires_in),
            BeginRebind::Existing {
                handle,
                state,
                origin,
                expires_in,
            } => (handle, state, origin, expires_in),
        };
        let result = protocol::BeginRebindResult::new(
            handle.wire(),
            Self::wire_origin(&origin)?,
            Self::duration_ms(expires_in)?,
            Self::wire_attempt_state(state),
        )
        .map_err(|error| Self::internal(error.to_string()))?;
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        self.notify_change();
        Ok(result)
    }

    pub(crate) async fn rebind(
        &self,
        handle: &protocol::RebindAttemptHandle,
        principal: &PublisherPrincipal,
        acknowledged_origin: &protocol::SemanticOrigin,
        capability: RetainedListenerCapability,
    ) -> Result<protocol::RebindResult, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_publisher_transition()?;
        let before = registry.preparation_tokens();
        let handle = registry
            .rebind_handle_for_wire(handle)
            .ok_or_else(|| Self::error(&PublicationRegistryError::AttemptStale))?;
        let origin = SemanticOrigin::parse(acknowledged_origin.as_str())
            .map_err(|error| Self::internal(error.to_string()))?;
        let listener = capability.identity().clone();
        let grant = match self
            .outcome(registry.begin_rebind_candidate(&handle, principal, &origin, &listener))?
        {
            BeginRebindCandidate::Started(permit) => {
                self.outcome(registry.commit_rebind(&permit.fence, capability))?
            }
            BeginRebindCandidate::Replay(grant) => grant,
            BeginRebindCandidate::Terminal(failure) => return Err(Self::terminal_error(failure)),
            BeginRebindCandidate::Joined(_) => {
                return Err(Self::error(&PublicationRegistryError::RebindInProgress));
            }
        };
        let state = Self::state_for(&registry, &grant.lease.service)?;
        let result = protocol::RebindResult::new(
            grant.lease.wire(),
            protocol::BindingRevision::new(grant.binding_revision)
                .map_err(|error| Self::internal(error.to_string()))?,
            Self::wire_origin(&grant.origin)?,
            Self::duration_ms(grant.schedule.renew_after)?,
            Self::duration_ms(grant.schedule.expires_in)?,
            Self::wire_publication_state(state),
        )
        .map_err(|error| Self::internal(error.to_string()))?;
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        self.notify_change();
        Ok(result)
    }

    pub(crate) async fn wait_ready(
        &self,
        handle: &protocol::LeaseHandle,
        principal: &PublisherPrincipal,
        expected_binding_revision: protocol::BindingRevision,
    ) -> Result<protocol::WaitReadyResult, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let (handle, wait_fence) = {
            let mut registry = self.inner.registry.lock().await;
            self.ensure_active()?;
            let _wake_barrier = self.enter_publisher_transition()?;
            let handle = registry
                .lease_handle_for_wire(handle)
                .ok_or_else(|| Self::error(&PublicationRegistryError::LeaseLost))?;
            let wait_fence = self.outcome(registry.begin_wait_ready(
                &handle,
                principal,
                expected_binding_revision.get(),
            ))?;
            (handle, wait_fence)
        };
        #[cfg(test)]
        self.wait_at_wait_ready_capture_hook().await;
        let mut changes = self.subscribe_changes();
        loop {
            let (projection, wait_remaining) = {
                let mut registry = self.inner.registry.lock().await;
                self.ensure_active()?;
                let _wake_barrier = self.enter_publisher_transition()?;
                self.outcome(registry.wait_projection(
                    &handle,
                    principal,
                    expected_binding_revision.get(),
                    &wait_fence,
                ))?
            };
            match projection.state {
                PublicationState::Ready => {
                    return Ok(protocol::WaitReadyResult {
                        binding_revision: expected_binding_revision,
                        origin: Self::wire_origin(&projection.origin)?,
                        publication_state: protocol::ReadyState::Ready,
                    });
                }
                PublicationState::RoutePaused => {
                    return Err(protocol::ProtocolError::new(
                        protocol::StableErrorCode::ProjectPaused,
                        "the project route is paused; resume the project before waiting again",
                        Some("run `locald up` to resume this project".to_owned()),
                    ));
                }
                PublicationState::WaitingForPublisher | PublicationState::InstanceMissing => {
                    return Err(Self::error(&PublicationRegistryError::LeaseLost));
                }
                PublicationState::CheckingEndpoint | PublicationState::EndpointUnhealthy => {}
            }
            match tokio::time::timeout(wait_remaining, changes.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(Self::operation_canceled()),
                Err(_) => {
                    return Err(protocol::ProtocolError::new(
                        protocol::StableErrorCode::WaitTimedOut,
                        "the exact binding did not become routable before the readiness deadline",
                        None,
                    ));
                }
            }
        }
    }

    #[cfg(test)]
    fn set_wait_ready_capture_hook(&self, hook: WaitReadyCaptureHook) {
        *self
            .inner
            .wait_ready_capture_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    #[cfg(test)]
    async fn wait_at_wait_ready_capture_hook(&self) {
        let hook = self
            .inner
            .wait_ready_capture_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(hook) = hook {
            hook.reached.notify_one();
            hook.resume.notified().await;
        }
    }

    pub(crate) async fn release(
        &self,
        handle: &protocol::LeaseHandle,
        principal: &PublisherPrincipal,
    ) -> Result<protocol::ReleaseResult, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_publisher_transition()?;
        let before = registry.preparation_tokens();
        let handle = registry
            .lease_handle_for_wire(handle)
            .ok_or_else(|| Self::error(&PublicationRegistryError::LeaseLost))?;
        self.outcome(registry.release(&handle, principal))?;
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        self.notify_change();
        Ok(protocol::ReleaseResult::released())
    }

    pub(crate) async fn projection(
        &self,
        key: &ServiceKey,
    ) -> Result<Option<(PublicationState, SemanticOrigin)>, protocol::ProtocolError> {
        self.ensure_deadline_driver();
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_registry_transition()?;
        let before = registry.preparation_tokens();
        let projection = self.outcome(registry.snapshot(key))?;
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        Ok(projection.map(|projection| (projection.state, projection.origin)))
    }

    pub(crate) async fn sweep_deadlines(&self) -> Result<(), protocol::ProtocolError> {
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_registry_transition()?;
        let before = registry.preparation_tokens();
        let result = self.outcome_with_preparation_error(
            registry.sweep_deadlines(),
            &Self::preparation_timed_out(),
        );
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::preparation_timed_out());
        self.notify_change();
        result
    }

    #[cfg(test)]
    pub(crate) async fn timeout_candidate_preparation_for_test(
        &self,
        key: &ServiceKey,
    ) -> Result<bool, protocol::ProtocolError> {
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_registry_transition()?;
        let timed_out = self.outcome(registry.timeout_candidate_preparation(key))?;
        drop(registry);
        self.notify_change();
        Ok(timed_out)
    }

    #[cfg(test)]
    pub(crate) fn timeout_candidate_preparation_deadline_for_test(
        &self,
        deadline: &PublisherPreparationDeadline,
    ) {
        deadline.forced_expired.store(true, Ordering::Release);
        self.notify_change();
    }

    fn notify_change(&self) {
        self.inner.changes.send_modify(|revision| {
            *revision = revision.wrapping_add(1);
        });
    }

    fn ensure_active(&self) -> Result<(), protocol::ProtocolError> {
        if self.inner.shutdown.load(Ordering::Acquire) {
            Err(Self::inactive_error())
        } else {
            Ok(())
        }
    }

    fn ensure_wake_trustworthy(&self) -> Result<(), protocol::ProtocolError> {
        let observation = self
            .inner
            .wake_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*observation {
            PublisherWakeObservation::Failed(error) => Err(Self::wake_unavailable(error)),
            PublisherWakeObservation::Registering { .. }
            | PublisherWakeObservation::Sleeping(_)
            | PublisherWakeObservation::BarrierPending(_) => Err(Self::wake_barrier_pending()),
            PublisherWakeObservation::Inactive | PublisherWakeObservation::Active(_) => Ok(()),
        }
    }

    fn enter_publisher_transition(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, ()>, protocol::ProtocolError> {
        let barrier = self
            .inner
            .wake_barrier_gate
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_wake_trustworthy()?;
        Ok(barrier)
    }

    fn enter_registry_transition(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, ()>, protocol::ProtocolError> {
        self.try_enter_registry_transition()
            .ok_or_else(Self::wake_barrier_pending)
    }

    fn try_enter_registry_transition(&self) -> Option<std::sync::RwLockReadGuard<'_, ()>> {
        let barrier = self
            .inner
            .wake_barrier_gate
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let observation = self
            .inner
            .wake_observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(
            &*observation,
            PublisherWakeObservation::Registering { .. }
                | PublisherWakeObservation::Sleeping(_)
                | PublisherWakeObservation::BarrierPending(_)
        ) {
            return None;
        }
        drop(observation);
        Some(barrier)
    }

    fn ensure_deadline_driver(&self) {
        if self
            .inner
            .deadline_driver_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let Some(wake_signals) = self
            .inner
            .wake_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            self.inner
                .deadline_driver_started
                .store(false, Ordering::Release);
            return;
        };
        let weak = Arc::downgrade(&self.inner);
        let changes = self.subscribe_changes();
        tokio::spawn(Self::deadline_driver(weak, changes, wake_signals));
    }

    async fn deadline_driver(
        weak: Weak<PublisherAuthorityInner>,
        mut changes: watch::Receiver<u64>,
        mut wake_signals: mpsc::UnboundedReceiver<PublisherWakeSignal>,
    ) {
        loop {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.shutdown.load(Ordering::Acquire) {
                return;
            }
            let authority = Self { inner };
            let deadline = authority.next_deadline().await;
            drop(authority);
            match deadline {
                Ok(None) => {
                    tokio::select! {
                        signal = wake_signals.recv() => {
                            if !Self::handle_wake_signal(&weak, signal).await {
                                return;
                            }
                        }
                        change_notification = changes.changed() => {
                            if change_notification.is_err() {
                                return;
                            }
                        }
                    }
                }
                Ok(Some(duration)) => {
                    tokio::select! {
                        () = tokio::time::sleep(duration) => {
                            let Some(inner) = weak.upgrade() else {
                                return;
                            };
                            let authority = Self { inner };
                            let _ = authority.sweep_deadlines().await;
                        }
                        change_notification = changes.changed() => {
                            if change_notification.is_err() {
                                return;
                            }
                        }
                        signal = wake_signals.recv() => {
                            if !Self::handle_wake_signal(&weak, signal).await {
                                return;
                            }
                        }
                    }
                }
                Err(_) => {
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                        change_notification = changes.changed() => {
                            if change_notification.is_err() {
                                return;
                            }
                        }
                        signal = wake_signals.recv() => {
                            if !Self::handle_wake_signal(&weak, signal).await {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn handle_wake_signal(
        weak: &Weak<PublisherAuthorityInner>,
        signal: Option<PublisherWakeSignal>,
    ) -> bool {
        let Some(signal) = signal else {
            return false;
        };
        let Some(inner) = weak.upgrade() else {
            return false;
        };
        let authority = Self { inner };
        let trustworthy = matches!(signal, PublisherWakeSignal::Resumed);
        let result = authority.apply_wake_barrier(trustworthy).await;
        !matches!(signal, PublisherWakeSignal::Failed) && result.is_ok()
    }

    async fn apply_wake_barrier(&self, trustworthy: bool) -> Result<(), protocol::ProtocolError> {
        let mut registry = self.inner.registry.lock().await;
        let wake_barrier = self
            .inner
            .wake_barrier_gate
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_active()?;
        let effective_trustworthy = trustworthy
            && matches!(
                &*self
                    .inner
                    .wake_observation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                PublisherWakeObservation::BarrierPending(_)
            );
        let before = registry.preparation_tokens();
        let result = self.outcome_with_preparation_error(
            registry.wake_barrier(effective_trustworthy),
            &Self::operation_canceled(),
        );
        let after = registry.preparation_tokens();
        let mut retired_registration = None;
        if effective_trustworthy {
            let mut observation = self
                .inner
                .wake_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::mem::replace(&mut *observation, PublisherWakeObservation::Inactive);
            match (previous, result.is_ok()) {
                (PublisherWakeObservation::BarrierPending(registration), true) => {
                    *observation = PublisherWakeObservation::Active(registration);
                }
                (PublisherWakeObservation::BarrierPending(registration), false) => {
                    retired_registration = Some(registration);
                    *observation = PublisherWakeObservation::Failed(WakeError::Failed(
                        "the suspend-inclusive server clock failed during resume".to_owned(),
                    ));
                }
                (other, _) => *observation = other,
            }
        }
        drop(registry);
        drop(wake_barrier);
        drop(retired_registration);
        self.finish_removed_preparations(&before, &after, &Self::operation_canceled());
        // The synchronous wake callback already nudged observers so an exact
        // wait can enforce its suspend-clock deadline even if it wins the
        // registry race. This second edge publishes the completed barrier.
        self.notify_change();
        result
    }

    async fn next_deadline(&self) -> Result<Option<Duration>, protocol::ProtocolError> {
        let mut registry = self.inner.registry.lock().await;
        self.ensure_active()?;
        let _wake_barrier = self.enter_registry_transition()?;
        let before = registry.preparation_tokens();
        let result = self.outcome(registry.next_deadline());
        let after = registry.preparation_tokens();
        drop(registry);
        self.finish_removed_preparations(&before, &after, &Self::operation_canceled());
        result
    }

    async fn wait_for_preparation(
        &self,
        mut receiver: watch::Receiver<Option<PreparationCompletion>>,
    ) -> PreparationCompletion {
        loop {
            let completion = receiver.borrow().clone();
            if let Some(result) = completion {
                return result;
            }
            if receiver.changed().await.is_err() {
                return Err(Self::operation_canceled());
            }
        }
    }

    fn finish_preparation(&self, token: &AuthorityToken, completion: PreparationCompletion) {
        let sender = self
            .inner
            .preparation_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(token);
        if let Some(sender) = sender {
            sender.send_replace(Some(completion));
        }
    }

    fn finish_removed_preparations(
        &self,
        before: &BTreeSet<AuthorityToken>,
        after: &BTreeSet<AuthorityToken>,
        error: &protocol::ProtocolError,
    ) {
        for token in before.difference(after) {
            self.finish_preparation(token, Err(error.clone()));
        }
    }

    fn finish_all_preparations(&self, completion: &PreparationCompletion) {
        let senders = self
            .inner
            .preparation_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, sender)| sender)
            .collect::<Vec<_>>();
        for sender in senders {
            sender.send_replace(Some(completion.clone()));
        }
    }

    fn begin_acquisition_result(
        handle: &AcquisitionAttemptHandle,
        state: AttemptState,
        origin: &SemanticOrigin,
        expires_in: Duration,
    ) -> Result<protocol::BeginAcquisitionResult, protocol::ProtocolError> {
        protocol::BeginAcquisitionResult::new(
            handle.wire(),
            protocol::ProjectInstanceId::parse(&handle.service.instance().to_string())
                .map_err(|error| Self::internal(error.to_string()))?,
            Self::wire_origin(origin)?,
            Self::duration_ms(expires_in)?,
            Self::wire_attempt_state(state),
        )
        .map_err(|error| Self::internal(error.to_string()))
    }

    fn origin_for(
        registry: &PublicationRegistry,
        key: &ServiceKey,
    ) -> Result<SemanticOrigin, protocol::ProtocolError> {
        registry
            .projection(key)
            .map(|projection| projection.origin)
            .ok_or_else(|| Self::error(&PublicationRegistryError::ServiceNotDeclared))
    }

    fn state_for(
        registry: &PublicationRegistry,
        key: &ServiceKey,
    ) -> Result<PublicationState, protocol::ProtocolError> {
        registry
            .projection(key)
            .map(|projection| projection.state)
            .ok_or_else(|| Self::error(&PublicationRegistryError::LeaseLost))
    }

    fn duration_ms(duration: Duration) -> Result<u64, protocol::ProtocolError> {
        let milliseconds = duration.as_nanos().div_ceil(1_000_000);
        u64::try_from(milliseconds)
            .map_err(|_| Self::internal("publication duration exceeds the wire range"))
    }

    fn wire_origin(
        origin: &SemanticOrigin,
    ) -> Result<protocol::SemanticOrigin, protocol::ProtocolError> {
        protocol::SemanticOrigin::parse(origin.as_str())
            .map_err(|error| Self::internal(error.to_string()))
    }

    const fn wire_attempt_state(state: AttemptState) -> protocol::AttemptState {
        match state {
            AttemptState::Pending => protocol::AttemptState::Pending,
            AttemptState::InFlight => protocol::AttemptState::InFlight,
            AttemptState::Terminal => protocol::AttemptState::Terminal,
        }
    }

    const fn wire_publication_state(state: PublicationState) -> protocol::PublicationState {
        match state {
            PublicationState::WaitingForPublisher => {
                protocol::PublicationState::WaitingForPublisher
            }
            PublicationState::CheckingEndpoint => protocol::PublicationState::CheckingEndpoint,
            PublicationState::EndpointUnhealthy => protocol::PublicationState::EndpointUnhealthy,
            PublicationState::Ready => protocol::PublicationState::Ready,
            PublicationState::RoutePaused => protocol::PublicationState::RoutePaused,
            PublicationState::InstanceMissing => protocol::PublicationState::InstanceMissing,
        }
    }

    fn outcome<T>(&self, outcome: PublicationOutcome<T>) -> Result<T, protocol::ProtocolError> {
        self.outcome_with_preparation_error(outcome, &Self::operation_canceled())
    }

    fn outcome_with_preparation_error<T>(
        &self,
        outcome: PublicationOutcome<T>,
        preparation_error: &protocol::ProtocolError,
    ) -> Result<T, protocol::ProtocolError> {
        let PublicationOutcome { result, effects } = outcome;
        let changed = effects.has_changes();
        let retired_preparations = effects.retired_preparations.clone();
        let timed_out_preparations = effects.timed_out_preparations.clone();
        let result = result.map_err(|error| Self::error(&error));
        drop(effects);
        for token in retired_preparations {
            self.finish_preparation(&token, Err(preparation_error.clone()));
        }
        for token in timed_out_preparations {
            self.finish_preparation(&token, Err(Self::preparation_timed_out()));
        }
        if changed {
            self.notify_change();
        }
        result
    }

    fn registry_outcome<T>(outcome: PublicationOutcome<T>) -> Result<T, protocol::ProtocolError> {
        outcome.result.map_err(|error| Self::error(&error))
    }

    fn accumulate_registry_outcome<T>(
        effects: &mut PublicationEffects,
        outcome: PublicationOutcome<T>,
    ) -> Result<T, PublicationRegistryError> {
        let PublicationOutcome {
            result,
            effects: outcome_effects,
        } = outcome;
        effects.merge(outcome_effects);
        result
    }

    fn terminal_error(failure: TerminalAttemptFailure) -> protocol::ProtocolError {
        match failure {
            TerminalAttemptFailure::EndpointUnhealthy => protocol::ProtocolError::new(
                protocol::StableErrorCode::EndpointUnhealthy,
                "the candidate endpoint did not satisfy its health policy",
                None,
            ),
            TerminalAttemptFailure::OperationCanceled => protocol::ProtocolError::new(
                protocol::StableErrorCode::OperationCanceled,
                "the publication operation was canceled",
                None,
            ),
            TerminalAttemptFailure::Internal => Self::internal("publication operation failed"),
        }
    }

    fn internal(message: impl Into<String>) -> protocol::ProtocolError {
        protocol::ProtocolError::new(protocol::StableErrorCode::Internal, message, None)
    }

    fn preparation_timed_out() -> protocol::ProtocolError {
        protocol::ProtocolError::new(
            protocol::StableErrorCode::PreparationTimedOut,
            "publisher preparation did not finish before its deadline",
            None,
        )
    }

    fn operation_canceled() -> protocol::ProtocolError {
        protocol::ProtocolError::new(
            protocol::StableErrorCode::OperationCanceled,
            "publisher preparation was canceled",
            None,
        )
    }

    fn inactive_error() -> protocol::ProtocolError {
        protocol::ProtocolError::new(
            protocol::StableErrorCode::OperationCanceled,
            "the publisher authority is shutting down",
            None,
        )
    }

    fn wake_unavailable(error: &WakeError) -> protocol::ProtocolError {
        protocol::ProtocolError::new(
            protocol::StableErrorCode::Internal,
            format!("published endpoint wake safety is unavailable: {error}"),
            Some("restart locald before publishing this endpoint again".to_owned()),
        )
    }

    fn wake_barrier_pending() -> protocol::ProtocolError {
        protocol::ProtocolError::new(
            protocol::StableErrorCode::WakeBarrierPending,
            "the publisher wake barrier is still being applied",
            None,
        )
    }

    fn error(error: &PublicationRegistryError) -> protocol::ProtocolError {
        let code = match error {
            PublicationRegistryError::ServiceNotDeclared => {
                protocol::StableErrorCode::ServiceNotPublished
            }
            PublicationRegistryError::InstanceMissing => protocol::StableErrorCode::ProjectNotFound,
            PublicationRegistryError::AlreadyPublished => {
                protocol::StableErrorCode::AlreadyPublished
            }
            PublicationRegistryError::AcquisitionInProgress => {
                protocol::StableErrorCode::AcquisitionInProgress
            }
            PublicationRegistryError::RebindInProgress => {
                protocol::StableErrorCode::RebindInProgress
            }
            PublicationRegistryError::AttemptStale => protocol::StableErrorCode::AttemptStale,
            PublicationRegistryError::AttemptExpired => protocol::StableErrorCode::AttemptExpired,
            PublicationRegistryError::AttemptMismatch => protocol::StableErrorCode::AttemptMismatch,
            PublicationRegistryError::LeaseLost => protocol::StableErrorCode::LeaseLost,
            PublicationRegistryError::BindingReplaced => protocol::StableErrorCode::BindingReplaced,
            PublicationRegistryError::OriginMismatch => protocol::StableErrorCode::OriginMismatch,
            PublicationRegistryError::ClockUnavailable
            | PublicationRegistryError::ClockRegressed
            | PublicationRegistryError::ClockOverflow
            | PublicationRegistryError::DeclarationConflict
            | PublicationRegistryError::GenerationOverflow
            | PublicationRegistryError::DeadlineNotElapsed => protocol::StableErrorCode::Internal,
            PublicationRegistryError::WaitDeadlineElapsed => {
                protocol::StableErrorCode::WaitTimedOut
            }
        };
        protocol::ProtocolError::new(code, error.to_string(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locald_core::{DomainName, DomainPattern, PublishedHttpHealthPolicy};
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

    #[derive(Debug, Default)]
    struct FakeWakeMonitor {
        sink: StdMutex<Option<Arc<dyn WakeSink>>>,
        registration_error: StdMutex<Option<WakeError>>,
        registration_attempts: AtomicUsize,
    }

    impl FakeWakeMonitor {
        fn suspend(&self) {
            self.sink
                .lock()
                .expect("fake wake sink lock")
                .clone()
                .expect("registered wake sink")
                .suspending();
        }

        fn resume(&self) {
            self.sink
                .lock()
                .expect("fake wake sink lock")
                .clone()
                .expect("registered wake sink")
                .resumed();
        }

        fn fail(&self, error: WakeError) {
            self.sink
                .lock()
                .expect("fake wake sink lock")
                .clone()
                .expect("registered wake sink")
                .failed(error);
        }

        fn fail_registration_with(&self, error: WakeError) {
            *self
                .registration_error
                .lock()
                .expect("fake wake registration error lock") = Some(error);
        }

        fn registration_attempts(&self) -> usize {
            self.registration_attempts.load(Ordering::SeqCst)
        }
    }

    #[derive(Debug)]
    struct FakeWakeRegistration;

    impl WakeRegistration for FakeWakeRegistration {}

    impl WakeMonitor for FakeWakeMonitor {
        fn register(
            &self,
            sink: Arc<dyn WakeSink>,
        ) -> Result<Box<dyn WakeRegistration>, WakeError> {
            self.registration_attempts.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self
                .registration_error
                .lock()
                .expect("fake wake registration error lock")
                .clone()
            {
                return Err(error);
            }
            *self.sink.lock().expect("fake wake sink lock") = Some(sink);
            Ok(Box::new(FakeWakeRegistration))
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

    fn macos_listener(pcb_generation: u64) -> ListenerIdentity {
        ListenerIdentity::MacOsIpv4 {
            address: [127, 0, 0, 1],
            port: 41_555,
            pcb_generation,
        }
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
        let mut registry =
            PublicationRegistry::with_epoch(Arc::new(clock.clone()), DaemonEpoch::from_byte(7));
        registry
            .reconcile_declarations(key.instance(), configuration_revision, [declaration])
            .result
            .expect("admit declaration");
        (registry, clock, key)
    }

    fn authority(registry: PublicationRegistry) -> PublisherAuthority {
        let (changes, _) = watch::channel(0_u64);
        let (wake_signals, wake_receiver) = mpsc::unbounded_channel();
        PublisherAuthority {
            inner: Arc::new(PublisherAuthorityInner {
                registry: Arc::new(Mutex::new(registry)),
                changes,
                preparation_waiters: std::sync::Mutex::new(HashMap::new()),
                wake_activation: std::sync::Mutex::new(()),
                wake_barrier_gate: std::sync::RwLock::new(()),
                wake_observation: std::sync::Mutex::new(PublisherWakeObservation::Inactive),
                wake_signals,
                wake_receiver: std::sync::Mutex::new(Some(wake_receiver)),
                deadline_driver_started: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
                wait_ready_capture_hook: std::sync::Mutex::new(None),
            }),
        }
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
    fn positive_wire_durations_never_collapse_to_zero() {
        assert_eq!(
            PublisherAuthority::duration_ms(Duration::from_nanos(1)).expect("positive duration"),
            1
        );
        assert_eq!(
            PublisherAuthority::duration_ms(Duration::from_micros(1_001))
                .expect("fractional millisecond duration"),
            2
        );
        assert_eq!(
            PublisherAuthority::duration_ms(Duration::ZERO).expect("zero duration"),
            0
        );
    }

    #[tokio::test]
    async fn authority_preparation_join_waits_for_the_owner_completion() {
        let declaration = declaration(instance(22), 1, "twenty-two.localhost");
        let (registry, _clock, key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);
        let owner = principal(1);

        let PublisherAcquisitionPreparation::Required(owner_permit) = authority
            .reserve_acquisition(key.clone(), owner.clone(), None)
            .await
            .expect("reserve owner preparation")
        else {
            panic!("expected preparation owner");
        };

        let joined_authority = authority.clone();
        let joined_key = key.clone();
        let joined_owner = owner.clone();
        let joined = tokio::spawn(async move {
            joined_authority
                .reserve_acquisition(joined_key, joined_owner, None)
                .await
        });
        tokio::task::yield_now().await;
        assert!(!joined.is_finished());

        let completed = authority
            .complete_acquisition_preparation(owner_permit)
            .await
            .expect("complete owner preparation");
        let PublisherAcquisitionPreparation::Ready(joined_result) = joined
            .await
            .expect("joined task")
            .expect("joined preparation")
        else {
            panic!("joined caller must observe the owner's result");
        };
        assert_eq!(
            joined_result.acquisition_attempt_handle(),
            completed.acquisition_attempt_handle()
        );
    }

    #[test]
    fn candidate_preparation_survives_only_its_exact_catalog_candidate() {
        let initial = declaration(instance(24), 1, "twenty-four.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, initial);
        let candidate = declaration(instance(24), 2, "next-twenty-four.localhost");
        let owner = principal(1);
        let deadline = registry
            .begin_candidate_preparation_deadline()
            .result
            .expect("start manager admission deadline");
        let BeginCandidatePreparation::Started(permit) = registry
            .begin_candidate_preparation(&deadline, candidate.clone(), owner.clone(), None)
            .result
            .expect("reserve exact candidate")
        else {
            panic!("expected candidate preparation owner");
        };

        let BeginCandidatePreparation::Joined(joined) = registry
            .begin_candidate_preparation(&deadline, candidate.clone(), owner, None)
            .result
            .expect("join exact candidate")
        else {
            panic!("expected exact candidate join");
        };
        assert_eq!(joined, permit.fence);
        assert_eq!(
            registry
                .begin_candidate_preparation(&deadline, candidate.clone(), principal(2), None)
                .result
                .expect_err("competing principal must fail promptly"),
            PublicationRegistryError::AcquisitionInProgress
        );

        registry
            .reconcile_declarations(key.instance(), 2, [candidate])
            .result
            .expect("publish exact candidate");
        assert!(!permit.cancellation.is_cancelled());
        registry
            .take_candidate_preparation(&permit.fence)
            .result
            .expect("exact candidate permit remains current");
    }

    #[test]
    fn changed_catalog_candidate_cancels_and_fences_stale_preparation() {
        let initial = declaration(instance(25), 1, "twenty-five.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, initial);
        let stale_candidate = declaration(instance(25), 2, "stale-twenty-five.localhost");
        let actual_candidate = declaration(instance(25), 2, "actual-twenty-five.localhost");
        let deadline = registry
            .begin_candidate_preparation_deadline()
            .result
            .expect("start manager admission deadline");
        let BeginCandidatePreparation::Started(stale) = registry
            .begin_candidate_preparation(&deadline, stale_candidate, principal(1), None)
            .result
            .expect("reserve stale candidate")
        else {
            panic!("expected candidate preparation owner");
        };

        registry
            .reconcile_declarations(key.instance(), 2, [actual_candidate])
            .result
            .expect("publish changed candidate");
        assert!(stale.cancellation.is_cancelled());
        assert_eq!(
            registry
                .take_candidate_preparation(&stale.fence)
                .result
                .expect_err("changed candidate fences stale permit"),
            PublicationRegistryError::AttemptStale
        );
    }

    #[test]
    fn candidate_preparation_carries_manager_deadline_and_fences_late_work() {
        let initial = declaration(instance(26), 1, "twenty-six.localhost");
        let (mut registry, clock, _key) = registry(Duration::ZERO, initial);
        let candidate = declaration(instance(26), 2, "next-twenty-six.localhost");
        let deadline = registry
            .begin_candidate_preparation_deadline()
            .result
            .expect("start manager admission deadline");
        clock.advance(Duration::from_secs(45));
        let BeginCandidatePreparation::Started(permit) = registry
            .begin_candidate_preparation(&deadline, candidate.clone(), principal(1), None)
            .result
            .expect("reserve candidate")
        else {
            panic!("expected candidate preparation owner");
        };
        assert_eq!(permit.fence.deadline, deadline.deadline);

        clock.advance(Duration::from_secs(15));
        let effects = registry.sweep_deadlines().effects;
        assert!(permit.cancellation.is_cancelled());
        assert!(effects.timed_out_preparations.contains(&permit.fence.token));
        assert_eq!(
            registry
                .take_candidate_preparation(&permit.fence)
                .result
                .expect_err("elapsed permit cannot issue an attempt"),
            PublicationRegistryError::AttemptStale
        );
        assert_eq!(
            registry
                .begin_candidate_preparation(&deadline, candidate, principal(1), None)
                .result
                .expect_err("late worker cannot restart the admission clock"),
            PublicationRegistryError::AttemptExpired
        );
    }

    #[tokio::test]
    async fn candidate_waiter_reports_suspend_inclusive_timeout_promptly() {
        let initial = declaration(instance(27), 1, "twenty-seven.localhost");
        let (registry, clock, _key) = registry(Duration::ZERO, initial);
        let authority = authority(registry);
        let candidate = declaration(instance(27), 2, "next-twenty-seven.localhost");
        let deadline = authority
            .begin_candidate_preparation_deadline()
            .await
            .expect("start manager admission deadline");
        let PublisherCandidateAcquisitionPreparation::Required(permit) = authority
            .reserve_candidate_acquisition(&deadline, candidate, principal(1), None)
            .await
            .expect("reserve candidate")
        else {
            panic!("expected candidate preparation owner");
        };
        let waiter = permit.completion_waiter();

        clock.advance(PREPARATION_TTL);
        authority
            .sweep_deadlines()
            .await
            .expect("sweep candidate deadline");
        assert_eq!(
            authority
                .wait_for_candidate_preparation(waiter)
                .await
                .expect_err("candidate waiter observes timeout")
                .code(),
            protocol::StableErrorCode::PreparationTimedOut
        );
        assert_eq!(
            authority
                .complete_candidate_acquisition_preparation(*permit)
                .await
                .expect_err("timed-out permit cannot issue an attempt")
                .code(),
            protocol::StableErrorCode::AttemptStale
        );
    }

    #[tokio::test]
    async fn manager_admission_deadline_waiter_uses_fake_clock_and_shutdown_fails_closed() {
        let initial = declaration(instance(28), 1, "twenty-eight.localhost");
        let (registry, clock, _key) = registry(Duration::ZERO, initial);
        let authority = authority(registry);
        let deadline = authority
            .begin_candidate_preparation_deadline()
            .await
            .expect("start manager admission deadline");
        let waiting_authority = authority.clone();
        let timeout = tokio::spawn(async move {
            waiting_authority
                .wait_for_candidate_preparation_deadline(deadline)
                .await
        });
        tokio::task::yield_now().await;
        clock.advance(PREPARATION_TTL);
        authority
            .sweep_deadlines()
            .await
            .expect("publish fake-clock advancement");
        assert_eq!(
            timeout.await.expect("join deadline waiter").code(),
            protocol::StableErrorCode::PreparationTimedOut
        );

        let shutdown_deadline = authority
            .begin_candidate_preparation_deadline()
            .await
            .expect("start shutdown deadline");
        let waiting_authority = authority.clone();
        let shutdown_waiter = tokio::spawn(async move {
            waiting_authority
                .wait_for_candidate_preparation_deadline(shutdown_deadline)
                .await
        });
        tokio::task::yield_now().await;
        authority.shutdown().await;
        assert_eq!(
            shutdown_waiter
                .await
                .expect("join shutdown deadline waiter")
                .code(),
            protocol::StableErrorCode::OperationCanceled
        );
    }

    #[tokio::test]
    async fn authority_shutdown_cancels_preparation_and_rejects_successors() {
        let declaration = declaration(instance(23), 1, "twenty-three.localhost");
        let (registry, _clock, key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);
        let owner = principal(1);
        let PublisherAcquisitionPreparation::Required(permit) = authority
            .reserve_acquisition(key.clone(), owner.clone(), None)
            .await
            .expect("reserve preparation")
        else {
            panic!("expected preparation owner");
        };

        authority.shutdown().await;
        assert!(permit.is_cancelled());
        assert_eq!(
            authority
                .reserve_acquisition(key, owner, None)
                .await
                .expect_err("shutdown authority rejects successors")
                .code(),
            protocol::StableErrorCode::OperationCanceled
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_sandbox_no_host_suspend_guarantee_activates_without_logind() {
        let declaration = declaration(instance(30), 1, "thirty.localhost");
        let (registry, _clock, _key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);

        let system = FakeWakeMonitor::default();
        system.fail_registration_with(WakeError::Unavailable);
        let fallback = FakeWakeMonitor::default();
        let monitor = LinuxSandboxExplicitNoHostSuspendWakeMonitor { system, fallback };

        authority
            .activate_wake_monitor(&monitor)
            .expect("explicit no-host-suspend policy activates without ambient wake services");
        assert_eq!(monitor.system.registration_attempts(), 1);
        assert_eq!(monitor.fallback.registration_attempts(), 1);
        authority
            .ensure_wake_trustworthy()
            .expect("the explicit no-host-suspend precondition admits publication operations");

        authority.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_sandbox_no_host_suspend_guarantee_prefers_system_wake_observation() {
        let declaration = declaration(instance(31), 1, "thirty-one.localhost");
        let (registry, _clock, _key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);
        let monitor = LinuxSandboxExplicitNoHostSuspendWakeMonitor {
            system: FakeWakeMonitor::default(),
            fallback: FakeWakeMonitor::default(),
        };

        authority
            .activate_wake_monitor(&monitor)
            .expect("available system wake observation activates");
        assert_eq!(monitor.system.registration_attempts(), 1);
        assert_eq!(monitor.fallback.registration_attempts(), 0);

        authority.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_sandbox_no_host_suspend_guarantee_propagates_system_failure() {
        let declaration = declaration(instance(32), 1, "thirty-two.localhost");
        let (registry, _clock, _key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);
        let system = FakeWakeMonitor::default();
        system.fail_registration_with(WakeError::Failed("lost logind".to_owned()));
        let monitor = LinuxSandboxExplicitNoHostSuspendWakeMonitor {
            system,
            fallback: FakeWakeMonitor::default(),
        };

        let error = authority
            .activate_wake_monitor(&monitor)
            .expect_err("an active system monitor failure must fail closed");
        assert!(error.to_string().contains("lost logind"));
        assert_eq!(monitor.system.registration_attempts(), 1);
        assert_eq!(monitor.fallback.registration_attempts(), 0);

        authority.shutdown().await;
    }

    #[tokio::test]
    async fn sleep_entry_waits_out_active_transition_and_blocks_successors_before_ack() {
        let declaration = declaration(instance(30), 1, "thirty.localhost");
        let (registry, _clock, _key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);
        let monitor = Arc::new(FakeWakeMonitor::default());
        authority
            .activate_wake_monitor(monitor.as_ref())
            .expect("activate fake wake monitor");

        let active_transition = authority
            .inner
            .wake_barrier_gate
            .read()
            .expect("active publisher transition");
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (returned_tx, returned_rx) = std::sync::mpsc::sync_channel(1);
        let suspending_monitor = Arc::clone(&monitor);
        let suspending = std::thread::spawn(move || {
            entered_tx.send(()).expect("announce sleep callback");
            suspending_monitor.suspend();
            returned_tx.send(()).expect("announce fenced sleep entry");
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("sleep callback starts");
        assert!(
            returned_rx.try_recv().is_err(),
            "sleep acknowledgement must wait for the active publisher transition"
        );

        drop(active_transition);
        returned_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("sleep entry returns after authority is fenced");
        suspending.join().expect("join sleep callback");
        assert_eq!(
            authority
                .ensure_wake_trustworthy()
                .expect_err("sleep entry blocks successor publication operations")
                .code(),
            protocol::StableErrorCode::WakeBarrierPending
        );

        monitor.resume();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if authority.ensure_wake_trustworthy().is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("resume applies the serialized wake barrier");
    }

    #[tokio::test]
    async fn sleeping_and_pending_barrier_reject_renewal_before_mutation() {
        let declaration = declaration(instance(37), 1, "thirty-seven.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 37, &drops);
        let lease_handle = grant.lease.wire();
        let original_expiry = grant.expiry_fence.clone();
        let authority = authority(registry);
        let monitor = FakeWakeMonitor::default();
        authority
            .activate_wake_monitor(&monitor)
            .expect("activate fake wake monitor");

        monitor.suspend();
        assert_eq!(
            authority
                .renew(&lease_handle, &owner)
                .await
                .expect_err("sleeping authority rejects renewal")
                .code(),
            protocol::StableErrorCode::WakeBarrierPending
        );
        {
            let registry = authority.inner.registry.lock().await;
            let SlotState::Live(lease) = &registry.slots.get(&key).expect("live slot").state else {
                panic!("expected live lease");
            };
            assert_eq!(lease.renewal_revision, original_expiry.renewal_revision);
            assert_eq!(lease.deadline, original_expiry.deadline);
        }

        // Hold the intermediate state deterministically so the async barrier
        // driver cannot win the race before the public renewal entry point.
        {
            let mut observation = authority
                .inner
                .wake_observation
                .lock()
                .expect("wake observation");
            let previous = std::mem::replace(&mut *observation, PublisherWakeObservation::Inactive);
            let PublisherWakeObservation::Sleeping(registration) = previous else {
                panic!("expected sleeping wake observation");
            };
            *observation = PublisherWakeObservation::BarrierPending(registration);
        }
        assert_eq!(
            authority
                .renew(&lease_handle, &owner)
                .await
                .expect_err("pending wake barrier rejects renewal")
                .code(),
            protocol::StableErrorCode::WakeBarrierPending
        );
        {
            let registry = authority.inner.registry.lock().await;
            let SlotState::Live(lease) = &registry.slots.get(&key).expect("live slot").state else {
                panic!("expected live lease");
            };
            assert_eq!(lease.renewal_revision, original_expiry.renewal_revision);
            assert_eq!(lease.deadline, original_expiry.deadline);
        }

        authority
            .apply_wake_barrier(true)
            .await
            .expect("apply pending wake barrier");
        authority
            .renew(&lease_handle, &owner)
            .await
            .expect("renewal succeeds after the barrier completes");
        let registry = authority.inner.registry.lock().await;
        let SlotState::Live(lease) = &registry.slots.get(&key).expect("live slot").state else {
            panic!("expected live lease");
        };
        assert_eq!(lease.renewal_revision, original_expiry.renewal_revision + 1);
    }

    #[tokio::test]
    async fn resumed_wake_sweeps_expired_authority_and_releases_exact_wait() {
        let declaration = declaration(instance(31), 1, "thirty-one.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 31, &drops);
        let authority = authority(registry);
        let monitor = FakeWakeMonitor::default();
        authority
            .activate_wake_monitor(&monitor)
            .expect("activate fake wake monitor");

        let waiting_authority = authority.clone();
        let waiting_owner = owner.clone();
        let lease = grant.lease.wire();
        let binding_revision =
            protocol::BindingRevision::new(grant.binding_revision).expect("valid binding revision");
        let waiter = tokio::spawn(async move {
            waiting_authority
                .wait_ready(&lease, &waiting_owner, binding_revision)
                .await
        });
        tokio::task::yield_now().await;

        clock.advance(LEASE_TTL);
        monitor.resume();
        assert_eq!(
            authority
                .ensure_wake_trustworthy()
                .expect_err("wake callback closes the operation gate synchronously")
                .code(),
            protocol::StableErrorCode::WakeBarrierPending
        );
        for _ in 0..32 {
            if waiter.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            waiter.is_finished(),
            "resume must wake the exact readiness wait"
        );
        assert_eq!(
            waiter
                .await
                .expect("join readiness waiter")
                .expect_err("expired authority cannot become ready")
                .code(),
            protocol::StableErrorCode::LeaseLost
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn wait_ready_observes_pause_generation_across_coalesced_resume() {
        let declaration = declaration(instance(36), 1, "thirty-six.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 36, &drops);
        let authority = authority(registry);
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        authority.set_wait_ready_capture_hook(WaitReadyCaptureHook {
            reached: reached.clone(),
            resume: resume.clone(),
        });

        let waiting_authority = authority.clone();
        let waiting_owner = owner.clone();
        let lease = grant.lease.wire();
        let binding_revision =
            protocol::BindingRevision::new(grant.binding_revision).expect("valid binding revision");
        let waiter = tokio::spawn(async move {
            waiting_authority
                .wait_ready(&lease, &waiting_owner, binding_revision)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), reached.notified())
            .await
            .expect("readiness wait captures its exact pause generation");

        authority
            .renew(&grant.lease.wire(), &owner)
            .await
            .expect("ordinary renewal preserves the readiness fence");
        authority
            .set_project_paused(key.instance(), true)
            .await
            .expect("pause route after wait capture");
        authority
            .set_project_paused(key.instance(), false)
            .await
            .expect("resume before the waiter rereads authority");
        resume.notify_one();

        let error = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("coalesced pause and resume terminate the exact wait")
            .expect("join readiness waiter")
            .expect_err("a later pause must not be erased by resume");
        assert_eq!(error.code(), protocol::StableErrorCode::ProjectPaused);

        authority
            .set_project_paused(key.instance(), true)
            .await
            .expect("pause before the second readiness capture");
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        authority.set_wait_ready_capture_hook(WaitReadyCaptureHook {
            reached: reached.clone(),
            resume: resume.clone(),
        });
        let waiting_authority = authority.clone();
        let waiting_owner = owner.clone();
        let lease = grant.lease.wire();
        let binding_revision =
            protocol::BindingRevision::new(grant.binding_revision).expect("valid binding revision");
        let waiter = tokio::spawn(async move {
            waiting_authority
                .wait_ready(&lease, &waiting_owner, binding_revision)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), reached.notified())
            .await
            .expect("readiness wait captures the already-paused policy");
        authority
            .set_project_paused(key.instance(), false)
            .await
            .expect("resume before the already-paused waiter rereads authority");
        resume.notify_one();
        let error = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("paused-at-capture wait terminates after coalesced resume")
            .expect("join paused-at-capture waiter")
            .expect_err("resume must not erase pause observed at wait capture");
        assert_eq!(error.code(), protocol::StableErrorCode::ProjectPaused);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn registry_policy_transition_waits_for_pending_wake_barrier() {
        let declaration = declaration(instance(35), 1, "thirty-five.localhost");
        let (registry, _clock, key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);
        let monitor = FakeWakeMonitor::default();
        authority
            .activate_wake_monitor(&monitor)
            .expect("activate fake wake monitor");

        let registry_guard = authority.inner.registry.lock().await;
        monitor.resume();
        let policy_authority = authority.clone();
        let policy = tokio::spawn(async move {
            policy_authority
                .set_project_paused(instance(35), true)
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !policy.is_finished(),
            "pause publication must not overtake the pending wake barrier"
        );

        drop(registry_guard);
        policy
            .await
            .expect("join pause publication")
            .expect("publish pause after wake barrier");
        assert_eq!(
            authority
                .projection(&key)
                .await
                .expect("read post-barrier projection")
                .map(|(state, _)| state),
            Some(PublicationState::RoutePaused)
        );
    }

    #[tokio::test]
    async fn failed_wake_observation_retires_authority_and_rejects_successors() {
        let declaration = declaration(instance(32), 1, "thirty-two.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 32, &drops);
        let authority = authority(registry);
        let monitor = FakeWakeMonitor::default();
        authority
            .activate_wake_monitor(&monitor)
            .expect("activate fake wake monitor");

        monitor.fail(WakeError::Failed("scripted wake failure".to_owned()));
        for _ in 0..32 {
            if drops.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            authority
                .renew(&grant.lease.wire(), &owner)
                .await
                .expect_err("failed wake observation blocks new authority")
                .code(),
            protocol::StableErrorCode::Internal
        );
        assert_eq!(
            authority
                .projection(&key)
                .await
                .expect("status projection remains available")
                .expect("declared projection")
                .0,
            PublicationState::WaitingForPublisher
        );
    }

    #[test]
    fn readiness_deadline_uses_suspend_clock_even_when_renewal_extends_the_lease() {
        let declaration = declaration(instance(33), 1, "thirty-three.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 33, &drops);
        let wait_fence = registry
            .begin_wait_ready(&grant.lease, &owner, grant.binding_revision)
            .result
            .expect("begin readiness wait");

        clock.advance(Duration::from_secs(10));
        registry
            .renew(&grant.lease, &owner)
            .result
            .expect("renew lease independently");
        clock.advance(Duration::from_secs(20));
        assert_eq!(
            registry
                .wait_projection(&grant.lease, &owner, grant.binding_revision, &wait_fence)
                .result
                .expect_err("suspend-inclusive wait deadline elapsed"),
            PublicationRegistryError::WaitDeadlineElapsed
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());
    }

    #[test]
    fn observed_pause_precedes_elapsed_wait_after_renewal_and_config_transfer() {
        let initial_declaration = declaration(instance(37), 1, "thirty-seven.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, initial_declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 37, &drops);
        let wait_fence = registry
            .begin_wait_ready(&grant.lease, &owner, grant.binding_revision)
            .result
            .expect("capture readiness fence");

        registry
            .reconcile_declarations(
                key.instance(),
                2,
                [declaration(instance(37), 2, "thirty-seven.localhost")],
            )
            .result
            .expect("compatible config transfer preserves live authority");
        clock.advance(Duration::from_secs(10));
        registry
            .renew(&grant.lease, &owner)
            .result
            .expect("renew lease beyond the readiness deadline");
        registry
            .set_paused(key.instance(), true)
            .result
            .expect("pause after readiness capture");
        registry
            .set_paused(key.instance(), false)
            .result
            .expect("resume before readiness reread");
        clock.advance(Duration::from_secs(20));

        let (projection, wait_remaining) = registry
            .wait_projection(&grant.lease, &owner, grant.binding_revision, &wait_fence)
            .result
            .expect("observed pause wins over the coincident wait deadline");
        assert_eq!(projection.state, PublicationState::RoutePaused);
        assert_eq!(wait_remaining, Duration::ZERO);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn elapsed_preparation_reports_timeout_during_an_unrelated_transition() {
        let declaration = declaration(instance(34), 1, "thirty-four.localhost");
        let (registry, clock, key) = registry(Duration::ZERO, declaration);
        let authority = authority(registry);
        let owner = principal(1);
        let PublisherAcquisitionPreparation::Required(_permit) = authority
            .reserve_acquisition(key.clone(), owner.clone(), None)
            .await
            .expect("reserve preparation")
        else {
            panic!("expected preparation owner");
        };
        let joined_authority = authority.clone();
        let joined_key = key.clone();
        let joined = tokio::spawn(async move {
            joined_authority
                .reserve_acquisition(joined_key, owner, None)
                .await
        });
        tokio::task::yield_now().await;

        clock.advance(PREPARATION_TTL);
        authority
            .projection(&key)
            .await
            .expect("unrelated projection sweeps elapsed preparation");
        assert_eq!(
            joined
                .await
                .expect("join preparation waiter")
                .expect_err("joined preparation times out")
                .code(),
            protocol::StableErrorCode::PreparationTimedOut
        );
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
            BeginPreparation::Joined(first.fence.clone())
        );
        assert_eq!(
            registry
                .begin_preparation(&key, principal(2))
                .result
                .expect_err("competing principal must fail"),
            PublicationRegistryError::AcquisitionInProgress
        );
        assert!(!first.cancellation.is_cancelled());

        clock.advance(PREPARATION_TTL);
        let BeginPreparation::Started(successor) = registry
            .begin_preparation(&key, owner)
            .result
            .expect("expired preparation vacates")
        else {
            panic!("expected successor preparation");
        };
        assert!(first.cancellation.is_cancelled());
        assert!(!successor.cancellation.is_cancelled());
        assert_eq!(successor.generation, first.generation + 1);
    }

    #[test]
    fn preparation_failure_replays_until_exact_compare_and_swap_replacement() {
        let declaration = declaration(instance(18), 1, "eighteen.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);

        let BeginPreparation::Started(first) = registry
            .begin_preparation(&key, owner.clone())
            .result
            .expect("begin preparation")
        else {
            panic!("expected started preparation");
        };
        registry
            .fail_preparation(&first, TerminalAttemptFailure::Internal)
            .result
            .expect("record terminal preparation failure");
        assert_eq!(
            registry
                .begin_preparation(&key, owner.clone())
                .result
                .expect("replay terminal preparation failure"),
            BeginPreparation::Terminal {
                fence: first.fence.clone(),
                failure: TerminalAttemptFailure::Internal,
            }
        );
        assert_eq!(
            registry
                .begin_preparation(&key, principal(2))
                .result
                .expect_err("terminal slot remains owned by its exact principal"),
            PublicationRegistryError::AcquisitionInProgress
        );

        let replacement = registry
            .replace_terminal_preparation(&first, &owner)
            .result
            .expect("replace exact terminal preparation");
        assert_eq!(replacement.generation, first.generation + 1);
        assert_eq!(
            registry
                .replace_terminal_preparation(&first, &owner)
                .result
                .expect_err("stale replacement cannot replace its successor"),
            PublicationRegistryError::AttemptStale
        );
        registry
            .complete_preparation(&replacement)
            .result
            .expect("replacement preparation can complete");
    }

    #[test]
    fn terminal_preparation_failure_keeps_its_original_deadline() {
        let declaration = declaration(instance(21), 1, "twenty-one.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);

        let BeginPreparation::Started(first) = registry
            .begin_preparation(&key, owner.clone())
            .result
            .expect("begin preparation")
        else {
            panic!("expected started preparation");
        };
        registry
            .fail_preparation(&first, TerminalAttemptFailure::Internal)
            .result
            .expect("record terminal preparation failure");
        clock.advance(PREPARATION_TTL - Duration::from_secs(1));
        assert_eq!(
            registry
                .begin_preparation(&key, owner.clone())
                .result
                .expect("replay before original deadline"),
            BeginPreparation::Terminal {
                fence: first.fence.clone(),
                failure: TerminalAttemptFailure::Internal,
            }
        );

        clock.advance(Duration::from_secs(1));
        let BeginPreparation::Started(successor) = registry
            .begin_preparation(&key, owner)
            .result
            .expect("original terminal deadline vacates the bounded slot")
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
    fn terminal_acquisition_replay_rejects_a_replacement_macos_listener() {
        let declaration = declaration(instance(31), 1, "thirty-one.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let attempt = begin_attempt(&mut registry, &key, &owner);
        let origin = registry.projection(&key).expect("projection").origin;
        let original_listener = macos_listener(1);
        let BeginAcquire::Started(fence) = registry
            .begin_acquire(&attempt, &owner, &origin, &original_listener)
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
                .begin_acquire(&attempt, &owner, &origin, &original_listener)
                .result
                .expect("exact listener replays terminal result"),
            BeginAcquire::Terminal(TerminalAttemptFailure::EndpointUnhealthy)
        );
        assert_eq!(
            registry
                .begin_acquire(&attempt, &owner, &origin, &macos_listener(2))
                .result
                .expect_err("replacement listener must not inherit terminal replay"),
            PublicationRegistryError::AttemptMismatch
        );
    }

    #[test]
    fn renewal_uses_commit_time_and_stale_expiry_enforces_the_current_deadline() {
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
        let expired = registry.expire(&first.expiry_fence);
        assert!(
            expired
                .result
                .expect("late stale expiry retires elapsed current lease")
        );
        drop(expired.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry.projection(&key).expect("projection").state,
            PublicationState::WaitingForPublisher
        );
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
    fn stale_expiry_from_retired_lease_cannot_retire_a_successor_generation() {
        let declaration = declaration(instance(20), 1, "twenty.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_first_attempt, first) = publish(&mut registry, &key, &owner, 20, &drops);
        let released = registry.release(&first.lease, &owner);
        released.result.expect("release first lease");
        drop(released.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        let (_second_attempt, second) = publish(&mut registry, &key, &owner, 21, &drops);
        clock.advance(LEASE_TTL);
        assert!(
            !registry
                .expire(&first.expiry_fence)
                .result
                .expect("old lease callback is fenced from successor")
        );
        assert_eq!(
            registry.projection(&key).expect("projection").state,
            PublicationState::CheckingEndpoint
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        let expired = registry.expire(&second.expiry_fence);
        assert!(expired.result.expect("successor expiry is authoritative"));
        drop(expired.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
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
    fn pause_policy_applies_to_declarations_added_after_the_pause() {
        let first = declaration(instance(19), 1, "nineteen.localhost");
        let (mut registry, _clock, first_key) = registry(Duration::ZERO, first.clone());
        registry
            .set_paused(first_key.instance(), true)
            .result
            .expect("pause project routes");

        let mut retained = first;
        retained.configuration_revision = 2;
        let mut added = declaration(instance(19), 2, "preview.nineteen.localhost");
        added.service_name = "preview".into();
        let added_key = ServiceKey::new(added.project_instance_id, added.service_name.clone());
        registry
            .reconcile_declarations(first_key.instance(), 2, [retained, added])
            .result
            .expect("add declaration while project is paused");
        assert_eq!(
            registry
                .projection(&added_key)
                .expect("added projection")
                .state,
            PublicationState::RoutePaused
        );

        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, _grant) = publish(&mut registry, &added_key, &owner, 19, &drops);
        assert_eq!(
            registry
                .projection(&added_key)
                .expect("published projection")
                .state,
            PublicationState::RoutePaused
        );

        let resumed = registry.set_paused(first_key.instance(), false);
        resumed.result.expect("resume project routes");
        assert!(resumed.effects.probe_required.contains(&added_key));
        assert_eq!(
            registry
                .projection(&added_key)
                .expect("resumed projection")
                .state,
            PublicationState::CheckingEndpoint
        );
    }

    #[test]
    fn missing_policy_applies_to_future_declarations_until_instance_retirement() {
        let project_instance = instance(22);
        let clock = FakePublicationClock::new(Duration::ZERO);
        let mut registry =
            PublicationRegistry::with_epoch(Arc::new(clock), DaemonEpoch::from_byte(22));
        registry
            .set_missing(project_instance, true)
            .result
            .expect("mark instance missing before discovery");

        let first = declaration(project_instance, 1, "twenty-two.localhost");
        let first_key = ServiceKey::new(project_instance, first.service_name.clone());
        registry
            .reconcile_declarations(project_instance, 1, [first.clone()])
            .result
            .expect("admit declaration for missing instance");
        assert_eq!(
            registry
                .projection(&first_key)
                .expect("first projection")
                .state,
            PublicationState::InstanceMissing
        );
        assert_eq!(
            registry
                .begin_preparation(&first_key, principal(1))
                .result
                .expect_err("missing instance cannot prepare publication"),
            PublicationRegistryError::InstanceMissing
        );

        let mut retained = first;
        retained.configuration_revision = 2;
        let mut added = declaration(project_instance, 2, "preview.twenty-two.localhost");
        added.service_name = "preview".into();
        let added_key = ServiceKey::new(project_instance, added.service_name.clone());
        registry
            .reconcile_declarations(project_instance, 2, [retained, added])
            .result
            .expect("add declaration while instance remains missing");
        assert_eq!(
            registry
                .projection(&added_key)
                .expect("added projection")
                .state,
            PublicationState::InstanceMissing
        );
        assert_eq!(
            registry
                .begin_preparation(&added_key, principal(1))
                .result
                .expect_err("later declaration inherits missing policy"),
            PublicationRegistryError::InstanceMissing
        );

        registry
            .retire_instance(project_instance)
            .result
            .expect("retire missing instance");
        let replacement = declaration(project_instance, 3, "twenty-two.localhost");
        registry
            .reconcile_admitted_reregistration(project_instance, 3, [replacement])
            .result
            .expect("re-admit retired instance at a newer revision");
        assert_eq!(
            registry
                .projection(&first_key)
                .expect("replacement projection")
                .state,
            PublicationState::WaitingForPublisher
        );
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
        assert!(stale_fence.cancellation.is_cancelled());
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
        assert!(!successor_fence.cancellation.is_cancelled());
        assert_eq!(
            registry
                .commit_acquire(&stale_fence, capability(14, &drops))
                .result
                .expect_err("pre-wake fence cannot become current again"),
            PublicationRegistryError::AttemptStale
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!successor_fence.cancellation.is_cancelled());
        assert!(
            registry
                .commit_acquire(&successor_fence, capability(15, &drops))
                .result
                .is_ok()
        );
    }

    #[test]
    fn acquisition_deadline_cancels_in_flight_work_without_touching_a_successor() {
        let declaration = declaration(instance(23), 1, "twenty-three.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let attempt = begin_attempt(&mut registry, &key, &owner);
        let origin = registry.projection(&key).expect("projection").origin;
        let BeginAcquire::Started(stale) = registry
            .begin_acquire(&attempt, &owner, &origin, &ListenerIdentity::Test(23))
            .result
            .expect("begin acquisition work")
        else {
            panic!("expected in-flight acquisition");
        };
        assert!(!stale.cancellation.is_cancelled());

        clock.advance(ACQUISITION_ATTEMPT_TTL);
        registry
            .snapshot(&key)
            .result
            .expect("sweep exact deadline");
        assert!(stale.cancellation.is_cancelled());

        let successor_attempt = begin_attempt(&mut registry, &key, &owner);
        let BeginAcquire::Started(successor) = registry
            .begin_acquire(
                &successor_attempt,
                &owner,
                &origin,
                &ListenerIdentity::Test(24),
            )
            .result
            .expect("begin successor acquisition")
        else {
            panic!("expected successor acquisition");
        };
        assert!(!successor.cancellation.is_cancelled());
        assert_eq!(
            registry
                .commit_acquire(&stale, capability(23, &drops))
                .result
                .expect_err("expired work cannot commit"),
            PublicationRegistryError::AttemptStale
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!successor.cancellation.is_cancelled());
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
    fn retirement_fences_delayed_newer_reconciliation_until_atomic_reregistration() {
        let instance = instance(32);
        let first = declaration(instance, 1, "thirty-two.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, first.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_attempt, grant) = publish(&mut registry, &key, &owner, 32, &drops);
        let mut delayed = first.clone();
        delayed.configuration_revision = 2;

        let retired = registry.retire_instance(instance);
        retired.result.expect("retire exact instance");
        drop(retired.effects);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(registry.projection(&key).is_none());
        assert_eq!(
            registry
                .renew(&grant.lease, &owner)
                .result
                .expect_err("retirement revokes the prior lease"),
            PublicationRegistryError::LeaseLost
        );

        assert_eq!(
            registry
                .reconcile_declarations(instance, 2, [delayed.clone()])
                .result
                .expect_err("delayed newer reconciliation remains fenced"),
            PublicationRegistryError::DeclarationConflict
        );
        assert!(registry.projection(&key).is_none());

        assert_eq!(
            registry
                .reconcile_admitted_reregistration(instance, 1, [first])
                .result
                .expect_err("re-registration must advance the retired high-water revision"),
            PublicationRegistryError::DeclarationConflict
        );
        assert!(registry.projection(&key).is_none());
        assert_eq!(
            registry
                .reconcile_declarations(instance, 2, [delayed.clone()])
                .result
                .expect_err("failed admission leaves the retirement fence intact"),
            PublicationRegistryError::DeclarationConflict
        );

        registry
            .reconcile_admitted_reregistration(instance, 2, [delayed])
            .result
            .expect("atomic re-registration admits the newer declaration");
        assert_eq!(
            registry
                .projection(&key)
                .expect("re-registered projection")
                .state,
            PublicationState::WaitingForPublisher
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
        let mut registry =
            PublicationRegistry::with_epoch(Arc::new(clock), DaemonEpoch::from_byte(3));
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
    fn terminal_rebind_replay_rejects_a_replacement_macos_listener() {
        let declaration = declaration(instance(33), 1, "thirty-three.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, grant) = publish(&mut registry, &key, &owner, 33, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let original_listener = macos_listener(3);
        let BeginRebindCandidate::Started(fence) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &original_listener)
            .result
            .expect("begin candidate")
        else {
            panic!("expected fresh candidate");
        };
        registry
            .fail_rebind(&fence, TerminalAttemptFailure::EndpointUnhealthy)
            .result
            .expect("record terminal rebind failure");

        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &original_listener)
                .result
                .expect("exact listener replays terminal result"),
            BeginRebindCandidate::Terminal(TerminalAttemptFailure::EndpointUnhealthy)
        );
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &macos_listener(4))
                .result
                .expect_err("replacement listener must not inherit terminal replay"),
            PublicationRegistryError::AttemptMismatch
        );
    }

    #[test]
    fn committed_rebind_replay_survives_compatible_declaration_transfers() {
        let first_declaration = declaration(instance(24), 1, "twenty-four.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, first_declaration.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, first) = publish(&mut registry, &key, &owner, 24, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&first.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let BeginRebindCandidate::Started(candidate) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(25))
            .result
            .expect("begin candidate")
        else {
            panic!("expected fresh candidate");
        };
        registry
            .commit_rebind(&candidate, capability(25, &drops))
            .result
            .expect("commit candidate");

        let mut alias_only = first_declaration.clone();
        alias_only.configuration_revision = 2;
        registry
            .reconcile_declarations(key.instance(), 2, [alias_only.clone()])
            .result
            .expect("transfer alias-only declaration");
        assert!(matches!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(25))
                .result
                .expect("replay after alias-only transfer"),
            BeginRebindCandidate::Replay(LeaseGrant {
                binding_revision: 2,
                ..
            })
        ));

        let mut health_reload = alias_only;
        health_reload.configuration_revision = 3;
        health_reload.health_policy =
            PublishedHttpHealthPolicy::new("/ready", 2, 5).expect("valid health policy");
        let transferred = registry.reconcile_declarations(key.instance(), 3, [health_reload]);
        transferred.result.expect("transfer health policy");
        assert!(transferred.effects.probe_required.contains(&key));
        assert!(matches!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(25))
                .result
                .expect("replay after health-policy transfer"),
            BeginRebindCandidate::Replay(LeaseGrant {
                binding_revision: 2,
                ..
            })
        ));

        clock.advance(REBIND_ATTEMPT_TTL);
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(25))
                .result
                .expect_err("bounded replay expires at its original deadline"),
            PublicationRegistryError::AttemptStale
        );
    }

    #[test]
    fn failed_rebind_replay_survives_alias_transfer_until_original_deadline() {
        let first_declaration = declaration(instance(30), 1, "thirty.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, first_declaration.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, grant) = publish(&mut registry, &key, &owner, 30, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let listener = ListenerIdentity::Test(31);
        let BeginRebindCandidate::Started(candidate) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &listener)
            .result
            .expect("begin candidate")
        else {
            panic!("expected fresh candidate");
        };
        registry
            .fail_rebind(&candidate, TerminalAttemptFailure::EndpointUnhealthy)
            .result
            .expect("record failed rebind");

        let mut alias_only = first_declaration;
        alias_only.configuration_revision = 2;
        alias_only.domain_claims.insert(DomainPattern::exact(
            "alias.thirty.localhost"
                .parse()
                .expect("valid alias domain"),
        ));
        let transferred = registry.reconcile_declarations(key.instance(), 2, [alias_only]);
        transferred.result.expect("transfer alias-only declaration");
        assert!(!transferred.effects.probe_required.contains(&key));
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &listener)
                .result
                .expect("replay failure after alias-only transfer"),
            BeginRebindCandidate::Terminal(TerminalAttemptFailure::EndpointUnhealthy)
        );

        clock.advance(REBIND_ATTEMPT_TTL - Duration::from_nanos(1));
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &listener)
                .result
                .expect("replay failure before original deadline"),
            BeginRebindCandidate::Terminal(TerminalAttemptFailure::EndpointUnhealthy)
        );
        clock.advance(Duration::from_nanos(1));
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &listener)
                .result
                .expect_err("bounded replay expires at its original deadline"),
            PublicationRegistryError::AttemptStale
        );
    }

    #[test]
    fn health_policy_change_invalidates_failed_rebind_replay() {
        let first_declaration = declaration(instance(31), 1, "thirty-one.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, first_declaration.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, grant) = publish(&mut registry, &key, &owner, 31, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let listener = ListenerIdentity::Test(32);
        let BeginRebindCandidate::Started(candidate) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &listener)
            .result
            .expect("begin candidate")
        else {
            panic!("expected fresh candidate");
        };
        registry
            .fail_rebind(&candidate, TerminalAttemptFailure::EndpointUnhealthy)
            .result
            .expect("record failed rebind");

        let mut changed_policy = first_declaration;
        changed_policy.configuration_revision = 2;
        changed_policy.health_policy =
            PublishedHttpHealthPolicy::new("/ready", 2, 5).expect("valid health policy");
        let transferred = registry.reconcile_declarations(key.instance(), 2, [changed_policy]);
        transferred.result.expect("transfer changed health policy");
        assert!(transferred.effects.probe_required.contains(&key));
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &listener)
                .result
                .expect_err("old failure does not replay under a new health policy"),
            PublicationRegistryError::AttemptStale
        );

        let BeginRebind::Started {
            handle: successor, ..
        } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin replacement rebind")
        else {
            panic!("expected replacement rebind");
        };
        assert_ne!(successor, handle);
    }

    #[test]
    fn committed_rebind_replay_survives_pause_resume_and_wake_until_deadline() {
        let declaration = declaration(instance(26), 1, "twenty-six.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, first) = publish(&mut registry, &key, &owner, 26, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&first.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let BeginRebindCandidate::Started(candidate) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(27))
            .result
            .expect("begin candidate")
        else {
            panic!("expected fresh candidate");
        };
        registry
            .commit_rebind(&candidate, capability(27, &drops))
            .result
            .expect("commit candidate");

        registry
            .set_paused(key.instance(), true)
            .result
            .expect("pause route");
        assert!(matches!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(27))
                .result
                .expect("replay while paused"),
            BeginRebindCandidate::Replay(LeaseGrant {
                binding_revision: 2,
                ..
            })
        ));

        registry
            .set_paused(key.instance(), false)
            .result
            .expect("resume route");
        registry
            .wake_barrier(true)
            .result
            .expect("trustworthy wake");
        assert!(matches!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(27))
                .result
                .expect("replay after resume and wake"),
            BeginRebindCandidate::Replay(LeaseGrant {
                binding_revision: 2,
                ..
            })
        ));

        clock.advance(REBIND_ATTEMPT_TTL);
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(27))
                .result
                .expect_err("bounded replay expires at its original deadline"),
            PublicationRegistryError::AttemptStale
        );
    }

    #[test]
    fn failed_rebind_replay_survives_pause_resume_and_wake_until_deadline() {
        let declaration = declaration(instance(28), 1, "twenty-eight.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, grant) = publish(&mut registry, &key, &owner, 28, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let BeginRebindCandidate::Started(candidate) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(29))
            .result
            .expect("begin candidate")
        else {
            panic!("expected fresh candidate");
        };
        registry
            .fail_rebind(&candidate, TerminalAttemptFailure::EndpointUnhealthy)
            .result
            .expect("record failed rebind");

        registry
            .set_paused(key.instance(), true)
            .result
            .expect("pause route");
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(29))
                .result
                .expect("replay failure while paused"),
            BeginRebindCandidate::Terminal(TerminalAttemptFailure::EndpointUnhealthy)
        );

        registry
            .set_paused(key.instance(), false)
            .result
            .expect("resume route");
        registry
            .wake_barrier(true)
            .result
            .expect("trustworthy wake");
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(29))
                .result
                .expect("replay failure after resume and wake"),
            BeginRebindCandidate::Terminal(TerminalAttemptFailure::EndpointUnhealthy)
        );

        clock.advance(REBIND_ATTEMPT_TTL);
        assert_eq!(
            registry
                .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(29))
                .result
                .expect_err("bounded replay expires at its original deadline"),
            PublicationRegistryError::AttemptStale
        );
    }

    #[test]
    fn pause_and_wake_retire_pending_and_in_flight_rebind_work() {
        let declaration = declaration(instance(27), 1, "twenty-seven.localhost");
        let (mut registry, _clock, key) = registry(Duration::ZERO, declaration);
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, grant) = publish(&mut registry, &key, &owner, 27, &drops);
        let BeginRebind::Started {
            handle: pending,
            origin,
            ..
        } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin pending rebind")
        else {
            panic!("expected pending rebind");
        };

        registry
            .set_paused(key.instance(), true)
            .result
            .expect("pause route");
        assert_eq!(
            registry
                .begin_rebind_candidate(&pending, &owner, &origin, &ListenerIdentity::Test(28),)
                .result
                .expect_err("pause retires pending candidate"),
            PublicationRegistryError::AttemptStale
        );
        registry
            .set_paused(key.instance(), false)
            .result
            .expect("resume route");

        let BeginRebind::Started {
            handle: in_flight, ..
        } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin in-flight rebind")
        else {
            panic!("expected successor rebind");
        };
        let BeginRebindCandidate::Started(candidate) = registry
            .begin_rebind_candidate(&in_flight, &owner, &origin, &ListenerIdentity::Test(29))
            .result
            .expect("begin in-flight candidate")
        else {
            panic!("expected in-flight candidate");
        };
        assert!(!candidate.cancellation.is_cancelled());

        registry
            .wake_barrier(true)
            .result
            .expect("trustworthy wake");
        assert!(candidate.cancellation.is_cancelled());
        assert_eq!(
            registry
                .commit_rebind(&candidate, capability(29, &drops))
                .result
                .expect_err("wake-canceled candidate cannot replace the binding"),
            PublicationRegistryError::AttemptStale
        );
    }

    #[test]
    fn rebind_retirement_cancels_only_the_candidate_and_preserves_the_live_binding() {
        let declaration = declaration(instance(25), 1, "twenty-five.localhost");
        let (mut registry, clock, key) = registry(Duration::ZERO, declaration.clone());
        let owner = principal(1);
        let drops = Arc::new(AtomicUsize::new(0));
        let (_acquisition, grant) = publish(&mut registry, &key, &owner, 25, &drops);
        let BeginRebind::Started { handle, origin, .. } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin rebind")
        else {
            panic!("expected fresh rebind");
        };
        let BeginRebindCandidate::Started(stale) = registry
            .begin_rebind_candidate(&handle, &owner, &origin, &ListenerIdentity::Test(26))
            .result
            .expect("begin candidate")
        else {
            panic!("expected in-flight candidate");
        };
        assert!(!stale.cancellation.is_cancelled());

        clock.advance(REBIND_ATTEMPT_TTL);
        registry
            .snapshot(&key)
            .result
            .expect("sweep rebind deadline");
        assert!(stale.cancellation.is_cancelled());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());

        let BeginRebind::Started {
            handle: successor_handle,
            ..
        } = registry
            .begin_rebind(&grant.lease, &owner, 1)
            .result
            .expect("begin successor rebind")
        else {
            panic!("expected successor rebind");
        };
        let BeginRebindCandidate::Started(successor) = registry
            .begin_rebind_candidate(
                &successor_handle,
                &owner,
                &origin,
                &ListenerIdentity::Test(27),
            )
            .result
            .expect("begin successor candidate")
        else {
            panic!("expected successor candidate");
        };
        assert!(!successor.cancellation.is_cancelled());
        let mut compatible_reload = declaration;
        compatible_reload.configuration_revision = 2;
        registry
            .reconcile_declarations(key.instance(), 2, [compatible_reload])
            .result
            .expect("compatible reload preserves the binding");
        assert!(successor.cancellation.is_cancelled());
        assert_eq!(
            registry
                .commit_rebind(&stale, capability(26, &drops))
                .result
                .expect_err("expired candidate cannot replace the binding"),
            PublicationRegistryError::AttemptStale
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry
                .commit_rebind(&successor, capability(27, &drops))
                .result
                .expect_err("reload-canceled candidate cannot replace the binding"),
            PublicationRegistryError::AttemptStale
        );
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        assert!(registry.renew(&grant.lease, &owner).result.is_ok());
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

        let mut restarted =
            PublicationRegistry::with_epoch(Arc::new(clock), DaemonEpoch::from_byte(99));
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
    fn daemon_epoch_is_128_bits_and_redacted() {
        let epoch = DaemonEpoch::from_byte(1);
        assert_eq!(epoch.0.len(), 16);
        assert_eq!(format!("{epoch:?}"), "DaemonEpoch(<redacted>)");
    }

    #[test]
    fn debug_output_redacts_every_private_authority_type() {
        let principal = principal(42);
        let epoch = DaemonEpoch::from_byte(1);
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
