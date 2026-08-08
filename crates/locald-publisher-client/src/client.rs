use std::fmt;
use std::net::TcpListener;
use std::os::fd::AsFd as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use locald_publisher_protocol::{
    AbsolutePath, AcquireArguments, AcquireResult, AcquisitionAttemptHandle, AttemptState,
    BeginAcquisitionArguments, BeginAcquisitionResult, BeginRebindArguments, BeginRebindResult,
    BindingRevision, DaemonEpoch, DescriptorPrelude, FrameError, LeaseHandle, ProjectInstanceId,
    ProtocolError, PublishedEndpointProtocolInfo, PublisherRequest, RebindArguments,
    RebindAttemptHandle, RebindResult, ReleaseArguments, ReleaseResult, RenewArguments,
    RenewResult, RequestEnvelope, ResponseEnvelope, SemanticOrigin, ServiceName, StableErrorCode,
    WaitReadyArguments, WaitReadyResult, decode_response_frame, encode_request_frame,
};
use nix::unistd::Uid;
use thiserror::Error;

use crate::backend::{
    AuthenticatedDaemonDiscovery, BackendError, DeliveryCertainty, PublisherTransport,
    TransportFailure,
};
use crate::clock::{ClockError, RenewalSchedule, SuspendAwareClock, SuspendInstant};
use crate::installation::InstalledPublisher;
use crate::supervisor::{
    LeaseSnapshot, LeaseState, PendingSupervisor, RenewalCause, SessionFence, SharedSession,
    SupervisorCallError, SupervisorDriver, SupervisorHandle, SupervisorShared,
};
use crate::wake::{InactiveWakeMonitor, WakeError, WakeMonitor};

const SUPERVISOR_TIMEOUT_FRAME_MULTIPLIER: u32 = 3;
const SUPERVISOR_TIMEOUT_MARGIN: Duration = Duration::from_secs(1);

/// Supported publisher client.
#[derive(Clone)]
pub struct PublisherClient {
    inner: Arc<ClientInner>,
    discovery: Arc<dyn AuthenticatedDaemonDiscovery>,
}

struct ClientInner {
    transport: Arc<dyn PublisherTransport>,
    clock: Arc<dyn SuspendAwareClock>,
    wake_monitor: Arc<dyn WakeMonitor>,
    expected_uid: u32,
    discovery_gate: Mutex<()>,
    clock_state: Mutex<ClockState>,
    active_epoch: Mutex<Option<DaemonEpoch>>,
    session: Arc<SharedSession>,
}

#[derive(Debug, Default)]
struct ClockState {
    last_observed: Option<SuspendInstant>,
    failure: Option<ClockError>,
}

impl fmt::Debug for ClientInner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientInner")
            .field("transport", &self.transport)
            .field("clock", &self.clock)
            .field("wake_monitor", &self.wake_monitor)
            .field("expected_uid", &self.expected_uid)
            .finish_non_exhaustive()
    }
}

impl ClientInner {
    fn now(&self) -> Result<SuspendInstant, ClockError> {
        let mut state = self
            .clock_state
            .lock()
            .map_err(|_| ClockError::Unavailable)?;
        if let Some(error) = state.failure {
            return Err(error);
        }
        let observed = self.clock.now();
        let result = match observed {
            Ok(now) if state.last_observed.is_some_and(|last| now < last) => {
                Err(ClockError::Regressed)
            }
            Ok(now) => {
                state.last_observed = Some(now);
                Ok(now)
            }
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            state.failure = Some(error);
            drop(state);
            self.session.fail_uncertain();
        }
        result
    }

    fn activate_epoch(&self, epoch: &DaemonEpoch) -> SessionFence {
        let mut active_epoch = self
            .active_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active_epoch.as_ref().is_some_and(|active| active != epoch) {
            self.session.invalidate();
        }
        *active_epoch = Some(epoch.clone());
        drop(active_epoch);
        self.session.fence()
    }
}

impl fmt::Debug for PublisherClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublisherClient")
            .field("expected_uid", &self.inner.expected_uid)
            .field("discovery", &self.discovery)
            .field("transport", &self.inner.transport)
            .field("clock", &self.inner.clock)
            .field("wake_monitor", &self.inner.wake_monitor)
            .finish()
    }
}

impl PublisherClient {
    /// Construct the production client while publisher discovery remains
    /// inactive. An advertised production transport must install a conforming
    /// wake monitor and use [`Self::with_wake_monitor`].
    #[must_use]
    pub fn new(
        discovery: Arc<dyn AuthenticatedDaemonDiscovery>,
        transport: Arc<dyn PublisherTransport>,
        clock: Arc<dyn SuspendAwareClock>,
    ) -> Self {
        Self::with_wake_monitor(discovery, transport, clock, Arc::new(InactiveWakeMonitor))
    }

    /// Construct a client with an explicit suspend-inclusive clock and wake
    /// monitor. Both are mandatory inputs to client-owned lease supervision.
    #[must_use]
    pub fn with_wake_monitor(
        discovery: Arc<dyn AuthenticatedDaemonDiscovery>,
        transport: Arc<dyn PublisherTransport>,
        clock: Arc<dyn SuspendAwareClock>,
        wake_monitor: Arc<dyn WakeMonitor>,
    ) -> Self {
        Self {
            inner: Arc::new(ClientInner {
                transport,
                clock,
                wake_monitor,
                expected_uid: Uid::effective().as_raw(),
                discovery_gate: Mutex::new(()),
                clock_state: Mutex::new(ClockState::default()),
                active_epoch: Mutex::new(None),
                session: SharedSession::new(),
            }),
            discovery,
        }
    }

    /// Resolve the daemon-observed project instance through authenticated
    /// ordinary IPC. The supplied path remains only a locator.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when installation validation, authenticated
    /// project resolution, or peer-identity verification fails.
    pub fn for_project(
        &self,
        installation: &InstalledPublisher,
        project_locator: AbsolutePath,
    ) -> Result<ProjectPublisher, ClientError> {
        self.validate_installed(installation)?;
        // Serialize the live discovery observation with epoch activation. A
        // clone holding an older installation snapshot must not be able to
        // race a newer observation and roll the shared session backward.
        let _discovery_guard = self
            .inner
            .discovery_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let resolved = self
            .discovery
            .resolve_project(installation.record().command_socket(), &project_locator)
            .map_err(ClientError::Backend)?;
        self.validate_peer_uid(resolved.peer_uid)?;
        let protocol_info = self
            .discovery
            .protocol_info(installation.record().command_socket())
            .map_err(ClientError::Backend)?;
        self.validate_peer_uid(protocol_info.peer_uid)?;
        protocol_info.value.validate().map_err(|error| {
            ClientError::InvalidDiscovery(format!(
                "daemon returned invalid publisher protocol info: {error}"
            ))
        })?;
        if protocol_info.value.publisher_socket() != installation.protocol_info().publisher_socket()
        {
            return Err(ClientError::InvalidDiscovery(
                "daemon publisher socket differs from the verified installation".to_owned(),
            ));
        }
        let protocol_info = protocol_info.value;
        let fence = self.inner.activate_epoch(protocol_info.daemon_epoch());
        Ok(ProjectPublisher {
            inner: Arc::clone(&self.inner),
            protocol_info,
            project_locator,
            project_instance_id: resolved.value,
            fence,
        })
    }

    fn validate_installed(&self, installed: &InstalledPublisher) -> Result<(), ClientError> {
        installed.record().validate().map_err(|error| {
            ClientError::InvalidDiscovery(format!("invalid installation record: {error}"))
        })?;
        installed.protocol_info().validate().map_err(|error| {
            ClientError::InvalidDiscovery(format!("invalid publisher protocol info: {error}"))
        })?;
        self.validate_peer_uid(installed.daemon_uid())
    }

    fn validate_peer_uid(&self, peer_uid: u32) -> Result<(), ClientError> {
        if peer_uid == self.inner.expected_uid {
            Ok(())
        } else {
            Err(ClientError::PeerUidMismatch {
                expected: self.inner.expected_uid,
                actual: peer_uid,
            })
        }
    }
}

/// Exact daemon-observed project context for one publisher workflow.
#[derive(Debug, Clone)]
pub struct ProjectPublisher {
    inner: Arc<ClientInner>,
    protocol_info: PublishedEndpointProtocolInfo,
    project_locator: AbsolutePath,
    project_instance_id: ProjectInstanceId,
    fence: SessionFence,
}

impl ProjectPublisher {
    /// Daemon-observed stable project-instance identity carried by every begin.
    #[must_use]
    pub const fn project_instance_id(&self) -> ProjectInstanceId {
        self.project_instance_id
    }

    /// Begin one bounded acquisition preparation. A terminal result remains
    /// exact-replay authority; replacement is an explicit consuming typestate
    /// operation and never exposes its opaque handle.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the authenticated begin exchange fails or
    /// yields an invalid response.
    pub fn prepare(&self, service_name: ServiceName) -> Result<PreparedPublication, ClientError> {
        self.prepare_with_replacement(service_name, None)
    }

    fn prepare_with_replacement(
        &self,
        service_name: ServiceName,
        replace_terminal_attempt_handle: Option<AcquisitionAttemptHandle>,
    ) -> Result<PreparedPublication, ClientError> {
        let response = self.exchange::<BeginAcquisitionResult>(
            PublisherRequest::BeginAcquisition(BeginAcquisitionArguments {
                expected_project_instance_id: self.project_instance_id,
                project_locator: self.project_locator.clone(),
                service_name: service_name.clone(),
                replace_terminal_attempt_handle,
            }),
            None,
            None,
        )?;
        if response.value.expected_project_instance_id() != self.project_instance_id {
            return Err(ClientError::ProjectInstanceMismatch {
                expected: self.project_instance_id,
                actual: response.value.expected_project_instance_id(),
            });
        }
        Ok(PreparedPublication {
            project: self.clone(),
            service_name,
            attempt_handle: response.value.acquisition_attempt_handle().clone(),
            origin: response.value.origin().clone(),
            attempt_state: response.value.attempt_state(),
        })
    }

