//! Client-owned lease supervision and redacted observation.

use std::fmt;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::Duration;

use locald_publisher_protocol::{BindingRevision, LeaseHandle, PublicationState, SemanticOrigin};

use crate::client::{
    ClientError, PreparedRebind, ReacquisitionReason, RebindInstalledOrigin,
    RebindReplacementAuthority, authority_loss_reason, mutation_outcome_is_uncertain,
};
use crate::clock::RenewalSchedule;
use crate::wake::{WakeError, WakeMonitor, WakeRegistration, WakeSink};

/// Client-observed lifecycle of one acquired publisher lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    /// The supervisor has current lease authority.
    Active,
    /// The client cannot prove which binding or release outcome is current.
    AuthorityUncertain,
    /// Fresh preparation and caller-confirmed origin installation are required.
    ReacquisitionRequired(ReacquisitionReason),
    /// Explicit or final-handle release completed.
    Released,
}

/// Redacted, cloneable observation of one supervised lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseSnapshot {
    sequence: u64,
    state: LeaseState,
    binding_revision: BindingRevision,
    origin: SemanticOrigin,
    publication_state: PublicationState,
}

impl LeaseSnapshot {
    pub(super) const fn active(
        binding_revision: BindingRevision,
        origin: SemanticOrigin,
        publication_state: PublicationState,
    ) -> Self {
        Self {
            sequence: 0,
            state: LeaseState::Active,
            binding_revision,
            origin,
            publication_state,
        }
    }

    /// Monotonically increasing local observation sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Current client-owned lifecycle state.
    #[must_use]
    pub const fn state(&self) -> LeaseState {
        self.state
    }

    /// Last authenticated publisher-private binding revision.
    #[must_use]
    pub const fn binding_revision(&self) -> BindingRevision {
        self.binding_revision
    }

    /// Last authenticated semantic origin.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }

    /// Last authenticated privacy-safe publication state.
    #[must_use]
    pub const fn publication_state(&self) -> PublicationState {
        self.publication_state
    }
}

#[derive(Debug, Clone)]
pub(super) struct WaitAuthority {
    pub(super) lease_handle: LeaseHandle,
    pub(super) binding_revision: BindingRevision,
    pub(super) origin: SemanticOrigin,
    pub(super) schedule: RenewalSchedule,
}

#[derive(Debug)]
struct ObservedLease {
    snapshot: LeaseSnapshot,
    authority: Option<WaitAuthority>,
    listener: TcpListener,
}

#[derive(Debug)]
pub(super) struct SupervisorShared {
    observed: Mutex<ObservedLease>,
    changed: Condvar,
}

impl SupervisorShared {
    pub(super) fn new(
        snapshot: LeaseSnapshot,
        lease_handle: LeaseHandle,
        schedule: RenewalSchedule,
        listener: TcpListener,
    ) -> Self {
        let authority = WaitAuthority {
            lease_handle,
            binding_revision: snapshot.binding_revision,
            origin: snapshot.origin.clone(),
            schedule,
        };
        Self {
            observed: Mutex::new(ObservedLease {
                snapshot,
                authority: Some(authority),
                listener,
            }),
            changed: Condvar::new(),
        }
    }

    pub(super) fn snapshot(&self) -> LeaseSnapshot {
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot
            .clone()
    }

    pub(super) fn state(&self) -> LeaseState {
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot
            .state
    }

    pub(super) fn wait_for_change(&self, after_sequence: u64, timeout: Duration) -> LeaseSnapshot {
        let (observed, _) = self
            .changed
            .wait_timeout_while(
                self.observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                timeout,
                |observed| observed.snapshot.sequence <= after_sequence,
            )
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = observed.snapshot.clone();
        drop(observed);
        snapshot
    }

    pub(super) fn wait_authority(&self) -> Option<WaitAuthority> {
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .authority
            .clone()
    }

    pub(super) fn clone_listener(&self) -> Result<TcpListener, std::io::Error> {
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .listener
            .try_clone()
    }