    fn exchange<'a, R: serde::de::DeserializeOwned>(
        &'a self,
        request: PublisherRequest,
        listener: Option<std::os::fd::BorrowedFd<'_>>,
        authority_schedule: Option<RenewalSchedule>,
    ) -> Result<TimedResponse<'a, R>, ClientError> {
        if !self.fence.is_current() {
            return Err(ClientError::DaemonEpochChanged);
        }
        let exact_replay = matches!(
            &request,
            PublisherRequest::Acquire(_)
                | PublisherRequest::Renew(_)
                | PublisherRequest::Rebind(_)
                | PublisherRequest::Release(_)
        );
        let schedule_bearing = matches!(
            &request,
            PublisherRequest::Acquire(_) | PublisherRequest::Renew(_) | PublisherRequest::Rebind(_)
        );
        let envelope = RequestEnvelope::v1(self.protocol_info.daemon_epoch().clone(), request);
        let frame = encode_request_frame(&envelope).map_err(ClientError::Frame)?;
        let has_listener = listener.is_some();
        if (frame.descriptor() == DescriptorPrelude::Listener) != has_listener {
            return Err(ClientError::ListenerDescriptorMismatch);
        }
        let request_started = if schedule_bearing || authority_schedule.is_some() {
            Some(self.inner.now().map_err(ClientError::Clock)?)
        } else {
            None
        };
        if let (Some(schedule), Some(request_started)) = (authority_schedule, request_started)
            && schedule.expired(request_started)
        {
            return Err(ClientError::LeaseExpired);
        }

        let reply = match self.inner.transport.exchange(
            self.protocol_info.publisher_socket(),
            &frame,
            listener,
        ) {
            Ok(reply) => reply,
            Err(first_failure)
                if exact_replay
                    && matches!(
                        first_failure.certainty,
                        DeliveryCertainty::NotSent | DeliveryCertainty::OutcomeUnknown
                    ) =>
            {
                // These mutation phases have a protocol-defined convergence rule:
                // acquire/rebind replay their exact terminal request, renew
                // repeats on the same live lease, and release converges on the
                // exact handle. The encoded frame and borrowed listener are
                // reused unchanged.
                match self.inner.transport.exchange(
                    self.protocol_info.publisher_socket(),
                    &frame,
                    listener,
                ) {
                    Ok(reply) => reply,
                    Err(second_failure) => {
                        return Err(ClientError::Transport(exact_replay_failure(
                            first_failure.certainty,
                            second_failure,
                        )));
                    }
                }
            }
            Err(failure) => return Err(ClientError::Transport(failure)),
        };
        if reply.peer_uid != self.inner.expected_uid {
            return Err(ClientError::PeerUidMismatch {
                expected: self.inner.expected_uid,
                actual: reply.peer_uid,
            });
        }
        let response: ResponseEnvelope<R> =
            decode_response_frame(&reply.response_frame).map_err(ClientError::Frame)?;
        if response.protocol_version() != locald_publisher_protocol::PROTOCOL_VERSION {
            return Err(ClientError::ProtocolVersionMismatch(
                response.protocol_version(),
            ));
        }
        // A response is not authority until it is committed by its caller.
        // Serialize that commit with live discovery/epoch activation, then
        // retain the guard in TimedResponse through the caller's complete
        // typestate or lease-state transition.
        let epoch_guard = self
            .inner
            .discovery_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.fence.is_current() {
            return Err(ClientError::DaemonEpochChanged);
        }
        if response.daemon_epoch() != self.protocol_info.daemon_epoch() {
            self.fence.invalidate();
            return Err(ClientError::DaemonEpochChanged);
        }
        let value = match response.into_result() {
            Ok(value) => value,
            Err(error) if error.code() == StableErrorCode::DaemonEpochChanged => {
                self.fence.invalidate();
                return Err(ClientError::DaemonEpochChanged);
            }
            Err(error) => return Err(ClientError::Protocol(error)),
        };
        let timing = if schedule_bearing {
            request_started.map(|request_started| ResponseTiming {
                request_started,
                response_received: self.inner.now(),
            })
        } else {
            None
        };
        Ok(TimedResponse {
            value,
            timing,
            _epoch_guard: epoch_guard,
        })
    }

    const fn command_timeout(&self) -> Duration {
        Duration::from_millis(self.protocol_info.frame_timeout_ms())
            .saturating_mul(SUPERVISOR_TIMEOUT_FRAME_MULTIPLIER)
            .saturating_add(SUPERVISOR_TIMEOUT_MARGIN)
    }

    fn best_effort_release(&self, lease_handle: LeaseHandle) {
        drop(self.exchange::<ReleaseResult>(
            PublisherRequest::Release(ReleaseArguments { lease_handle }),
            None,
            None,
        ));
    }
}

struct TimedResponse<'a, R> {
    value: R,
    timing: Option<ResponseTiming>,
    _epoch_guard: MutexGuard<'a, ()>,
}

#[derive(Clone, Copy)]
struct ResponseTiming {
    request_started: SuspendInstant,
    response_received: Result<SuspendInstant, ClockError>,
}

/// Acquisition attempt plus the daemon-derived origin not yet acknowledged.
pub struct PreparedPublication {
    project: ProjectPublisher,
    service_name: ServiceName,
    attempt_handle: AcquisitionAttemptHandle,
    origin: SemanticOrigin,
    attempt_state: AttemptState,
}

impl fmt::Debug for PreparedPublication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedPublication")
            .field("service_name", &self.service_name)
            .field("origin", &self.origin)
            .field("attempt_state", &self.attempt_state)
            .finish_non_exhaustive()
    }
}

impl PreparedPublication {
    /// Exact origin that the caller must install before acquisition.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }

    /// Server-observed execution state for this exact attempt.
    #[must_use]
    pub const fn attempt_state(&self) -> AttemptState {
        self.attempt_state
    }

    /// Enter the consuming terminal typestate without discarding exact replay
    /// authority. Non-terminal preparation is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns the unchanged preparation when its attempt is not terminal.
    #[allow(
        clippy::result_large_err,
        reason = "the error intentionally returns the unchanged authority typestate"
    )]
    pub fn into_terminal(self) -> Result<TerminalPreparedPublication, Self> {
        if self.attempt_state == AttemptState::Terminal {
            Ok(TerminalPreparedPublication { prepared: self })
        } else {
            Err(self)
        }
    }

    /// Confirm caller-performed installation of the exact daemon-derived origin.
    ///
    /// # Errors
    ///
    /// Returns [`OriginInstallError`] with the unchanged state when the
    /// installed origin differs from daemon authority.
    #[allow(
        clippy::result_large_err,
        reason = "the error intentionally returns the unchanged authority typestate"
    )]
    pub fn confirm_origin_installed(
        self,
        installed_origin: &SemanticOrigin,
    ) -> Result<InstalledOrigin, OriginInstallError<Self>> {
        if installed_origin != &self.origin {
            return Err(OriginInstallError { state: self });
        }
        Ok(InstalledOrigin { prepared: self })
    }
}

/// A terminal acquisition attempt that may be replayed exactly or explicitly
/// replaced without exposing its opaque server handle.
pub struct TerminalPreparedPublication {
    prepared: PreparedPublication,
}

impl fmt::Debug for TerminalPreparedPublication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalPreparedPublication")
            .field("origin", &self.prepared.origin)
            .finish_non_exhaustive()
    }
}

impl TerminalPreparedPublication {
    /// Exact origin installed by the terminal request.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        self.prepared.origin()
    }

    /// Confirm the origin and preserve the terminal handle for exact acquire
    /// replay with the same listener identity.
    ///
    /// # Errors
    ///
    /// Returns [`OriginInstallError`] with the unchanged terminal state when
    /// the installed origin differs from daemon authority.
    #[allow(
        clippy::result_large_err,
        reason = "the error intentionally returns the unchanged authority typestate"
    )]
    pub fn confirm_origin_installed(
        self,
        installed_origin: &SemanticOrigin,
    ) -> Result<InstalledOrigin, OriginInstallError<Self>> {
        if installed_origin != &self.prepared.origin {
            return Err(OriginInstallError { state: self });
        }
        Ok(InstalledOrigin {
            prepared: self.prepared,
        })
    }

    /// Explicitly replace this exact terminal attempt. The consumed handle is
    /// named internally in the compare-and-swap begin request.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the compare-and-swap begin exchange fails.
    pub fn replace(self) -> Result<PreparedPublication, ClientError> {
        let PreparedPublication {
            project,
            service_name,
            attempt_handle,
            ..
        } = self.prepared;
        project.prepare_with_replacement(service_name, Some(attempt_handle))
    }
}

/// Preparation after the caller explicitly installed the exact origin.
pub struct InstalledOrigin {
    prepared: PreparedPublication,
}

impl fmt::Debug for InstalledOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstalledOrigin")
            .field("service_name", &self.prepared.service_name)
            .field("origin", &self.prepared.origin)
            .finish_non_exhaustive()
    }
}

impl InstalledOrigin {
    /// Duplicate and transfer an already-bound listener, then start the
    /// client-owned lease supervisor. This consumes the installed-origin
    /// authority so one attempt cannot be driven by multiple caller paths.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when listener duplication, acquisition,
    /// response validation, timing, wake registration, or supervisor startup
    /// fails.
    pub fn acquire(self, listener: &TcpListener) -> Result<Lease, ClientError> {
        let pending = PendingSupervisor::register(
            &self.prepared.project.inner.session,
            self.prepared.project.inner.wake_monitor.as_ref(),
        )
        .map_err(ClientError::Wake)?;
        let retained_listener = listener
            .try_clone()
            .map_err(|error| ClientError::ListenerClone(error.to_string()))?;
        let observed_listener = retained_listener
            .try_clone()
            .map_err(|error| ClientError::ListenerClone(error.to_string()))?;
        let response = self.prepared.project.exchange::<AcquireResult>(
            PublisherRequest::Acquire(AcquireArguments {
                acquisition_attempt_handle: self.prepared.attempt_handle.clone(),
                acknowledged_origin: self.prepared.origin.clone(),
            }),
            Some(retained_listener.as_fd()),
            None,
        )?;
        let lease_handle = response.value.lease_handle().clone();
        if response.value.origin() != &self.prepared.origin {
            drop(response);
            self.prepared.project.best_effort_release(lease_handle);
            return Err(ClientError::OriginChanged);
        }
        let Some(timing) = response.timing else {
            drop(response);
            self.prepared.project.best_effort_release(lease_handle);
            return Err(ClientError::Clock(ClockError::Unavailable));
        };
        let response_received = match timing.response_received {
            Ok(response_received) => response_received,
            Err(error) => {
                drop(response);
                self.prepared.project.best_effort_release(lease_handle);
                return Err(ClientError::Clock(error));
            }
        };
        let schedule = match RenewalSchedule::from_response(
            timing.request_started,
            response.value.renew_after_ms(),
            response.value.expires_in_ms(),
        ) {
            Ok(schedule) => schedule,
            Err(error) => {
                drop(response);
                self.prepared.project.best_effort_release(lease_handle);
                return Err(ClientError::Clock(error));
            }
        };
        if schedule.expired(response_received) {
            drop(response);
            self.prepared.project.best_effort_release(lease_handle);
            return Err(ClientError::LeaseExpired);
        }
        let shared = Arc::new(SupervisorShared::new(
            LeaseSnapshot::active(
                response.value.binding_revision(),
                response.value.origin().clone(),
                response.value.publication_state(),
            ),
            lease_handle.clone(),
            schedule,
            observed_listener,
        ));
        let driver = LeaseDriver {
            project: self.prepared.project.clone(),
            lease_handle: lease_handle.clone(),
            binding_revision: response.value.binding_revision(),
            origin: response.value.origin().clone(),
            schedule,
            renewal_retry_not_before: None,
            listener: retained_listener,
            shared: Arc::clone(&shared),
        };
        let supervisor = match pending.start(
            self.prepared.project.fence.clone(),
            shared,
            self.prepared.project.command_timeout(),
            driver,
        ) {
            Ok(supervisor) => supervisor,
            Err(error) => {
                // Thread creation failed after acquisition. There is no
                // supervisor to own cleanup, so perform one bounded attempt
                // before reporting the startup failure.
                drop(response);
                self.prepared.project.best_effort_release(lease_handle);
                return Err(ClientError::Wake(error));
            }
        };
        Ok(Lease {
            project: self.prepared.project.clone(),
            service_name: self.prepared.service_name,
            supervisor,
        })
    }
}

/// Origin acknowledgement failed locally before any capability transfer.
pub struct OriginInstallError<S> {
    state: S,
}

impl<S> fmt::Debug for OriginInstallError<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OriginInstallError")
            .finish_non_exhaustive()
    }
}

impl<S> OriginInstallError<S> {
    /// Recover the unchanged pre-acknowledgement state.
    #[must_use]
    pub fn into_state(self) -> S {
        self.state
    }
}

impl<S> fmt::Display for OriginInstallError<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("installed origin does not match the daemon-derived origin")
    }
}

impl<S> std::error::Error for OriginInstallError<S> {}

/// Cloneable handle to one client-supervised publication lease.
///
/// Clones share one supervisor and one release sequence. Dropping the final
/// clone is nonblocking and signals a best-effort exact-handle release.
#[derive(Clone)]
pub struct Lease {
    project: ProjectPublisher,
    service_name: ServiceName,
    supervisor: SupervisorHandle,
}

impl fmt::Debug for Lease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lease")
            .field("service_name", &self.service_name)
            .field("snapshot", &self.snapshot())
            .finish_non_exhaustive()
    }
}

impl Lease {
    /// Observe the latest authenticated, redacted lease state.
    #[must_use]
    pub fn snapshot(&self) -> LeaseSnapshot {
        self.supervisor.snapshot()
    }

    /// Wait for a local snapshot transition without performing publisher IPC.
    #[must_use]
    pub fn wait_for_change(&self, after_sequence: u64, timeout: Duration) -> LeaseSnapshot {
        self.supervisor.wait_for_change(after_sequence, timeout)
    }

    /// Ask the supervisor thread to begin a candidate rebind. Renewal and wake
    /// ordering remain owned by that thread while this synchronous call waits.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when lease authority is inactive or the
    /// serialized begin-rebind exchange fails.
    pub fn prepare_rebind(&self) -> Result<PreparedRebind, ClientError> {
        self.supervisor
            .begin_rebind(None)
            .map_err(|error| self.map_supervisor_error(error))
    }

    fn replace_terminal_rebind(
        &self,
        terminal: TerminalPreparedRebind,
    ) -> Result<PreparedRebind, ClientError> {
        let PreparedRebind {
            lease_handle,
            expected_binding_revision,
            attempt_handle,
            ..
        } = terminal.prepared;
        self.supervisor
            .begin_rebind(Some(RebindReplacementAuthority {
                lease_handle,
                expected_binding_revision,
                attempt_handle,
            }))
            .map_err(|error| self.map_supervisor_error(error))
    }

    /// Consume an origin-confirmed candidate and ask the supervisor to install
    /// its duplicated listener capability.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the listener cannot be duplicated, the
    /// candidate is stale, or the serialized rebind fails.
    pub fn rebind(
        &self,
        candidate: RebindInstalledOrigin,
        listener: &TcpListener,
    ) -> Result<(), ClientError> {
        let retained_listener = listener
            .try_clone()
            .map_err(|error| ClientError::ListenerClone(error.to_string()))?;
        self.supervisor
            .rebind(candidate, retained_listener)
            .map_err(|error| self.map_supervisor_error(error))
    }

    /// Wait for the exact captured binding on a separate transport exchange.
    /// A server-side 30-second wait therefore cannot block renewal, rebind, or
    /// release work on the lease-supervisor thread.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when authority is inactive or uncertain, the
    /// authenticated wait fails, or its response mismatches the captured bind.
    pub fn wait_ready(&self) -> Result<WaitOutcome, ClientError> {
        // A queued wake or due deadline is handled before this barrier, so a
        // stable-origin wait cannot overtake required maintenance. The long
        // readiness exchange itself still runs on the caller's thread.
        self.supervisor
            .synchronize()
            .map_err(|error| self.map_supervisor_error(error))?;
        let Some(authority) = self.supervisor.wait_authority() else {
            return self.current_authority_error();
        };
        let response = self.project.exchange::<WaitReadyResult>(
            PublisherRequest::WaitReady(WaitReadyArguments {
                lease_handle: authority.lease_handle,
                expected_binding_revision: authority.binding_revision,
            }),
            None,
            Some(authority.schedule),
        );
        match response {
            Ok(response) => {
                if response.value.binding_revision != authority.binding_revision
                    || response.value.origin != authority.origin
                {
                    return Err(ClientError::WaitAuthorityMismatch);
                }
                if self.supervisor.ready(authority.binding_revision) {
                    return Ok(WaitOutcome::Ready(response.value));
                }
                match self.supervisor.state() {
                    LeaseState::Active => Ok(WaitOutcome::BindingReplaced),
                    LeaseState::ReacquisitionRequired(reason) => self
                        .build_reacquisition(reason)
                        .map(WaitOutcome::ReacquisitionRequired),
                    LeaseState::AuthorityUncertain => Err(ClientError::AuthorityUncertain),
                    LeaseState::Released => Err(ClientError::LeaseInactive),
                }
            }
            Err(ClientError::Protocol(error)) if error.code() == StableErrorCode::WaitTimedOut => {
                Ok(WaitOutcome::TimedOut)
            }
            Err(ClientError::Protocol(error))
                if error.code() == StableErrorCode::BindingReplaced =>
            {
                Ok(WaitOutcome::BindingReplaced)
            }
            Err(error) => {
                if let Some(reason) = authority_loss_reason(&error) {
                    self.supervisor.invalidate(reason);
                    return match self.supervisor.state() {
                        LeaseState::ReacquisitionRequired(current_reason) => self
                            .build_reacquisition(current_reason)
                            .map(WaitOutcome::ReacquisitionRequired),
                        LeaseState::AuthorityUncertain => Err(ClientError::AuthorityUncertain),
                        LeaseState::Released => Err(ClientError::LeaseInactive),
                        LeaseState::Active => Err(error),
                    };
                }
                Err(error)
            }
        }
    }

    /// Explicitly release this lease. The call is bounded even if a backend
    /// violates its own exchange deadline; lease expiry remains the final
    /// correctness boundary after an ambiguous outcome.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when release cannot converge within the local
    /// bound or another clone already initiated release.
    pub fn release(self) -> Result<(), ClientError> {
        self.supervisor
            .release()
            .map_err(|error| self.map_supervisor_error(error))
    }

    /// Recover the retained inputs needed for a fresh typed acquisition after
    /// an observed authority-loss transition.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the retained listener cannot be cloned.
    pub fn reacquisition(&self) -> Result<Option<Reacquisition>, ClientError> {
        match self.supervisor.state() {
            LeaseState::ReacquisitionRequired(reason) => self.build_reacquisition(reason).map(Some),
            _ => Ok(None),
        }
    }

    fn build_reacquisition(
        &self,
        reason: ReacquisitionReason,
    ) -> Result<Reacquisition, ClientError> {
        let listener = self
            .supervisor
            .clone_listener()
            .map_err(|error| ClientError::ListenerClone(error.to_string()))?;
        Ok(Reacquisition {
            reason,
            project_locator: self.project.project_locator.clone(),
            expected_project_instance_id: self.project.project_instance_id,
            service_name: self.service_name.clone(),
            listener,
        })
    }

    fn current_authority_error<T>(&self) -> Result<T, ClientError> {
        match self.supervisor.state() {
            LeaseState::ReacquisitionRequired(reason) => Err(ClientError::ReacquisitionRequired(
                self.build_reacquisition(reason)?,
            )),
            LeaseState::AuthorityUncertain => Err(ClientError::AuthorityUncertain),
            LeaseState::Released => Err(ClientError::LeaseInactive),
            LeaseState::Active => Err(ClientError::SupervisorStopped),
        }
    }

    fn map_supervisor_error(&self, error: SupervisorCallError) -> ClientError {
        match error {
            SupervisorCallError::Operation(error) => error,
            SupervisorCallError::Reacquisition(reason) => match self.build_reacquisition(reason) {
                Ok(reacquisition) => ClientError::ReacquisitionRequired(reacquisition),
                Err(error) => error,
            },
            SupervisorCallError::Timeout => ClientError::SupervisorTimeout,
            SupervisorCallError::ReleaseAlreadyRequested => ClientError::ReleaseAlreadyRequested,
            SupervisorCallError::Stopped => match self.supervisor.state() {
                LeaseState::ReacquisitionRequired(reason) => {
                    match self.build_reacquisition(reason) {
                        Ok(reacquisition) => ClientError::ReacquisitionRequired(reacquisition),
                        Err(error) => error,
                    }
                }
                LeaseState::AuthorityUncertain => ClientError::AuthorityUncertain,
                LeaseState::Released => ClientError::LeaseInactive,
                LeaseState::Active => ClientError::SupervisorStopped,
            },
        }
    }
}

#[derive(Debug)]
struct LeaseDriver {
    project: ProjectPublisher,
    lease_handle: LeaseHandle,
    binding_revision: BindingRevision,
    origin: SemanticOrigin,
    schedule: RenewalSchedule,
    renewal_retry_not_before: Option<SuspendInstant>,
    listener: TcpListener,
    shared: Arc<SupervisorShared>,
}

impl SupervisorDriver for LeaseDriver {
    fn renewal_wait(&self) -> Result<Duration, ClientError> {
        let now = self.project.inner.now().map_err(ClientError::Clock)?;
        if self.schedule.expired(now) {
            return Err(ClientError::LeaseExpired);
        }
        if let Some(retry_not_before) = self.renewal_retry_not_before {
            return Ok(retry_not_before
                .as_duration()
                .saturating_sub(now.as_duration()));
        }
        self.schedule.renew_in(now).map_err(ClientError::Clock)
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "the response guard must fence live epoch activation through the complete renewal commit"
    )]
    fn renew(&mut self, _cause: RenewalCause) -> Result<(), ClientError> {
        let now = self.project.inner.now().map_err(ClientError::Clock)?;
        if self.schedule.expired(now) {
            return Err(ClientError::LeaseExpired);
        }
        let response = self.project.exchange::<RenewResult>(
            PublisherRequest::Renew(RenewArguments {
                lease_handle: self.lease_handle.clone(),
            }),
            None,
            Some(self.schedule),
        );
        let response = match response {
            Ok(response) => response,
            Err(
                error @ ClientError::Transport(TransportFailure {
                    certainty: DeliveryCertainty::NotSent,
                    ..
                }),
            ) => {
                let now = self.project.inner.now().map_err(ClientError::Clock)?;
                self.renewal_retry_not_before = self.schedule.unsent_retry_not_before(now);
                if self.renewal_retry_not_before.is_none() {
                    return Err(ClientError::LeaseExpired);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let timing = response
            .timing
            .ok_or(ClientError::Clock(ClockError::Unavailable))?;
        let response_received = timing.response_received.map_err(ClientError::Clock)?;
        if response.value.binding_revision() != self.binding_revision {
            return Err(ClientError::BindingRevisionChanged);
        }
        let schedule = RenewalSchedule::from_response(
            timing.request_started,
            response.value.renew_after_ms(),
            response.value.expires_in_ms(),
        )
        .map_err(ClientError::Clock)?;
        if schedule.expired(response_received) {
            return Err(ClientError::LeaseExpired);
        }
        if !self.shared.renew(
            response.value.binding_revision(),
            response.value.publication_state(),
            schedule,
        ) {
            return Err(ClientError::LeaseInactive);
        }
        self.schedule = schedule;
        self.renewal_retry_not_before = None;
        Ok(())
    }

    fn begin_rebind(
        &mut self,
        replacement: Option<RebindReplacementAuthority>,
    ) -> Result<PreparedRebind, ClientError> {
        let replace_terminal_attempt = match replacement {
            Some(replacement) => {
                let same_lease = replacement.lease_handle == self.lease_handle;
                let same_revision = replacement.expected_binding_revision == self.binding_revision;
                if !same_lease || !same_revision {
                    return Err(ClientError::RebindAuthorityMismatch);
                }
                Some(replacement.attempt_handle)
            }
            None => None,
        };
        let response = self.project.exchange::<BeginRebindResult>(
            PublisherRequest::BeginRebind(BeginRebindArguments {
                lease_handle: self.lease_handle.clone(),
                expected_binding_revision: self.binding_revision,
                replace_terminal_attempt_handle: replace_terminal_attempt,
            }),
            None,
            Some(self.schedule),
        )?;
        if response.value.origin() != &self.origin {
            return Err(ClientError::RebindResultMismatch);
        }
        Ok(PreparedRebind {
            lease_handle: self.lease_handle.clone(),
            expected_binding_revision: self.binding_revision,
            attempt_handle: response.value.rebind_attempt_handle().clone(),
            origin: response.value.origin().clone(),
            attempt_state: response.value.attempt_state(),
        })
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "the response guard must fence live epoch activation through the complete rebind commit"
    )]
    fn rebind(
        &mut self,
        candidate: RebindInstalledOrigin,
        listener: TcpListener,
    ) -> Result<(), ClientError> {
        if candidate.prepared.lease_handle != self.lease_handle
            || candidate.prepared.expected_binding_revision != self.binding_revision
        {
            return Err(ClientError::RebindAuthorityMismatch);
        }
        let attempt_handle = candidate.prepared.attempt_handle;
        let candidate_origin = candidate.prepared.origin;
        let observed_listener = listener
            .try_clone()
            .map_err(|error| ClientError::ListenerClone(error.to_string()))?;
        let response = self.project.exchange::<RebindResult>(
            PublisherRequest::Rebind(RebindArguments {
                rebind_attempt_handle: attempt_handle,
                acknowledged_origin: candidate_origin.clone(),
            }),
            Some(listener.as_fd()),
            Some(self.schedule),
        )?;
        let timing = response
            .timing
            .ok_or(ClientError::Clock(ClockError::Unavailable))?;
        let response_received = timing.response_received.map_err(ClientError::Clock)?;
        if response.value.lease_handle() != &self.lease_handle
            || response.value.origin() != &candidate_origin
            || response.value.binding_revision().get() <= self.binding_revision.get()
        {
            return Err(ClientError::RebindResultMismatch);
        }
        let schedule = RenewalSchedule::from_response(
            timing.request_started,
            response.value.renew_after_ms(),
            response.value.expires_in_ms(),
        )
        .map_err(ClientError::Clock)?;
        if schedule.expired(response_received) {
            return Err(ClientError::LeaseExpired);
        }
        if !self.shared.rebind(
            self.lease_handle.clone(),
            response.value.binding_revision(),
            response.value.origin().clone(),
            response.value.publication_state(),
            schedule,
            observed_listener,
        ) {
            return Err(ClientError::LeaseInactive);
        }
        self.binding_revision = response.value.binding_revision();
        self.origin = response.value.origin().clone();
        self.schedule = schedule;
        self.renewal_retry_not_before = None;
        self.listener = listener;
        Ok(())
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "the response guard must fence live epoch activation through definitive release validation"
    )]
    fn release(&mut self) -> Result<(), ClientError> {
        let response = self.project.exchange::<ReleaseResult>(
            PublisherRequest::Release(ReleaseArguments {
                lease_handle: self.lease_handle.clone(),
            }),
            None,
            None,
        );
        let response = match response {
            Ok(response) => response,
            Err(ClientError::Protocol(error)) if error.code() == StableErrorCode::LeaseLost => {
                // An ambiguous first release followed by definitive lease loss
                // proves the exact authority is gone and converges to closure.
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if !response.value.is_released() {
            return Err(ClientError::InvalidReleaseResult);
        }
        Ok(())
    }
}

/// Rebind attempt before candidate-origin installation.
pub struct PreparedRebind {
    lease_handle: LeaseHandle,
    expected_binding_revision: BindingRevision,
    attempt_handle: RebindAttemptHandle,
    origin: SemanticOrigin,
    attempt_state: AttemptState,
}

pub(super) struct RebindReplacementAuthority {
    pub(super) lease_handle: LeaseHandle,
    pub(super) expected_binding_revision: BindingRevision,
    pub(super) attempt_handle: RebindAttemptHandle,
}

impl fmt::Debug for RebindReplacementAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RebindReplacementAuthority")
            .field("expected_binding_revision", &self.expected_binding_revision)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PreparedRebind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedRebind")
            .field("origin", &self.origin)
            .field("attempt_state", &self.attempt_state)
            .finish_non_exhaustive()
    }
}