    pub(super) fn renew(
        &self,
        binding_revision: BindingRevision,
        publication_state: PublicationState,
        schedule: RenewalSchedule,
    ) -> bool {
        let mut observed = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if observed.snapshot.state != LeaseState::Active
            || observed.snapshot.binding_revision != binding_revision
        {
            return false;
        }
        let changed = observed.snapshot.publication_state != publication_state;
        if changed {
            observed.snapshot.sequence = observed.snapshot.sequence.saturating_add(1);
            observed.snapshot.publication_state = publication_state;
        }
        if let Some(authority) = &mut observed.authority {
            authority.schedule = schedule;
        }
        drop(observed);
        if changed {
            self.changed.notify_all();
        }
        true
    }

    pub(super) fn rebind(
        &self,
        lease_handle: LeaseHandle,
        binding_revision: BindingRevision,
        origin: SemanticOrigin,
        publication_state: PublicationState,
        schedule: RenewalSchedule,
        listener: TcpListener,
    ) -> bool {
        let mut observed = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if observed.snapshot.state != LeaseState::Active {
            return false;
        }
        observed.snapshot.sequence = observed.snapshot.sequence.saturating_add(1);
        observed.snapshot.binding_revision = binding_revision;
        observed.snapshot.origin = origin.clone();
        observed.snapshot.publication_state = publication_state;
        observed.authority = Some(WaitAuthority {
            lease_handle,
            binding_revision,
            origin,
            schedule,
        });
        observed.listener = listener;
        drop(observed);
        self.changed.notify_all();
        true
    }

    pub(super) fn ready(&self, expected_binding_revision: BindingRevision) -> bool {
        let mut observed = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if observed.snapshot.state != LeaseState::Active
            || observed.snapshot.binding_revision != expected_binding_revision
        {
            return false;
        }
        let changed = observed.snapshot.publication_state != PublicationState::Ready;
        if changed {
            observed.snapshot.sequence = observed.snapshot.sequence.saturating_add(1);
            observed.snapshot.publication_state = PublicationState::Ready;
        }
        drop(observed);
        if changed {
            self.changed.notify_all();
        }
        true
    }

    pub(super) fn transition(&self, state: LeaseState) {
        let mut observed = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = observed.snapshot.state;
        let permitted = if current == LeaseState::Released || current == state {
            false
        } else if state == LeaseState::Released {
            true
        } else {
            match current {
                LeaseState::Active => true,
                LeaseState::AuthorityUncertain => {
                    matches!(state, LeaseState::ReacquisitionRequired(_))
                }
                LeaseState::ReacquisitionRequired(_) | LeaseState::Released => false,
            }
        };
        if !permitted {
            return;
        }
        observed.snapshot.sequence = observed.snapshot.sequence.saturating_add(1);
        observed.snapshot.state = state;
        if state != LeaseState::Active {
            observed.authority = None;
        }
        drop(observed);
        self.changed.notify_all();
    }
}

/// One generation of authenticated daemon-session authority.
#[derive(Debug, Clone)]
pub(super) struct SessionFence {
    session: Arc<SharedSession>,
    generation: u64,
}

impl SessionFence {
    pub(super) fn is_current(&self) -> bool {
        self.session.generation.load(Ordering::Acquire) == self.generation
    }

    pub(super) fn invalidate(&self) {
        self.session.invalidate_if(self.generation);
    }
}

/// Shared invalidation fanout for every typestate and lease in one daemon session.
#[derive(Debug, Default)]
pub(super) struct SharedSession {
    generation: AtomicU64,
    subscribers: Mutex<Vec<Weak<SupervisorSignals>>>,
}

impl SharedSession {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            generation: AtomicU64::new(1),
            subscribers: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn fence(self: &Arc<Self>) -> SessionFence {
        SessionFence {
            session: Arc::clone(self),
            generation: self.generation.load(Ordering::Acquire),
        }
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.nudge_subscribers();
    }

    fn invalidate_if(&self, generation: u64) {
        if self
            .generation
            .compare_exchange(
                generation,
                generation.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.nudge_subscribers();
        }
    }

    fn nudge_subscribers(&self) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subscribers.retain(|subscriber| {
            let Some(subscriber) = subscriber.upgrade() else {
                return false;
            };
            subscriber.nudge();
            true
        });
    }

    pub(super) fn fail_uncertain(&self) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subscribers.retain(|subscriber| {
            let Some(subscriber) = subscriber.upgrade() else {
                return false;
            };
            subscriber.failed(WakeError::Failed(
                "session suspend-aware clock failed".to_owned(),
            ));
            true
        });
    }

    fn subscribe(&self, subscriber: &Arc<SupervisorSignals>) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subscribers.retain(|existing| existing.strong_count() != 0);
        subscribers.push(Arc::downgrade(subscriber));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RenewalCause {
    Wake,
    Deadline,
}

pub(super) trait SupervisorDriver: Send + fmt::Debug + 'static {
    fn renewal_wait(&self) -> Result<Duration, ClientError>;
    fn renew(&mut self, cause: RenewalCause) -> Result<(), ClientError>;
    fn begin_rebind(
        &mut self,
        replacement: Option<RebindReplacementAuthority>,
    ) -> Result<PreparedRebind, ClientError>;
    fn rebind(
        &mut self,
        candidate: RebindInstalledOrigin,
        listener: TcpListener,
    ) -> Result<(), ClientError>;
    fn release(&mut self) -> Result<(), ClientError>;
}

#[derive(Debug)]
enum SupervisorCommand {
    Synchronize(SyncSender<Result<(), SupervisorCallError>>),
    BeginRebind {
        replacement: Option<RebindReplacementAuthority>,
        response: SyncSender<Result<PreparedRebind, SupervisorCallError>>,
    },
    Rebind {
        candidate: RebindInstalledOrigin,
        listener: TcpListener,
        response: SyncSender<Result<(), SupervisorCallError>>,
    },
    Release(Option<SyncSender<Result<(), SupervisorCallError>>>),
}

#[derive(Debug)]
enum SupervisorSignal {
    Command(SupervisorCommand),
    Nudge,
}

#[derive(Debug)]
struct SupervisorSignals {
    sender: Sender<SupervisorSignal>,
    wake_pending: AtomicBool,
    wake_failure: Mutex<Option<WakeError>>,
}

impl SupervisorSignals {
    fn nudge(&self) {
        drop(self.sender.send(SupervisorSignal::Nudge));
    }