impl PreparedRebind {
    /// Exact candidate origin that must be installed before capability transfer.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }

    /// Server-observed execution state for this exact rebind attempt.
    #[must_use]
    pub const fn attempt_state(&self) -> AttemptState {
        self.attempt_state
    }

    /// Enter the consuming terminal typestate without discarding exact replay
    /// authority. Non-terminal preparation is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns the unchanged preparation when its attempt is not terminal.
    pub fn into_terminal(self) -> Result<TerminalPreparedRebind, Self> {
        if self.attempt_state == AttemptState::Terminal {
            Ok(TerminalPreparedRebind { prepared: self })
        } else {
            Err(self)
        }
    }

    /// Confirm caller-performed installation of the exact candidate origin.
    ///
    /// # Errors
    ///
    /// Returns [`OriginInstallError`] with the unchanged state when the
    /// installed origin differs from daemon authority.
    pub fn confirm_origin_installed(
        self,
        installed_origin: &SemanticOrigin,
    ) -> Result<RebindInstalledOrigin, OriginInstallError<Self>> {
        if installed_origin != &self.origin {
            return Err(OriginInstallError { state: self });
        }
        Ok(RebindInstalledOrigin { prepared: self })
    }
}

/// A terminal rebind attempt that may be replayed exactly or explicitly
/// replaced through its owning lease supervisor.
pub struct TerminalPreparedRebind {
    prepared: PreparedRebind,
}

impl fmt::Debug for TerminalPreparedRebind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalPreparedRebind")
            .field("origin", &self.prepared.origin)
            .finish_non_exhaustive()
    }
}

impl TerminalPreparedRebind {
    /// Exact candidate origin carried by the terminal attempt.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        self.prepared.origin()
    }

    /// Confirm origin installation and retain this terminal handle for exact
    /// rebind replay with the same listener identity.
    ///
    /// # Errors
    ///
    /// Returns [`OriginInstallError`] with the unchanged terminal state when
    /// the installed origin differs from daemon authority.
    pub fn confirm_origin_installed(
        self,
        installed_origin: &SemanticOrigin,
    ) -> Result<RebindInstalledOrigin, OriginInstallError<Self>> {
        if installed_origin != &self.prepared.origin {
            return Err(OriginInstallError { state: self });
        }
        Ok(RebindInstalledOrigin {
            prepared: self.prepared,
        })
    }

    /// Explicitly replace this exact terminal attempt on its owning lease.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the terminal authority is stale or the
    /// serialized compare-and-swap begin exchange fails.
    pub fn replace(self, lease: &Lease) -> Result<PreparedRebind, ClientError> {
        lease.replace_terminal_rebind(self)
    }
}

/// Rebind attempt whose exact candidate origin was installed by the caller.
pub struct RebindInstalledOrigin {
    prepared: PreparedRebind,
}

impl fmt::Debug for RebindInstalledOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RebindInstalledOrigin")
            .field("origin", &self.prepared.origin)
            .finish_non_exhaustive()
    }
}

/// Authority-scoped readiness result.
#[derive(Debug)]
pub enum WaitOutcome {
    /// Exact binding route authorization was observed.
    Ready(WaitReadyResult),
    /// The observational server-side wait reached its fixed bound.
    TimedOut,
    /// The captured binding was replaced before readiness was observed.
    BindingReplaced,
    /// Current lease authority was lost and fresh installation is required.
    ReacquisitionRequired(Reacquisition),
}

/// Why a live client state requires fresh preparation and origin installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReacquisitionReason {
    /// The daemon restarted and selected a new random epoch.
    DaemonEpochChanged,
    /// This exact lease expired or was retired.
    LeaseLost,
}

/// Retained identity, service, and listener needed for explicit reacquisition.
pub struct Reacquisition {
    /// Exact reason fresh authority is required.
    pub reason: ReacquisitionReason,
    project_locator: AbsolutePath,
    expected_project_instance_id: ProjectInstanceId,
    service_name: ServiceName,
    listener: TcpListener,
}

impl fmt::Debug for Reacquisition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Reacquisition")
            .field("reason", &self.reason)
            .field("project_locator", &self.project_locator)
            .field(
                "expected_project_instance_id",
                &self.expected_project_instance_id,
            )
            .field("service_name", &self.service_name)
            .field("listener", &"<retained listener>")
            .finish()
    }
}

impl Reacquisition {
    /// Resolve the same physical instance afresh and prepare a new attempt.
    /// The listener is returned separately so origin installation remains a
    /// required typed step before acquisition.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when project resolution fails, resolves another
    /// instance, or fresh acquisition preparation fails.
    pub fn prepare(
        self,
        client: &PublisherClient,
        installation: &InstalledPublisher,
    ) -> Result<(PreparedPublication, TcpListener), ClientError> {
        let project = client.for_project(installation, self.project_locator)?;
        if project.project_instance_id != self.expected_project_instance_id {
            return Err(ClientError::ProjectInstanceMismatch {
                expected: self.expected_project_instance_id,
                actual: project.project_instance_id,
            });
        }
        let prepared = project.prepare(self.service_name)?;
        Ok((prepared, self.listener))
    }
}

pub(super) fn authority_loss_reason(error: &ClientError) -> Option<ReacquisitionReason> {
    match error {
        ClientError::DaemonEpochChanged => Some(ReacquisitionReason::DaemonEpochChanged),
        ClientError::Protocol(error) if error.code() == StableErrorCode::DaemonEpochChanged => {
            Some(ReacquisitionReason::DaemonEpochChanged)
        }
        ClientError::Protocol(error) if error.code() == StableErrorCode::LeaseLost => {
            Some(ReacquisitionReason::LeaseLost)
        }
        ClientError::LeaseExpired => Some(ReacquisitionReason::LeaseLost),
        _ => None,
    }
}

pub(super) fn mutation_outcome_is_uncertain(error: &ClientError) -> bool {
    match error {
        ClientError::Transport(failure) => failure.certainty != DeliveryCertainty::NotSent,
        ClientError::Protocol(_)
        | ClientError::ListenerClone(_)
        | ClientError::RebindAuthorityMismatch => false,
        _ => true,
    }
}

fn exact_replay_failure(
    first_certainty: DeliveryCertainty,
    second_failure: TransportFailure,
) -> TransportFailure {
    let certainty = if first_certainty == DeliveryCertainty::OutcomeUnknown
        || second_failure.certainty == DeliveryCertainty::OutcomeUnknown
    {
        DeliveryCertainty::OutcomeUnknown
    } else {
        second_failure.certainty
    };
    TransportFailure::new(certainty, second_failure.error)
}