    fn take_wake_failure(&self) -> Option<WakeError> {
        self.wake_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl WakeSink for SupervisorSignals {
    fn resumed(&self) {
        self.wake_pending.store(true, Ordering::Release);
        self.nudge();
    }

    fn failed(&self, error: WakeError) {
        *self
            .wake_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
        self.nudge();
    }
}

#[derive(Debug)]
struct FinalHandleDrop {
    sender: Sender<SupervisorSignal>,
    release_signaled: AtomicBool,
}

impl FinalHandleDrop {
    fn signal_release(
        &self,
        response: Option<SyncSender<Result<(), SupervisorCallError>>>,
    ) -> Result<bool, ()> {
        if self.release_signaled.swap(true, Ordering::AcqRel) {
            return Ok(false);
        }
        self.sender
            .send(SupervisorSignal::Command(SupervisorCommand::Release(
                response,
            )))
            .map(|()| true)
            .map_err(|_| ())
    }
}

impl Drop for FinalHandleDrop {
    fn drop(&mut self) {
        // The final public handle only enqueues one best-effort release. It
        // never performs IPC or joins the supervisor thread.
        let _release_result = self.signal_release(None);
    }
}

#[derive(Debug)]
pub(super) enum SupervisorCallError {
    Operation(ClientError),
    Reacquisition(ReacquisitionReason),
    Timeout,
    Stopped,
    ReleaseAlreadyRequested,
}

#[derive(Debug)]
pub(super) struct PendingSupervisor {
    sender: Sender<SupervisorSignal>,
    receiver: Receiver<SupervisorSignal>,
    signals: Arc<SupervisorSignals>,
    registration: Box<dyn WakeRegistration>,
}

impl PendingSupervisor {
    pub(super) fn register(
        session: &Arc<SharedSession>,
        wake_monitor: &dyn WakeMonitor,
    ) -> Result<Self, WakeError> {
        let (sender, receiver) = mpsc::channel();
        let signals = Arc::new(SupervisorSignals {
            sender: sender.clone(),
            wake_pending: AtomicBool::new(false),
            wake_failure: Mutex::new(None),
        });
        let registration = wake_monitor.register(Arc::clone(&signals) as Arc<dyn WakeSink>)?;
        session.subscribe(&signals);
        Ok(Self {
            sender,
            receiver,
            signals,
            registration,
        })
    }

    pub(super) fn start<D: SupervisorDriver>(
        self,
        fence: SessionFence,
        shared: Arc<SupervisorShared>,
        command_timeout: Duration,
        driver: D,
    ) -> Result<SupervisorHandle, WakeError> {
        let Self {
            sender,
            receiver,
            signals,
            registration,
        } = self;
        let thread_shared = Arc::clone(&shared);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let thread_handle = thread::Builder::new()
            .name("locald-publisher-lease".to_owned())
            .spawn(move || {
                let mut driver = driver;
                let _registration = registration;
                if let Some(error) = signals.take_wake_failure() {
                    drop(startup_sender.send(Err(error)));
                    return;
                }
                drop(startup_sender.send(Ok(())));
                run_supervisor(&mut driver, &fence, &signals, &receiver, &thread_shared);
            })
            .map_err(|error| {
                WakeError::Failed(format!("cannot start lease supervisor: {error}"))
            })?;
        #[cfg(test)]
        let test_thread = Arc::new(Mutex::new(Some(thread_handle)));
        #[cfg(not(test))]
        drop(thread_handle);
        startup_receiver.recv().map_err(|_| {
            WakeError::Failed("lease supervisor stopped during startup".to_owned())
        })??;
        Ok(SupervisorHandle {
            sender: sender.clone(),
            final_drop: Arc::new(FinalHandleDrop {
                sender,
                release_signaled: AtomicBool::new(false),
            }),
            shared,
            command_timeout,
            #[cfg(test)]
            test_thread,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct SupervisorHandle {
    sender: Sender<SupervisorSignal>,
    final_drop: Arc<FinalHandleDrop>,
    shared: Arc<SupervisorShared>,
    command_timeout: Duration,
    #[cfg(test)]
    test_thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl SupervisorHandle {
    fn require_active(&self) -> Result<(), SupervisorCallError> {
        if self.shared.state() == LeaseState::Active {
            Ok(())
        } else {
            Err(SupervisorCallError::Stopped)
        }
    }

    pub(super) fn snapshot(&self) -> LeaseSnapshot {
        self.shared.snapshot()
    }

    pub(super) fn wait_for_change(&self, after_sequence: u64, timeout: Duration) -> LeaseSnapshot {
        self.shared.wait_for_change(after_sequence, timeout)
    }

    pub(super) fn wait_authority(&self) -> Option<WaitAuthority> {
        self.shared.wait_authority()
    }

    pub(super) fn ready(&self, expected_binding_revision: BindingRevision) -> bool {
        self.shared.ready(expected_binding_revision)
    }

    pub(super) fn clone_listener(&self) -> Result<TcpListener, std::io::Error> {
        self.shared.clone_listener()
    }

    pub(super) fn synchronize(&self) -> Result<(), SupervisorCallError> {
        self.require_active()?;
        let (response, result) = mpsc::sync_channel(1);
        self.sender
            .send(SupervisorSignal::Command(SupervisorCommand::Synchronize(
                response,
            )))
            .map_err(|_| SupervisorCallError::Stopped)?;
        receive_response(&result, self.command_timeout)
    }

    pub(super) fn begin_rebind(
        &self,
        replacement: Option<RebindReplacementAuthority>,
    ) -> Result<PreparedRebind, SupervisorCallError> {
        self.require_active()?;
        let (response, result) = mpsc::sync_channel(1);
        self.sender
            .send(SupervisorSignal::Command(SupervisorCommand::BeginRebind {
                replacement,
                response,
            }))
            .map_err(|_| SupervisorCallError::Stopped)?;
        receive_response(&result, self.command_timeout)
    }

    pub(super) fn rebind(
        &self,
        candidate: RebindInstalledOrigin,
        listener: TcpListener,
    ) -> Result<(), SupervisorCallError> {
        self.require_active()?;
        let (response, result) = mpsc::sync_channel(1);
        self.sender
            .send(SupervisorSignal::Command(SupervisorCommand::Rebind {
                candidate,
                listener,
                response,
            }))
            .map_err(|_| SupervisorCallError::Stopped)?;
        match receive_response(&result, self.command_timeout) {
            Err(SupervisorCallError::Timeout) => {
                self.shared.transition(LeaseState::AuthorityUncertain);
                Err(SupervisorCallError::Timeout)
            }
            result => result,
        }
    }

    pub(super) fn release(&self) -> Result<(), SupervisorCallError> {
        self.require_active()?;
        let (response, result) = mpsc::sync_channel(1);
        let signaled = self
            .final_drop
            .signal_release(Some(response))
            .map_err(|()| SupervisorCallError::Stopped)?;
        if !signaled {
            return Err(SupervisorCallError::ReleaseAlreadyRequested);
        }
        match receive_response(&result, self.command_timeout) {
            Err(SupervisorCallError::Timeout) => {
                self.shared.transition(LeaseState::AuthorityUncertain);
                Err(SupervisorCallError::Timeout)
            }
            result => result,
        }
    }

    #[cfg(test)]
    pub(super) const fn set_command_timeout(&mut self, timeout: Duration) {
        self.command_timeout = timeout;
    }

    #[cfg(test)]
    pub(super) fn join_for_test(&self) -> thread::Result<()> {
        let thread = self
            .test_thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        thread.map_or(Ok(()), thread::JoinHandle::join)
    }

    pub(super) fn invalidate(&self, reason: ReacquisitionReason) {
        self.shared
            .transition(LeaseState::ReacquisitionRequired(reason));
        drop(self.sender.send(SupervisorSignal::Nudge));
    }

    pub(super) fn state(&self) -> LeaseState {
        self.shared.state()
    }
}

fn receive_response<T>(
    receiver: &Receiver<Result<T, SupervisorCallError>>,
    timeout: Duration,
) -> Result<T, SupervisorCallError> {
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(SupervisorCallError::Timeout),
        Err(RecvTimeoutError::Disconnected) => Err(SupervisorCallError::Stopped),
    }
}

fn run_supervisor<D: SupervisorDriver>(
    driver: &mut D,
    fence: &SessionFence,
    signals: &SupervisorSignals,
    receiver: &Receiver<SupervisorSignal>,
    shared: &SupervisorShared,
) {
    let mut pending_command = None;
    loop {
        if !authority_is_current(fence, signals, shared) {
            break;
        }

        if pending_command.is_none() {
            match receiver.try_recv() {
                Ok(SupervisorSignal::Command(command)) => pending_command = Some(command),
                Ok(SupervisorSignal::Nudge) | Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    finish_release(driver, None, shared);
                    break;
                }
            }
        }

        if matches!(pending_command, Some(SupervisorCommand::Release(_))) {
            let Some(SupervisorCommand::Release(response)) = pending_command.take() else {
                unreachable!("release command was matched")
            };
            finish_release(driver, response, shared);
            break;
        }

        if signals.wake_pending.swap(false, Ordering::AcqRel) {
            if finish_maintenance(
                &driver.renew(RenewalCause::Wake),
                MaintenanceOperation::Renew,
                shared,
            ) {
                break;
            }
            if !authority_is_current(fence, signals, shared) {
                break;
            }
            if let Some(command) = pending_command.take()
                && run_command(driver, command, shared)
            {
                break;
            }
            continue;
        }

        let renewal_wait = match driver.renewal_wait() {
            Ok(wait) => wait,
            Err(error) => {
                if let Some(reason) = authority_loss_reason(&error) {
                    shared.transition(LeaseState::ReacquisitionRequired(reason));
                } else {
                    shared.transition(LeaseState::AuthorityUncertain);
                }
                break;
            }
        };
        if renewal_wait.is_zero() {
            if finish_maintenance(
                &driver.renew(RenewalCause::Deadline),
                MaintenanceOperation::Renew,
                shared,
            ) {
                break;
            }
            if !authority_is_current(fence, signals, shared) {
                break;
            }
            if let Some(command) = pending_command.take()
                && run_command(driver, command, shared)
            {
                break;
            }
            continue;
        }

        if let Some(command) = pending_command.take() {
            if run_command(driver, command, shared) {
                break;
            }
            continue;
        }

        match receiver.recv_timeout(renewal_wait) {
            Ok(SupervisorSignal::Command(command)) => pending_command = Some(command),
            Ok(SupervisorSignal::Nudge) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // `FinalHandleDrop` normally sends an explicit release signal;
                // disconnection is a last-resort one-attempt cleanup.
                finish_release(driver, None, shared);
                break;
            }
        }
    }
}

fn authority_is_current(
    fence: &SessionFence,
    signals: &SupervisorSignals,
    shared: &SupervisorShared,
) -> bool {
    if shared.state() != LeaseState::Active {
        return false;
    }
    if !fence.is_current() {
        shared.transition(LeaseState::ReacquisitionRequired(
            ReacquisitionReason::DaemonEpochChanged,
        ));
        return false;
    }
    if signals.take_wake_failure().is_some() {
        shared.transition(LeaseState::AuthorityUncertain);
        return false;
    }
    true
}

fn run_command<D: SupervisorDriver>(
    driver: &mut D,
    command: SupervisorCommand,
    shared: &SupervisorShared,
) -> bool {
    match command {
        SupervisorCommand::Synchronize(response) => {
            drop(response.send(Ok(())));
            false
        }
        SupervisorCommand::BeginRebind {
            replacement,
            response,
        } => {
            let result = driver.begin_rebind(replacement);
            let stop = finish_call_result(&result, MaintenanceOperation::BeginRebind, shared);
            drop(response.send(result.map_err(call_error)));
            stop
        }
        SupervisorCommand::Rebind {
            candidate,
            listener,
            response,
        } => {
            let result = driver.rebind(candidate, listener);
            let stop = finish_call_result(&result, MaintenanceOperation::Rebind, shared);
            drop(response.send(result.map_err(call_error)));
            stop
        }
        SupervisorCommand::Release(response) => {
            finish_release(driver, response, shared);
            true
        }
    }
}

fn finish_release<D: SupervisorDriver>(
    driver: &mut D,
    response: Option<SyncSender<Result<(), SupervisorCallError>>>,
    shared: &SupervisorShared,
) {
    let result = driver.release();
    let outgoing = match result {
        Ok(()) => {
            shared.transition(LeaseState::Released);
            Ok(())
        }
        Err(error) => authority_loss_reason(&error).map_or_else(
            || {
                shared.transition(LeaseState::AuthorityUncertain);
                Err(SupervisorCallError::Operation(error))
            },
            |reason| {
                shared.transition(LeaseState::ReacquisitionRequired(reason));
                Err(SupervisorCallError::Reacquisition(reason))
            },
        ),
    };
    if let Some(response) = response {
        drop(response.send(outgoing));
    }
}

#[derive(Debug, Clone, Copy)]
enum MaintenanceOperation {
    Renew,
    BeginRebind,
    Rebind,
}

fn finish_maintenance(
    result: &Result<(), ClientError>,
    operation: MaintenanceOperation,
    shared: &SupervisorShared,
) -> bool {
    finish_call_result(result, operation, shared)
}

fn finish_call_result<T>(
    result: &Result<T, ClientError>,
    operation: MaintenanceOperation,
    shared: &SupervisorShared,
) -> bool {
    let Err(error) = result else {
        return false;
    };
    if let Some(reason) = authority_loss_reason(error) {
        shared.transition(LeaseState::ReacquisitionRequired(reason));
        return true;
    }
    let uncertain = match operation {
        MaintenanceOperation::Renew => !matches!(
            error,
            ClientError::Transport(crate::backend::TransportFailure {
                certainty: crate::backend::DeliveryCertainty::NotSent,
                ..
            })
        ),
        MaintenanceOperation::BeginRebind => matches!(error, ClientError::RebindResultMismatch),
        MaintenanceOperation::Rebind => mutation_outcome_is_uncertain(error),
    };
    if uncertain {
        shared.transition(LeaseState::AuthorityUncertain);
    }
    uncertain
}

fn call_error(error: ClientError) -> SupervisorCallError {
    authority_loss_reason(&error).map_or_else(
        || SupervisorCallError::Operation(error),
        SupervisorCallError::Reacquisition,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribing_compacts_dead_supervisor_signals() {
        let session = SharedSession::new();
        for _ in 0..100 {
            let (sender, _receiver) = mpsc::channel();
            let signals = Arc::new(SupervisorSignals {
                sender,
                wake_pending: AtomicBool::new(false),
                wake_failure: Mutex::new(None),
            });
            session.subscribe(&signals);
        }

        let (sender, _receiver) = mpsc::channel();
        let retained = Arc::new(SupervisorSignals {
            sender,
            wake_pending: AtomicBool::new(false),
            wake_failure: Mutex::new(None),
        });
        session.subscribe(&retained);

        let subscribers = session
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(subscribers.len(), 1);
        assert!(subscribers[0].upgrade().is_some());
        drop(subscribers);
    }
}