/// Publisher-client failure that preserves stable protocol and transport errors.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Authenticated discovery failed.
    #[error("publisher backend failed: {0}")]
    Backend(BackendError),
    /// Dedicated publisher transport failed with delivery certainty preserved.
    #[error("publisher transport failed: {0}")]
    Transport(TransportFailure),
    /// Strict frame encoding or decoding failed.
    #[error("publisher frame failed: {0}")]
    Frame(FrameError),
    /// The daemon returned a structured stable error.
    #[error("publisher operation failed: {0}")]
    Protocol(ProtocolError),
    /// Suspend-inclusive scheduling failed closed.
    #[error("publisher scheduling failed: {0}")]
    Clock(ClockError),
    /// Wake observation or supervisor startup failed.
    #[error("publisher wake supervision failed: {0}")]
    Wake(WakeError),
    /// Discovery state was structurally incompatible.
    #[error("publisher discovery is invalid: {0}")]
    InvalidDiscovery(String),
    /// A socket peer had the wrong UID.
    #[error("publisher peer UID {actual} does not match expected UID {expected}")]
    PeerUidMismatch {
        /// Effective UID required by this client session.
        expected: u32,
        /// Kernel-authenticated UID returned by the peer.
        actual: u32,
    },
    /// A response selected another protocol version.
    #[error("publisher response selected unsupported protocol version {0}")]
    ProtocolVersionMismatch(u32),
    /// A response came from another daemon lifetime.
    #[error("publisher daemon epoch changed; reacquisition is required")]
    DaemonEpochChanged,
    /// Preparation resolved another physical instance.
    #[error("publisher project resolved as {actual}, expected {expected}")]
    ProjectInstanceMismatch {
        /// Daemon-observed physical identity retained by the workflow.
        expected: ProjectInstanceId,
        /// Identity returned by the latest project resolution.
        actual: ProjectInstanceId,
    },
    /// Operation/prelude and supplied descriptor disagreed locally.
    #[error("publisher listener descriptor does not match the operation")]
    ListenerDescriptorMismatch,
    /// The listener could not be duplicated without consuming the caller's copy.
    #[error("cannot duplicate publisher listener: {0}")]
    ListenerClone(String),
    /// Acquire returned an origin other than acknowledged authority.
    #[error("publisher origin changed during acquisition")]
    OriginChanged,
    /// Renew unexpectedly changed the binding revision.
    #[error("publisher renewal changed the binding revision")]
    BindingRevisionChanged,
    /// The local lease object no longer carries usable authority.
    #[error("publisher lease is inactive")]
    LeaseInactive,
    /// The conservative client-side lease expiry bound elapsed.
    #[error("publisher lease expired before authority could be refreshed")]
    LeaseExpired,
    /// A rebind candidate does not match the lease's current fences.
    #[error("publisher rebind authority no longer matches the lease")]
    RebindAuthorityMismatch,
    /// A successful rebind-phase response did not describe valid authority.
    #[error("publisher returned invalid rebind authority; lease authority is uncertain")]
    RebindResultMismatch,
    /// Readiness response did not match the exact awaited binding.
    #[error("publisher readiness response did not match awaited authority")]
    WaitAuthorityMismatch,
    /// A successful release response violated the fixed result contract.
    #[error("publisher release returned an invalid success result")]
    InvalidReleaseResult,
    /// A synchronous supervisor command exceeded its local bound.
    #[error("publisher lease supervisor command timed out")]
    SupervisorTimeout,
    /// The supervisor stopped without recording a terminal state.
    #[error("publisher lease supervisor stopped unexpectedly")]
    SupervisorStopped,
    /// A mutation may have committed but its authenticated outcome is unknown.
    #[error("publisher lease authority is uncertain")]
    AuthorityUncertain,
    /// Another clone already initiated the one release sequence.
    #[error("publisher lease release was already requested")]
    ReleaseAlreadyRequested,
    /// A live handle was invalidated and fresh typed acquisition is required.
    #[error("publisher authority was lost; reacquisition is required")]
    ReacquisitionRequired(Reacquisition),
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "scripted protocol fixtures fail immediately when their deterministic script is invalid"
)]
mod tests {
    use std::collections::VecDeque;
    use std::os::fd::{AsRawFd as _, BorrowedFd, RawFd};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender};
    use std::sync::{Condvar, Mutex};
    use std::time::Instant;

    use locald_publisher_protocol::{
        DaemonEpoch, EncodedRequestFrame, PublicationState, ReadyState, ResponseEnvelope,
        decode_request_frame, encode_response_frame,
    };

    use super::*;
    use crate::backend::{AuthenticatedValue, BackendErrorKind, TransportReply};
    use crate::wake::{WakeRegistration, WakeSink};

    const ATTEMPT_A: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const ATTEMPT_B: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBA";
    const LEASE: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCA";

    #[derive(Debug)]
    struct FakeDiscovery {
        uid: u32,
        project_instance_id: ProjectInstanceId,
        protocol_info: Mutex<PublishedEndpointProtocolInfo>,
        blocked_protocol_info: Mutex<Option<(SyncSender<()>, Arc<BlockGate>)>>,
    }

    impl FakeDiscovery {
        fn set_protocol_info(&self, protocol_info: PublishedEndpointProtocolInfo) {
            *self.protocol_info.lock().expect("protocol info") = protocol_info;
        }

        fn block_next_protocol_info(&self) -> (Receiver<()>, Arc<BlockGate>) {
            let (entered, receiver) = mpsc::sync_channel(1);
            let gate = Arc::new(BlockGate::default());
            *self
                .blocked_protocol_info
                .lock()
                .expect("blocked protocol info") = Some((entered, Arc::clone(&gate)));
            (receiver, gate)
        }
    }

    impl AuthenticatedDaemonDiscovery for FakeDiscovery {
        fn protocol_info(
            &self,
            _command_socket: &AbsolutePath,
        ) -> Result<AuthenticatedValue<PublishedEndpointProtocolInfo>, BackendError> {
            let value = self.protocol_info.lock().expect("protocol info").clone();
            let blocked = self
                .blocked_protocol_info
                .lock()
                .expect("blocked protocol info")
                .take();
            if let Some((entered, gate)) = blocked {
                entered.send(()).expect("protocol-info observer");
                gate.wait();
            }
            Ok(AuthenticatedValue {
                peer_uid: self.uid,
                value,
            })
        }

        fn resolve_project(
            &self,
            _command_socket: &AbsolutePath,
            _project_locator: &AbsolutePath,
        ) -> Result<AuthenticatedValue<ProjectInstanceId>, BackendError> {
            Ok(AuthenticatedValue {
                peer_uid: self.uid,
                value: self.project_instance_id,
            })
        }
    }

    #[derive(Debug, Default)]
    struct FakeClock {
        millis: AtomicU64,
        fail_next: AtomicBool,
    }

    impl FakeClock {
        fn set_millis(&self, value: u64) {
            self.millis.store(value, Ordering::SeqCst);
        }

        fn fail_next(&self) {
            self.fail_next.store(true, Ordering::SeqCst);
        }
    }

    impl SuspendAwareClock for FakeClock {
        fn now(&self) -> Result<SuspendInstant, ClockError> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(ClockError::Unavailable);
            }
            Ok(SuspendInstant::from_duration(Duration::from_millis(
                self.millis.load(Ordering::SeqCst),
            )))
        }
    }

    #[derive(Debug, Default)]
    struct FakeRegistration;

    impl WakeRegistration for FakeRegistration {}

    #[derive(Debug, Default)]
    struct FakeWakeMonitor {
        sinks: Mutex<Vec<Arc<dyn WakeSink>>>,
        fail_next_registration: AtomicBool,
    }

    impl FakeWakeMonitor {
        fn resume(&self, index: usize) {
            self.sinks.lock().expect("wake sinks")[index].resumed();
        }

        fn fail(&self, index: usize) {
            self.sinks.lock().expect("wake sinks")[index]
                .failed(WakeError::Failed("scripted wake failure".to_owned()));
        }

        fn fail_next_registration(&self) {
            self.fail_next_registration.store(true, Ordering::SeqCst);
        }
    }

    impl WakeMonitor for FakeWakeMonitor {
        fn register(
            &self,
            sink: Arc<dyn WakeSink>,
        ) -> Result<Box<dyn WakeRegistration>, WakeError> {
            if self.fail_next_registration.swap(false, Ordering::SeqCst) {
                sink.failed(WakeError::Failed(
                    "scripted registration failure".to_owned(),
                ));
            }
            self.sinks.lock().expect("wake sinks").push(sink);
            Ok(Box::new(FakeRegistration))
        }
    }

    #[derive(Debug, Default)]
    struct BlockGate {
        open: Mutex<bool>,
        changed: Condvar,
    }

    impl BlockGate {
        fn wait(&self) {
            let open = self.open.lock().expect("block gate");
            drop(
                self.changed
                    .wait_while(open, |open| !*open)
                    .expect("block gate"),
            );
        }

        fn open(&self) {
            *self.open.lock().expect("block gate") = true;
            self.changed.notify_all();
        }
    }

    enum ScriptAction {
        Reply(Vec<u8>),
        Failure(TransportFailure),
        Block {
            entered: SyncSender<()>,
            gate: Arc<BlockGate>,
            reply: Vec<u8>,
        },
    }

    impl fmt::Debug for ScriptAction {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Reply(bytes) => formatter
                    .debug_tuple("Reply")
                    .field(&format_args!("{} bytes", bytes.len()))
                    .finish(),
                Self::Failure(error) => formatter.debug_tuple("Failure").field(error).finish(),
                Self::Block { reply, .. } => formatter
                    .debug_tuple("Block")
                    .field(&format_args!("{} bytes", reply.len()))
                    .finish(),
            }
        }
    }

    #[derive(Debug)]
    struct ScriptStep {
        operation: &'static str,
        action: ScriptAction,
    }

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        operation: String,
        frame: Vec<u8>,
        listener_fd: Option<RawFd>,
    }

    #[derive(Debug, Default)]
    struct FakeTransport {
        steps: Mutex<VecDeque<ScriptStep>>,
        requests: Mutex<Vec<RecordedRequest>>,
        request_changed: Condvar,
    }

    impl FakeTransport {
        fn push<R: serde::Serialize>(
            &self,
            operation: &'static str,
            response: &ResponseEnvelope<R>,
        ) {
            self.steps
                .lock()
                .expect("script steps")
                .push_back(ScriptStep {
                    operation,
                    action: ScriptAction::Reply(
                        encode_response_frame(response).expect("encode fake response"),
                    ),
                });
        }

        fn push_error(&self, operation: &'static str, failure: TransportFailure) {
            self.steps
                .lock()
                .expect("script steps")
                .push_back(ScriptStep {
                    operation,
                    action: ScriptAction::Failure(failure),
                });
        }

        fn push_blocking<R: serde::Serialize>(
            &self,
            operation: &'static str,
            response: &ResponseEnvelope<R>,
        ) -> (Receiver<()>, Arc<BlockGate>) {
            let (entered, entered_receiver) = mpsc::sync_channel(1);
            let gate = Arc::new(BlockGate::default());
            self.steps
                .lock()
                .expect("script steps")
                .push_back(ScriptStep {
                    operation,
                    action: ScriptAction::Block {
                        entered,
                        gate: Arc::clone(&gate),
                        reply: encode_response_frame(response).expect("encode blocking response"),
                    },
                });
            (entered_receiver, gate)
        }

        #[allow(
            clippy::significant_drop_tightening,
            reason = "the mutex guard is the condition-variable predicate and must enter wait_timeout"
        )]
        fn wait_for_requests(&self, operation: &str, expected: usize) {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let requests = self.requests.lock().expect("recorded requests");
                let count = requests
                    .iter()
                    .filter(|request| request.operation == operation)
                    .count();
                if count >= expected {
                    drop(requests);
                    return;
                }
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .expect("timed out waiting for fake request");
                let (requests, result) = self
                    .request_changed
                    .wait_timeout(requests, remaining)
                    .expect("recorded requests");
                drop(requests);
                assert!(!result.timed_out(), "timed out waiting for {operation}");
            }
        }

        fn frames(&self, operation: &str) -> Vec<Vec<u8>> {
            self.requests
                .lock()
                .expect("recorded requests")
                .iter()
                .filter(|request| request.operation == operation)
                .map(|request| request.frame.clone())
                .collect()
        }

        fn recorded(&self, operation: &str) -> Vec<RecordedRequest> {
            self.requests
                .lock()
                .expect("recorded requests")
                .iter()
                .filter(|request| request.operation == operation)
                .cloned()
                .collect()
        }
    }

    impl PublisherTransport for FakeTransport {
        fn exchange(
            &self,
            _publisher_socket: &AbsolutePath,
            request: &EncodedRequestFrame,
            listener: Option<BorrowedFd<'_>>,
        ) -> Result<TransportReply, TransportFailure> {
            let decoded = decode_request_frame(request.as_bytes()).expect("decode fake request");
            let operation = decoded.request().operation();
            assert_eq!(
                request.descriptor() == DescriptorPrelude::Listener,
                listener.is_some()
            );
            self.requests
                .lock()
                .expect("recorded requests")
                .push(RecordedRequest {
                    operation: operation.to_owned(),
                    frame: request.as_bytes().to_vec(),
                    listener_fd: listener.map(|listener| listener.as_raw_fd()),
                });
            self.request_changed.notify_all();
            let step = self
                .steps
                .lock()
                .expect("script steps")
                .pop_front()
                .unwrap_or_else(|| panic!("no fake response for {operation}"));
            assert_eq!(step.operation, operation);
            let response_frame = match step.action {
                ScriptAction::Reply(reply) => reply,
                ScriptAction::Failure(error) => return Err(error),
                ScriptAction::Block {
                    entered,
                    gate,
                    reply,
                } => {
                    entered.send(()).expect("blocking request observer");
                    gate.wait();
                    reply
                }
            };
            Ok(TransportReply {
                peer_uid: Uid::effective().as_raw(),
                response_frame,
            })
        }
    }

    struct Fixture {
        client: PublisherClient,
        installed: InstalledPublisher,
        transport: Arc<FakeTransport>,
        clock: Arc<FakeClock>,
        wakes: Arc<FakeWakeMonitor>,
        discovery: Arc<FakeDiscovery>,
        instance: ProjectInstanceId,
        epoch: DaemonEpoch,
    }

    fn protocol_info(epoch: DaemonEpoch) -> PublishedEndpointProtocolInfo {
        PublishedEndpointProtocolInfo::v1(
            epoch,
            AbsolutePath::parse("/tmp/locald/publisher-v1.sock").expect("publisher socket"),
        )
    }

    fn fixture() -> Fixture {
        let uid = Uid::effective().as_raw();
        let instance =
            ProjectInstanceId::parse("550e8400-e29b-41d4-a716-446655440000").expect("instance");
        let epoch = DaemonEpoch::from_bytes([7; 16]);
        let protocol_info = protocol_info(epoch.clone());
        let installed = InstalledPublisher::from_verified(
            locald_publisher_protocol::InstallationRecord::v1().expect("record"),
            uid,
            protocol_info.clone(),
        );
        let discovery = Arc::new(FakeDiscovery {
            uid,
            project_instance_id: instance,
            protocol_info: Mutex::new(protocol_info),
            blocked_protocol_info: Mutex::new(None),
        });
        let transport = Arc::new(FakeTransport::default());
        let clock = Arc::new(FakeClock::default());
        let wakes = Arc::new(FakeWakeMonitor::default());
        let client = PublisherClient::with_wake_monitor(
            Arc::clone(&discovery) as Arc<dyn AuthenticatedDaemonDiscovery>,
            Arc::clone(&transport) as Arc<dyn PublisherTransport>,
            Arc::clone(&clock) as Arc<dyn SuspendAwareClock>,
            Arc::clone(&wakes) as Arc<dyn WakeMonitor>,
        );
        Fixture {
            client,
            installed,
            transport,
            clock,
            wakes,
            discovery,
            instance,
            epoch,
        }
    }

    fn origin() -> SemanticOrigin {
        SemanticOrigin::parse("https://workbench.example.localhost").expect("origin")
    }

    fn other_origin() -> SemanticOrigin {
        SemanticOrigin::parse("https://other.example.localhost").expect("other origin")
    }

    fn begin_result(
        fixture: &Fixture,
        attempt: &str,
        state: AttemptState,
    ) -> BeginAcquisitionResult {
        BeginAcquisitionResult::new(
            AcquisitionAttemptHandle::parse(attempt).expect("acquisition attempt"),
            fixture.instance,
            origin(),
            15_000,
            state,
        )
        .expect("valid begin result")
    }

    fn acquire_result(renew_after_ms: u64) -> AcquireResult {
        AcquireResult::new(
            LeaseHandle::parse(LEASE).expect("lease"),
            BindingRevision::new(1).expect("revision"),
            origin(),
            renew_after_ms,
            30_000,
            PublicationState::CheckingEndpoint,
        )
        .expect("valid acquire result")
    }

    fn renew_result(state: PublicationState) -> RenewResult {
        renew_result_after(10_000, state)
    }

    fn renew_result_after(renew_after_ms: u64, state: PublicationState) -> RenewResult {
        RenewResult::new(
            BindingRevision::new(1).expect("revision"),
            renew_after_ms,
            30_000,
            state,
        )
        .expect("valid renew result")
    }

    fn begin_rebind_result(state: AttemptState, handle: &str) -> BeginRebindResult {
        BeginRebindResult::new(
            RebindAttemptHandle::parse(handle).expect("rebind attempt"),
            origin(),
            15_000,
            state,
        )
        .expect("valid begin rebind")
    }

    fn rebind_result() -> RebindResult {
        RebindResult::new(
            LeaseHandle::parse(LEASE).expect("lease"),
            BindingRevision::new(2).expect("revision"),
            origin(),
            10_000,
            30_000,
            PublicationState::CheckingEndpoint,
        )
        .expect("valid rebind result")
    }

    fn project(fixture: &Fixture) -> ProjectPublisher {
        fixture
            .client
            .for_project(
                &fixture.installed,
                AbsolutePath::parse("/work/project").expect("locator"),
            )
            .expect("project")
    }

    #[test]
    fn stale_installation_snapshot_cannot_roll_back_the_active_daemon_epoch() {
        let fixture = fixture();
        let stale_installation = fixture.installed.clone();
        let current_epoch = DaemonEpoch::from_bytes([9; 16]);
        let current_protocol_info = protocol_info(current_epoch.clone());
        fixture
            .discovery
            .set_protocol_info(current_protocol_info.clone());
        let current_installation = InstalledPublisher::from_verified(
            stale_installation.record().clone(),
            stale_installation.daemon_uid(),
            current_protocol_info,
        );

        let current_project = fixture
            .client
            .for_project(
                &current_installation,
                AbsolutePath::parse("/work/project").expect("locator"),
            )
            .expect("current project");
        assert!(current_project.fence.is_current());

        let refreshed_from_stale = fixture
            .client
            .for_project(
                &stale_installation,
                AbsolutePath::parse("/work/project").expect("locator"),
            )
            .expect("stale snapshot refreshed");
        assert_eq!(
            refreshed_from_stale.protocol_info.daemon_epoch(),
            &current_epoch
        );
        assert!(current_project.fence.is_current());
        assert!(refreshed_from_stale.fence.is_current());
    }

    #[test]
    fn concurrent_discovery_cannot_reactivate_an_older_epoch_after_a_newer_observation() {
        let fixture = fixture();
        let stale_installation = fixture.installed.clone();
        let (stale_entered, stale_gate) = fixture.discovery.block_next_protocol_info();
        let stale_client = fixture.client.clone();
        let stale_worker = std::thread::spawn(move || {
            stale_client.for_project(
                &stale_installation,
                AbsolutePath::parse("/work/project").expect("locator"),
            )
        });
        stale_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("stale discovery entered");

        let current_epoch = DaemonEpoch::from_bytes([9; 16]);
        let current_protocol_info = protocol_info(current_epoch.clone());
        fixture
            .discovery
            .set_protocol_info(current_protocol_info.clone());
        let current_installation = InstalledPublisher::from_verified(
            fixture.installed.record().clone(),
            fixture.installed.daemon_uid(),
            current_protocol_info,
        );
        let current_client = fixture.client;
        let (current_result, current_receiver) = mpsc::sync_channel(1);
        let current_worker = std::thread::spawn(move || {
            current_result
                .send(current_client.for_project(
                    &current_installation,
                    AbsolutePath::parse("/work/project").expect("locator"),
                ))
                .expect("current result observer");
        });
        assert!(
            current_receiver
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "newer discovery must wait until the older observation is activated"
        );

        stale_gate.open();
        let stale_project = stale_worker
            .join()
            .expect("stale discovery worker")
            .expect("stale project");
        let current_project = current_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("current discovery result")
            .expect("current project");
        current_worker.join().expect("current discovery worker");

        assert!(!stale_project.fence.is_current());
        assert!(current_project.fence.is_current());
        assert_eq!(current_project.protocol_info.daemon_epoch(), &current_epoch);
    }

    #[test]
    fn delayed_old_epoch_acquisition_cannot_commit_after_new_epoch_discovery() {
        let fixture = fixture();
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        let prepared = project(&fixture)
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("prepare");
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let (acquire_entered, acquire_gate) = fixture.transport.push_blocking(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), acquire_result(10_000)),
        );
        let acquire_worker = std::thread::spawn(move || installed.acquire(&listener));
        acquire_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("old acquisition entered");

        let current_epoch = DaemonEpoch::from_bytes([9; 16]);
        let current_protocol_info = protocol_info(current_epoch);
        fixture
            .discovery
            .set_protocol_info(current_protocol_info.clone());
        let current_installation = InstalledPublisher::from_verified(
            fixture.installed.record().clone(),
            fixture.installed.daemon_uid(),
            current_protocol_info,
        );
        let current_project = fixture
            .client
            .for_project(
                &current_installation,
                AbsolutePath::parse("/work/project").expect("locator"),
            )
            .expect("current project");
        assert!(current_project.fence.is_current());

        acquire_gate.open();
        assert!(matches!(
            acquire_worker.join().expect("acquire worker"),
            Err(ClientError::DaemonEpochChanged)
        ));
        assert!(current_project.fence.is_current());
    }

    fn enqueue_acquisition(fixture: &Fixture, renew_after_ms: u64) {
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        fixture.transport.push(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), acquire_result(renew_after_ms)),
        );
    }

    fn acquire_lease(fixture: &Fixture, service: &str) -> Lease {
        let prepared = project(fixture)
            .prepare(ServiceName::parse(service).expect("service"))
            .expect("prepare");
        assert_eq!(prepared.attempt_state(), AttemptState::Pending);
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        installed.acquire(&listener).expect("acquire")
    }

    fn transport_failure(certainty: DeliveryCertainty) -> TransportFailure {
        TransportFailure::new(
            certainty,
            BackendError::new(BackendErrorKind::Io, "scripted transport failure"),
        )
    }

    fn release_success(fixture: &Fixture) {
        fixture.transport.push(
            "release",
            &ResponseEnvelope::success(fixture.epoch.clone(), ReleaseResult::released()),
        );
    }

    #[test]
    fn due_renewal_is_owned_by_the_supervisor() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 0);
        fixture.transport.push(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::Ready),
            ),
        );
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.wait_for_requests("renew", 1);
        let snapshot = lease.wait_for_change(0, Duration::from_secs(2));
        assert_eq!(snapshot.publication_state(), PublicationState::Ready);
        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn delayed_acquire_response_consumes_margin_and_renews_immediately() {
        let fixture = fixture();
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        let prepared = project(&fixture)
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("prepare");
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let (acquire_entered, acquire_gate) = fixture.transport.push_blocking(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), acquire_result(10_000)),
        );
        fixture.transport.push(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::Ready),
            ),
        );
        let acquirer = std::thread::spawn(move || installed.acquire(&listener));
        acquire_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("acquire exchange entered");
        fixture.clock.set_millis(10_001);
        acquire_gate.open();
        let lease = acquirer.join().expect("acquirer thread").expect("acquire");
        fixture.transport.wait_for_requests("renew", 1);
        assert_eq!(
            lease
                .wait_for_change(0, Duration::from_secs(2))
                .publication_state(),
            PublicationState::Ready
        );
        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn acquire_response_after_local_expiry_is_released_without_a_supervisor() {
        let fixture = fixture();
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        let prepared = project(&fixture)
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("prepare");
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let (acquire_entered, acquire_gate) = fixture.transport.push_blocking(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), acquire_result(10_000)),
        );
        release_success(&fixture);
        let acquirer = std::thread::spawn(move || installed.acquire(&listener));
        acquire_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("acquire exchange entered");
        fixture.clock.set_millis(30_001);
        acquire_gate.open();
        assert!(matches!(
            acquirer.join().expect("acquirer thread"),
            Err(ClientError::LeaseExpired)
        ));
        assert_eq!(fixture.transport.frames("renew").len(), 0);
        assert_eq!(fixture.transport.frames("release").len(), 1);
    }

    #[test]
    fn invalid_acquire_success_is_released_without_a_supervisor() {
        let fixture = fixture();
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        let prepared = project(&fixture)
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("prepare");
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        let mismatched = AcquireResult::new(
            LeaseHandle::parse(LEASE).expect("lease"),
            BindingRevision::new(1).expect("revision"),
            SemanticOrigin::parse("https://other.example.localhost").expect("other origin"),
            10_000,
            30_000,
            PublicationState::CheckingEndpoint,
        )
        .expect("valid mismatched acquire result");
        fixture.transport.push(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), mismatched),
        );
        release_success(&fixture);
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        assert!(matches!(
            installed.acquire(&listener),
            Err(ClientError::OriginChanged)
        ));
        assert_eq!(fixture.transport.frames("release").len(), 1);
    }

    #[test]
    fn wake_failure_during_acquire_startup_releases_the_committed_lease() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        release_success(&fixture);
        let prepared = project(&fixture)
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("prepare");
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        fixture.wakes.fail_next_registration();
        assert!(matches!(
            installed.acquire(&listener),
            Err(ClientError::Wake(WakeError::Failed(_)))
        ));
        assert_eq!(fixture.transport.frames("release").len(), 1);
    }

    #[test]
    fn post_acquire_clock_failure_still_releases_the_committed_lease() {
        let fixture = fixture();
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        let prepared = project(&fixture)
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("prepare");
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let (acquire_entered, acquire_gate) = fixture.transport.push_blocking(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), acquire_result(10_000)),
        );
        release_success(&fixture);
        let acquirer = std::thread::spawn(move || installed.acquire(&listener));
        acquire_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("acquire exchange entered");
        fixture.clock.fail_next();
        acquire_gate.open();
        assert!(matches!(
            acquirer.join().expect("acquirer thread"),
            Err(ClientError::Clock(ClockError::Unavailable))
        ));
        assert_eq!(fixture.transport.frames("release").len(), 1);
    }

    #[test]
    fn wake_triggers_immediate_supervisor_renewal() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.push(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::EndpointUnhealthy),
            ),
        );
        fixture.wakes.resume(0);
        fixture.transport.wait_for_requests("renew", 1);
        let snapshot = lease.wait_for_change(0, Duration::from_secs(2));
        assert_eq!(
            snapshot.publication_state(),
            PublicationState::EndpointUnhealthy
        );
        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn definitively_unsent_renewal_stays_active_and_retries_before_expiry() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture
            .transport
            .push_error("renew", transport_failure(DeliveryCertainty::NotSent));
        fixture
            .transport
            .push_error("renew", transport_failure(DeliveryCertainty::NotSent));

        fixture.wakes.resume(0);
        fixture.transport.wait_for_requests("renew", 2);
        lease.supervisor.synchronize().expect("renewal settled");
        assert_eq!(lease.snapshot().state(), LeaseState::Active);

        fixture.transport.push(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::Ready),
            ),
        );
        fixture.clock.set_millis(1_000);
        lease.supervisor.synchronize().expect("scheduled retry");

        let renew_frames = fixture.transport.frames("renew");
        assert_eq!(renew_frames.len(), 3);
        assert_eq!(renew_frames[0], renew_frames[1]);
        assert_eq!(renew_frames[1], renew_frames[2]);
        assert_eq!(lease.snapshot().state(), LeaseState::Active);
        assert_eq!(
            lease.snapshot().publication_state(),
            PublicationState::Ready
        );
        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn definitively_unsent_renewal_keeps_only_the_original_expiry() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture
            .transport
            .push_error("renew", transport_failure(DeliveryCertainty::NotSent));
        fixture
            .transport
            .push_error("renew", transport_failure(DeliveryCertainty::NotSent));

        fixture.wakes.resume(0);
        fixture.transport.wait_for_requests("renew", 2);
        lease.supervisor.synchronize().expect("renewal settled");
        assert_eq!(lease.snapshot().state(), LeaseState::Active);

        fixture.clock.set_millis(30_001);
        fixture.wakes.resume(0);
        assert_eq!(
            lease.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::ReacquisitionRequired(ReacquisitionReason::LeaseLost)
        );
        assert_eq!(fixture.transport.frames("renew").len(), 2);
    }

    #[test]
    fn ambiguous_renewal_still_makes_authority_uncertain() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.push_error(
            "renew",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        fixture.transport.push_error(
            "renew",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );

        fixture.wakes.resume(0);
        assert_eq!(
            lease.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::AuthorityUncertain
        );
        assert_eq!(fixture.transport.frames("renew").len(), 2);
    }

    #[test]
    fn wake_monitor_failure_makes_authority_uncertain_and_stops_operations() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.wakes.fail(0);
        assert_eq!(
            lease.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::AuthorityUncertain
        );
        assert!(matches!(
            lease.prepare_rebind(),
            Err(ClientError::AuthorityUncertain)
        ));
        assert!(fixture.transport.frames("begin_rebind").is_empty());
    }

    #[test]
    fn wake_after_local_expiry_requires_reacquisition_without_renewing() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.clock.set_millis(30_001);
        fixture.wakes.resume(0);
        assert_eq!(
            lease.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::ReacquisitionRequired(ReacquisitionReason::LeaseLost)
        );
        assert!(fixture.transport.frames("renew").is_empty());
    }

    #[test]
    fn wake_renewal_precedes_a_new_readiness_wait() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.push(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::CheckingEndpoint),
            ),
        );
        fixture.transport.push(
            "wait_ready",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                WaitReadyResult {
                    binding_revision: BindingRevision::new(1).expect("revision"),
                    origin: origin(),
                    publication_state: ReadyState::Ready,
                },
            ),
        );
        fixture.wakes.resume(0);
        assert!(matches!(
            lease.wait_ready().expect("wait after wake"),
            WaitOutcome::Ready(_)
        ));
        let operations = fixture
            .transport
            .requests
            .lock()
            .expect("recorded requests")
            .iter()
            .filter(|request| request.operation == "renew" || request.operation == "wait_ready")
            .map(|request| request.operation.clone())
            .collect::<Vec<_>>();
        assert_eq!(operations, ["renew".to_owned(), "wait_ready".to_owned()]);
        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn acquire_renew_and_rebind_replay_byte_identical_ambiguous_requests() {
        let fixture = fixture();
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        fixture.transport.push_error(
            "acquire",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        fixture.transport.push(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), acquire_result(10_000)),
        );
        let lease = acquire_lease(&fixture, "workbench");
        let acquire_frames = fixture.transport.frames("acquire");
        assert_eq!(acquire_frames.len(), 2);
        assert_eq!(acquire_frames[0], acquire_frames[1]);
        let acquire_requests = fixture.transport.recorded("acquire");
        assert_eq!(
            acquire_requests[0].listener_fd,
            acquire_requests[1].listener_fd
        );
        assert!(acquire_requests[0].listener_fd.is_some());

        fixture.transport.push_error(
            "renew",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        fixture.transport.push(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::Ready),
            ),
        );
        fixture.wakes.resume(0);
        fixture.transport.wait_for_requests("renew", 2);
        let renew_frames = fixture.transport.frames("renew");
        assert_eq!(renew_frames[0], renew_frames[1]);

        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Pending, ATTEMPT_B),
            ),
        );
        let candidate = lease
            .prepare_rebind()
            .expect("prepare rebind")
            .confirm_origin_installed(&origin())
            .expect("candidate origin");
        fixture.transport.push_error(
            "rebind",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        fixture.transport.push(
            "rebind",
            &ResponseEnvelope::success(fixture.epoch.clone(), rebind_result()),
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("candidate listener");
        lease.rebind(candidate, &listener).expect("rebind");
        let rebind_frames = fixture.transport.frames("rebind");
        assert_eq!(rebind_frames.len(), 2);
        assert_eq!(rebind_frames[0], rebind_frames[1]);
        let rebind_requests = fixture.transport.recorded("rebind");
        assert_eq!(
            rebind_requests[0].listener_fd,
            rebind_requests[1].listener_fd
        );
        assert!(rebind_requests[0].listener_fd.is_some());

        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn exact_replay_preserves_an_ambiguous_first_delivery() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Pending, ATTEMPT_B),
            ),
        );
        let candidate = lease
            .prepare_rebind()
            .expect("prepare rebind")
            .confirm_origin_installed(&origin())
            .expect("candidate origin");
        fixture.transport.push_error(
            "rebind",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        fixture
            .transport
            .push_error("rebind", transport_failure(DeliveryCertainty::NotSent));
        let listener = TcpListener::bind("127.0.0.1:0").expect("candidate listener");
        assert!(matches!(
            lease.rebind(candidate, &listener),
            Err(ClientError::Transport(TransportFailure {
                certainty: DeliveryCertainty::OutcomeUnknown,
                ..
            }))
        ));
        assert_eq!(
            lease.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::AuthorityUncertain
        );
        let rebind_frames = fixture.transport.frames("rebind");
        assert_eq!(rebind_frames.len(), 2);
        assert_eq!(rebind_frames[0], rebind_frames[1]);
    }

    #[test]
    fn invalid_rebind_success_makes_authority_uncertain() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Pending, ATTEMPT_B),
            ),
        );
        let candidate = lease
            .prepare_rebind()
            .expect("prepare rebind")
            .confirm_origin_installed(&origin())
            .expect("candidate origin");
        let invalid = RebindResult::new(
            LeaseHandle::parse(LEASE).expect("lease"),
            BindingRevision::new(2).expect("revision"),
            other_origin(),
            10_000,
            30_000,
            PublicationState::CheckingEndpoint,
        )
        .expect("valid invalid successor");
        fixture.transport.push(
            "rebind",
            &ResponseEnvelope::success(fixture.epoch.clone(), invalid),
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("candidate listener");
        assert!(matches!(
            lease.rebind(candidate, &listener),
            Err(ClientError::RebindResultMismatch)
        ));
        assert_eq!(
            lease.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::AuthorityUncertain
        );
    }

    #[test]
    fn begin_rebind_origin_mismatch_makes_authority_uncertain() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        let mismatched = BeginRebindResult::new(
            RebindAttemptHandle::parse(ATTEMPT_B).expect("rebind attempt"),
            other_origin(),
            15_000,
            AttemptState::Pending,
        )
        .expect("valid mismatched begin rebind");
        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(fixture.epoch.clone(), mismatched),
        );

        assert!(matches!(
            lease.prepare_rebind(),
            Err(ClientError::RebindResultMismatch)
        ));
        assert_eq!(lease.snapshot().state(), LeaseState::AuthorityUncertain);
    }

    #[test]
    fn started_rebind_timeout_fails_closed_and_late_success_cannot_restore_active() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Pending, ATTEMPT_B),
            ),
        );
        let candidate = lease
            .prepare_rebind()
            .expect("prepare rebind")
            .confirm_origin_installed(&origin())
            .expect("candidate origin");
        let (entered, gate) = fixture.transport.push_blocking(
            "rebind",
            &ResponseEnvelope::success(fixture.epoch.clone(), rebind_result()),
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("candidate listener");
        let mut timed_lease = lease.clone();
        timed_lease
            .supervisor
            .set_command_timeout(Duration::from_millis(500));
        let worker = std::thread::spawn(move || timed_lease.rebind(candidate, &listener));
        entered
            .recv_timeout(Duration::from_secs(2))
            .expect("rebind exchange entered");

        assert!(matches!(
            worker.join().expect("rebind caller"),
            Err(ClientError::SupervisorTimeout)
        ));
        assert_eq!(lease.snapshot().state(), LeaseState::AuthorityUncertain);
        gate.open();
        lease
            .supervisor
            .join_for_test()
            .expect("supervisor completion");
        assert_eq!(lease.snapshot().state(), LeaseState::AuthorityUncertain);
    }

    #[test]
    fn timed_out_rebind_queued_behind_renewal_is_abandoned() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Pending, ATTEMPT_B),
            ),
        );
        let candidate = lease
            .prepare_rebind()
            .expect("prepare rebind")
            .confirm_origin_installed(&origin())
            .expect("candidate origin");
        let (renew_entered, renew_gate) = fixture.transport.push_blocking(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::CheckingEndpoint),
            ),
        );
        fixture.wakes.resume(0);
        renew_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("renew exchange entered");

        let mut timed_lease = lease.clone();
        timed_lease.supervisor.set_command_timeout(Duration::ZERO);
        let listener = TcpListener::bind("127.0.0.1:0").expect("candidate listener");
        assert!(matches!(
            timed_lease.rebind(candidate, &listener),
            Err(ClientError::SupervisorTimeout)
        ));
        assert_eq!(lease.snapshot().state(), LeaseState::AuthorityUncertain);

        renew_gate.open();
        lease
            .supervisor
            .join_for_test()
            .expect("supervisor completion");
        assert!(fixture.transport.frames("rebind").is_empty());
        assert_eq!(lease.snapshot().state(), LeaseState::AuthorityUncertain);
    }

    #[test]
    fn begin_rebind_timeout_does_not_change_lease_authority() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        let (entered, gate) = fixture.transport.push_blocking(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Pending, ATTEMPT_B),
            ),
        );
        let mut timed_lease = lease.clone();
        timed_lease
            .supervisor
            .set_command_timeout(Duration::from_millis(500));
        let worker = std::thread::spawn(move || timed_lease.prepare_rebind());
        entered
            .recv_timeout(Duration::from_secs(2))
            .expect("begin rebind exchange entered");

        assert!(matches!(
            worker.join().expect("begin rebind caller"),
            Err(ClientError::SupervisorTimeout)
        ));
        assert_eq!(lease.snapshot().state(), LeaseState::Active);

        gate.open();
        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Pending, ATTEMPT_B),
            ),
        );
        let prepared = lease.prepare_rebind().expect("retry begin rebind");
        assert_eq!(prepared.origin(), &origin());
        assert_eq!(lease.snapshot().state(), LeaseState::Active);
        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn epoch_change_invalidates_every_lease_in_the_client_session() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let first = acquire_lease(&fixture, "workbench-a");
        enqueue_acquisition(&fixture, 10_000);
        let second = acquire_lease(&fixture, "workbench-b");
        let changed_epoch = DaemonEpoch::from_bytes([9; 16]);
        fixture.transport.push(
            "wait_ready",
            &ResponseEnvelope::success(
                changed_epoch,
                WaitReadyResult {
                    binding_revision: BindingRevision::new(1).expect("revision"),
                    origin: origin(),
                    publication_state: ReadyState::Ready,
                },
            ),
        );
        let WaitOutcome::ReacquisitionRequired(reacquisition) =
            first.wait_ready().expect("typed epoch loss")
        else {
            panic!("expected epoch reacquisition");
        };
        assert_eq!(
            reacquisition.reason,
            ReacquisitionReason::DaemonEpochChanged
        );
        assert_eq!(
            second.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::ReacquisitionRequired(ReacquisitionReason::DaemonEpochChanged)
        );
    }

    #[test]
    fn clock_regression_fails_every_lease_in_the_client_session_closed() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let first = acquire_lease(&fixture, "workbench-a");
        enqueue_acquisition(&fixture, 10_000);
        let second = acquire_lease(&fixture, "workbench-b");

        fixture.clock.set_millis(100);
        fixture
            .client
            .inner
            .now()
            .expect("establish later clock observation");

        fixture.clock.set_millis(50);
        assert!(matches!(
            first.wait_ready(),
            Err(ClientError::AuthorityUncertain)
        ));
        assert_eq!(
            second.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::AuthorityUncertain
        );
        assert!(fixture.transport.frames("wait_ready").is_empty());
    }

    #[test]
    fn lease_lost_is_local_to_the_affected_supervisor() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let first = acquire_lease(&fixture, "workbench-a");
        enqueue_acquisition(&fixture, 10_000);
        let second = acquire_lease(&fixture, "workbench-b");
        fixture.transport.push(
            "renew",
            &ResponseEnvelope::<RenewResult>::error(
                fixture.epoch.clone(),
                ProtocolError::new(StableErrorCode::LeaseLost, "expired", None),
            ),
        );
        fixture.wakes.resume(0);
        assert_eq!(
            first.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::ReacquisitionRequired(ReacquisitionReason::LeaseLost)
        );
        assert_eq!(second.snapshot().state(), LeaseState::Active);
        release_success(&fixture);
        second.release().expect("release unaffected lease");
    }

    #[test]
    fn blocked_wait_ready_does_not_starve_wake_renewal() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        let (wait_entered, wait_gate) = fixture.transport.push_blocking(
            "wait_ready",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                WaitReadyResult {
                    binding_revision: BindingRevision::new(1).expect("revision"),
                    origin: origin(),
                    publication_state: ReadyState::Ready,
                },
            ),
        );
        let waiting_lease = lease.clone();
        let waiter = std::thread::spawn(move || waiting_lease.wait_ready());
        wait_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("wait exchange entered");
        fixture.transport.push(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result(PublicationState::EndpointUnhealthy),
            ),
        );
        fixture.wakes.resume(0);
        fixture.transport.wait_for_requests("renew", 1);
        assert_eq!(
            lease
                .wait_for_change(0, Duration::from_secs(2))
                .publication_state(),
            PublicationState::EndpointUnhealthy
        );
        wait_gate.open();
        assert!(matches!(
            waiter.join().expect("waiter thread").expect("wait result"),
            WaitOutcome::Ready(_)
        ));
        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn completed_release_is_not_overwritten_by_an_in_flight_wait() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        let observer = lease.clone();
        let (wait_entered, wait_gate) = fixture.transport.push_blocking(
            "wait_ready",
            &ResponseEnvelope::<WaitReadyResult>::error(
                fixture.epoch.clone(),
                ProtocolError::new(StableErrorCode::LeaseLost, "released lease", None),
            ),
        );
        let waiting_lease = lease.clone();
        let waiter = std::thread::spawn(move || waiting_lease.wait_ready());
        wait_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("wait exchange entered");

        release_success(&fixture);
        lease.release().expect("release");
        assert_eq!(observer.snapshot().state(), LeaseState::Released);
        wait_gate.open();
        assert!(matches!(
            waiter.join().expect("waiter thread"),
            Err(ClientError::LeaseInactive)
        ));
        assert_eq!(observer.snapshot().state(), LeaseState::Released);
        drop(observer);
        assert_eq!(fixture.transport.frames("release").len(), 1);
    }

    #[test]
    fn final_drop_is_nonblocking_and_starts_one_release_sequence() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        let (release_entered, release_gate) = fixture.transport.push_blocking(
            "release",
            &ResponseEnvelope::success(fixture.epoch.clone(), ReleaseResult::released()),
        );
        let before = Instant::now();
        drop(lease);
        assert!(before.elapsed() < Duration::from_millis(100));
        release_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("best-effort release entered");
        assert_eq!(fixture.transport.frames("release").len(), 1);
        release_gate.open();
    }

    #[test]
    fn queued_release_is_not_starved_by_an_always_due_renewal() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 0);
        let (renew_entered, renew_gate) = fixture.transport.push_blocking(
            "renew",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                renew_result_after(0, PublicationState::CheckingEndpoint),
            ),
        );
        let lease = acquire_lease(&fixture, "workbench");
        renew_entered
            .recv_timeout(Duration::from_secs(2))
            .expect("due renewal entered");
        release_success(&fixture);
        drop(lease);
        renew_gate.open();
        fixture.transport.wait_for_requests("release", 1);
        assert_eq!(fixture.transport.frames("renew").len(), 1);
        assert_eq!(fixture.transport.frames("release").len(), 1);
    }

    #[test]
    fn ambiguous_release_exact_replay_converges_on_lease_lost() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        let observer = lease.clone();
        fixture.transport.push_error(
            "release",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        fixture.transport.push(
            "release",
            &ResponseEnvelope::<ReleaseResult>::error(
                fixture.epoch.clone(),
                ProtocolError::new(StableErrorCode::LeaseLost, "already gone", None),
            ),
        );
        lease.release().expect("ambiguous release converged");
        assert_eq!(
            observer.wait_for_change(0, Duration::from_secs(2)).state(),
            LeaseState::Released
        );
        let frames = fixture.transport.frames("release");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], frames[1]);
        drop(observer);
        assert_eq!(fixture.transport.frames("release").len(), 2);
    }

    #[test]
    fn explicit_release_timeout_fails_clones_closed_then_late_success_releases() {
        let fixture = fixture();
        enqueue_acquisition(&fixture, 10_000);
        let lease = acquire_lease(&fixture, "workbench");
        let observer = lease.clone();
        let mut releasing_lease = lease.clone();
        releasing_lease
            .supervisor
            .set_command_timeout(Duration::from_millis(500));
        let (entered, gate) = fixture.transport.push_blocking(
            "release",
            &ResponseEnvelope::success(fixture.epoch.clone(), ReleaseResult::released()),
        );
        let worker = std::thread::spawn(move || releasing_lease.release());
        entered
            .recv_timeout(Duration::from_secs(2))
            .expect("release exchange entered");

        assert!(matches!(
            worker.join().expect("release caller"),
            Err(ClientError::SupervisorTimeout)
        ));
        assert_eq!(observer.snapshot().state(), LeaseState::AuthorityUncertain);

        gate.open();
        observer
            .supervisor
            .join_for_test()
            .expect("supervisor completion");
        assert_eq!(observer.snapshot().state(), LeaseState::Released);
        drop(lease);
        drop(observer);
        assert_eq!(fixture.transport.frames("release").len(), 1);
    }

    #[test]
    fn terminal_attempts_remain_replayable_and_replacement_is_explicit() {
        let fixture = fixture();
        let project = project(&fixture);
        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Pending),
            ),
        );
        let prepared = project
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("prepare");
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let installed = prepared
            .confirm_origin_installed(&origin())
            .expect("origin installed");
        fixture.transport.push_error(
            "acquire",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        fixture.transport.push_error(
            "acquire",
            transport_failure(DeliveryCertainty::OutcomeUnknown),
        );
        assert!(matches!(
            installed.acquire(&listener),
            Err(ClientError::Transport(TransportFailure {
                certainty: DeliveryCertainty::OutcomeUnknown,
                ..
            }))
        ));
        let ambiguous_acquire = fixture.transport.recorded("acquire");
        assert_eq!(ambiguous_acquire.len(), 2);
        assert_eq!(ambiguous_acquire[0].frame, ambiguous_acquire[1].frame);
        assert_eq!(
            ambiguous_acquire[0].listener_fd,
            ambiguous_acquire[1].listener_fd
        );
        assert!(ambiguous_acquire[0].listener_fd.is_some());

        fixture.transport.push(
            "begin_acquisition",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_result(&fixture, ATTEMPT_A, AttemptState::Terminal),
            ),
        );
        let terminal = project
            .prepare(ServiceName::parse("workbench").expect("service"))
            .expect("terminal preparation");
        assert_eq!(terminal.attempt_state(), AttemptState::Terminal);
        let terminal = terminal.into_terminal().expect("terminal typestate");
        let installed = terminal
            .confirm_origin_installed(&origin())
            .expect("terminal origin");
        fixture.transport.push(
            "acquire",
            &ResponseEnvelope::success(fixture.epoch.clone(), acquire_result(10_000)),
        );
        let lease = installed.acquire(&listener).expect("terminal exact replay");
        let acquire_frames = fixture.transport.frames("acquire");
        assert_eq!(acquire_frames.len(), 3);
        assert_eq!(acquire_frames[0], acquire_frames[1]);
        assert_eq!(acquire_frames[1], acquire_frames[2]);

        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Terminal, ATTEMPT_A),
            ),
        );
        let terminal_rebind = lease
            .prepare_rebind()
            .expect("terminal rebind")
            .into_terminal()
            .expect("terminal rebind typestate");
        let candidate_listener = TcpListener::bind("127.0.0.1:0").expect("candidate listener");
        let candidate = terminal_rebind
            .confirm_origin_installed(&origin())
            .expect("terminal rebind origin");
        fixture.transport.push(
            "rebind",
            &ResponseEnvelope::<RebindResult>::error(
                fixture.epoch.clone(),
                ProtocolError::new(
                    StableErrorCode::EndpointUnhealthy,
                    "candidate unhealthy",
                    None,
                ),
            ),
        );
        assert!(matches!(
            lease.rebind(candidate, &candidate_listener),
            Err(ClientError::Protocol(error))
                if error.code() == StableErrorCode::EndpointUnhealthy
        ));

        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Terminal, ATTEMPT_A),
            ),
        );
        let candidate = lease
            .prepare_rebind()
            .expect("replayed terminal rebind")
            .into_terminal()
            .expect("replayed terminal typestate")
            .confirm_origin_installed(&origin())
            .expect("replayed terminal origin");
        fixture.transport.push(
            "rebind",
            &ResponseEnvelope::<RebindResult>::error(
                fixture.epoch.clone(),
                ProtocolError::new(
                    StableErrorCode::EndpointUnhealthy,
                    "candidate unhealthy",
                    None,
                ),
            ),
        );
        assert!(matches!(
            lease.rebind(candidate, &candidate_listener),
            Err(ClientError::Protocol(error))
                if error.code() == StableErrorCode::EndpointUnhealthy
        ));
        let rebind_frames = fixture.transport.frames("rebind");
        assert_eq!(rebind_frames.len(), 2);
        assert_eq!(rebind_frames[0], rebind_frames[1]);

        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Terminal, ATTEMPT_A),
            ),
        );
        let terminal_rebind = lease
            .prepare_rebind()
            .expect("terminal rebind for replacement")
            .into_terminal()
            .expect("terminal replacement typestate");
        fixture.transport.push(
            "begin_rebind",
            &ResponseEnvelope::success(
                fixture.epoch.clone(),
                begin_rebind_result(AttemptState::Terminal, ATTEMPT_B),
            ),
        );
        let replacement = terminal_rebind
            .replace(&lease)
            .expect("explicit replacement");
        assert_eq!(replacement.attempt_state(), AttemptState::Terminal);
        let begin_rebind = fixture.transport.recorded("begin_rebind");
        assert_eq!(begin_rebind.len(), 4);
        let replacement_requests = begin_rebind
            .iter()
            .map(|request| {
                let request = decode_request_frame(&request.frame).expect("decode begin rebind");
                let PublisherRequest::BeginRebind(request) = request.into_request() else {
                    panic!("expected begin rebind");
                };
                request.replace_terminal_attempt_handle
            })
            .collect::<Vec<_>>();
        assert!(replacement_requests[..3].iter().all(Option::is_none));
        assert!(replacement_requests[3].is_some());
        assert!(
            begin_rebind
                .iter()
                .all(|request| request.listener_fd.is_none())
        );

        release_success(&fixture);
        lease.release().expect("release");
    }

    #[test]
    fn protocol_origins_match_core_canonical_origin_acceptance() {
        for value in [
            "https://workbench.example.localhost",
            "https://workbench.example.localhost:8443",
            "https://WORKBENCH.example.localhost",
            "https://workbench.example.localhost.",
            "https://workbench.example.localhost:443",
            "http://workbench.example.localhost",
            "https://-workbench.example.localhost",
            "https://workbench..example.localhost",
            "https://workbench.example.localhost/path",
        ] {
            let protocol_accepts = SemanticOrigin::parse(value).is_ok();
            let core_accepts =
                serde_json::from_value::<locald_core::SemanticOrigin>(serde_json::json!(value))
                    .is_ok();
            assert_eq!(
                protocol_accepts, core_accepts,
                "origin grammar diverged for {value}"
            );
        }
    }
}
