//! Process-global coordination between process creation and descriptor acquisition.
//!
//! macOS does not provide an atomic `MSG_CMSG_CLOEXEC` equivalent for
//! descriptors received with `SCM_RIGHTS`, or atomic close-on-exec socket
//! creation. Publisher transports therefore have to acquire a descriptor and
//! set `FD_CLOEXEC` while no other thread can create a child process. Every
//! process-spawn path takes a shared spawn permit, while descriptor acquisition
//! takes an exclusive, fail-closed guard.
//!
//! Spawn permits deliberately do not exclude one another. Besides allowing
//! unrelated process creation to proceed concurrently, this makes the barrier
//! safe for helpers whose implementation can itself create a child process:
//! nested spawn permits increment the shared counter instead of waiting on the
//! outer permit.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Instant;
use thiserror::Error;

static GLOBAL_PROCESS_SPAWN_BARRIER: OnceLock<ProcessSpawnBarrier> = OnceLock::new();

/// Process-wide exclusion between process creation and descriptor acquisition.
#[derive(Clone)]
pub struct ProcessSpawnBarrier {
    inner: Arc<BarrierInner>,
}

impl ProcessSpawnBarrier {
    /// Return the barrier shared by every descriptor and spawn path in this process.
    #[must_use]
    pub fn global() -> &'static Self {
        GLOBAL_PROCESS_SPAWN_BARRIER.get_or_init(Self::new)
    }

    fn new() -> Self {
        Self {
            inner: Arc::new(BarrierInner {
                state: Mutex::new(BarrierState::default()),
                descriptor_acquisitions_finished: Condvar::new(),
                spawns_finished: Condvar::new(),
            }),
        }
    }

    /// Announce a process spawn or direct exec, then wait for descriptor acquisition.
    ///
    /// More than one spawn permit may be active at once. This is intentional:
    /// code that guards an opaque helper which internally spawns may also guard
    /// its own direct spawn without deadlocking.
    #[must_use]
    pub fn enter_spawn(&self) -> ProcessSpawnPermit {
        let mut state = self.lock_state();
        // Publish spawn intent before waiting. Otherwise a stream of new
        // descriptor acquisitions can repeatedly enter while this spawn is
        // queued, starving every child-process launch.
        state.active_spawns = state.active_spawns.saturating_add(1);
        while state.active_descriptor_acquisitions != 0 {
            state = self
                .inner
                .descriptor_acquisitions_finished
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(state);

        ProcessSpawnPermit {
            barrier: self.clone(),
            active: true,
        }
    }

    /// Spawn one standard-library command while holding the process-spawn permit.
    ///
    /// The permit is released as soon as [`std::process::Command::spawn`]
    /// returns. Waiting for the child is deliberately left to the caller so a
    /// long-running child does not block descriptor acquisition.
    ///
    /// # Errors
    ///
    /// Returns the error from [`std::process::Command::spawn`].
    pub fn spawn_std_command(
        &self,
        command: &mut std::process::Command,
    ) -> std::io::Result<std::process::Child> {
        self.with_spawn_permit(|| command.spawn())
    }

    /// Spawn one Tokio command while holding the process-spawn permit.
    ///
    /// The permit is released as soon as [`tokio::process::Command::spawn`]
    /// returns. Awaiting the child is deliberately left to the caller so a
    /// long-running child does not block descriptor acquisition.
    ///
    /// # Errors
    ///
    /// Returns the error from [`tokio::process::Command::spawn`].
    pub fn spawn_tokio_command(
        &self,
        command: &mut tokio::process::Command,
    ) -> std::io::Result<tokio::process::Child> {
        self.with_spawn_permit(|| command.spawn())
    }

    /// Enter descriptor acquisition exclusion or fail if a spawn is active.
    ///
    /// Acquisition never waits for an in-progress spawn. The caller must fail
    /// the operation before acquiring a descriptor and let its bounded caller
    /// retry, keeping descriptor ownership fail-closed.
    ///
    /// # Errors
    ///
    /// Returns [`DescriptorAcquisitionBlocked`] when at least one process-spawn
    /// permit is active.
    pub fn try_enter_descriptor_acquisition(
        &self,
    ) -> Result<DescriptorAcquisitionGuard, DescriptorAcquisitionBlocked> {
        let mut state = self.lock_state();
        if state.active_spawns != 0 {
            return Err(DescriptorAcquisitionBlocked {
                active_spawns: state.active_spawns,
            });
        }
        state.active_descriptor_acquisitions =
            state.active_descriptor_acquisitions.saturating_add(1);
        drop(state);

        Ok(DescriptorAcquisitionGuard {
            barrier: self.clone(),
            active: true,
        })
    }

    /// Wait until descriptor acquisition can begin, bounded by an absolute deadline.
    ///
    /// This is intended for operations that have not acquired a descriptor and
    /// can safely wait for a concurrent process spawn to finish. Descriptor
    /// receipt after request delivery should continue to use the immediate,
    /// fail-closed [`Self::try_enter_descriptor_acquisition`] path.
    ///
    /// # Errors
    ///
    /// Returns [`DescriptorAcquisitionBlocked`] when the deadline is reached
    /// before descriptor acquisition can begin. The reported active-spawn
    /// count can be zero when the deadline wins a last-spawn completion race.
    pub fn enter_descriptor_acquisition_before(
        &self,
        deadline: Instant,
    ) -> Result<DescriptorAcquisitionGuard, DescriptorAcquisitionBlocked> {
        self.enter_descriptor_acquisition_before_with_wait_hook(deadline, || {}, Instant::now)
    }

    fn enter_descriptor_acquisition_before_with_wait_hook(
        &self,
        deadline: Instant,
        before_wait: impl FnOnce(),
        mut now: impl FnMut() -> Instant,
    ) -> Result<DescriptorAcquisitionGuard, DescriptorAcquisitionBlocked> {
        let mut state = self.lock_state();
        let mut before_wait = Some(before_wait);
        while state.active_spawns != 0 {
            let Some(remaining) = deadline.checked_duration_since(now()) else {
                return Err(DescriptorAcquisitionBlocked {
                    active_spawns: state.active_spawns,
                });
            };
            if let Some(before_wait) = before_wait.take() {
                before_wait();
            }
            let (next_state, wait_result) = self
                .inner
                .spawns_finished
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next_state;
            if wait_result.timed_out() && state.active_spawns != 0 {
                return Err(DescriptorAcquisitionBlocked {
                    active_spawns: state.active_spawns,
                });
            }
        }
        if now() >= deadline {
            return Err(DescriptorAcquisitionBlocked {
                active_spawns: state.active_spawns,
            });
        }
        state.active_descriptor_acquisitions =
            state.active_descriptor_acquisitions.saturating_add(1);
        drop(state);

        Ok(DescriptorAcquisitionGuard {
            barrier: self.clone(),
            active: true,
        })
    }

    /// Enter descriptor-receipt exclusion or fail if a spawn is active.
    ///
    /// This compatibility name retains the server's `SCM_RIGHTS` vocabulary;
    /// new descriptor-creation code should use
    /// [`Self::try_enter_descriptor_acquisition`].
    ///
    /// # Errors
    ///
    /// Returns [`DescriptorReceiptBlocked`] when at least one process-spawn
    /// permit is active.
    pub fn try_enter_descriptor_receipt(
        &self,
    ) -> Result<DescriptorReceiptGuard, DescriptorReceiptBlocked> {
        self.try_enter_descriptor_acquisition()
    }

    fn lock_state(&self) -> MutexGuard<'_, BarrierState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn with_spawn_permit<T>(&self, spawn: impl FnOnce() -> T) -> T {
        let permit = self.enter_spawn();
        let result = spawn();
        drop(permit);
        result
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    /// Construct an isolated barrier for deterministic cross-crate tests.
    #[must_use]
    pub fn isolated_for_test() -> Self {
        Self::new()
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    /// Observe queued and active spawn sections in deterministic tests.
    #[must_use]
    pub fn announced_spawns_for_test(&self) -> usize {
        self.lock_state().active_spawns
    }
}

impl fmt::Debug for ProcessSpawnBarrier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock_state();
        formatter
            .debug_struct("ProcessSpawnBarrier")
            .field("active_spawns", &state.active_spawns)
            .field(
                "active_descriptor_acquisitions",
                &state.active_descriptor_acquisitions,
            )
            .finish()
    }
}

#[derive(Debug)]
struct BarrierInner {
    state: Mutex<BarrierState>,
    descriptor_acquisitions_finished: Condvar,
    spawns_finished: Condvar,
}

#[derive(Debug, Default)]
struct BarrierState {
    active_spawns: usize,
    active_descriptor_acquisitions: usize,
}

/// Shared permit announcing that a child process may be created or exec may run.
///
/// Hold the permit from immediately before the operation that can fork,
/// `posix_spawn`, or exec until that operation returns. An exec that succeeds
/// replaces the process while every guarded descriptor is already close-on-exec.
/// For command APIs, call `spawn` under the permit, drop the permit when
/// `spawn` returns, and wait for the child separately.
#[derive(Debug)]
pub struct ProcessSpawnPermit {
    barrier: ProcessSpawnBarrier,
    active: bool,
}

impl Drop for ProcessSpawnPermit {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let mut state = self.barrier.lock_state();
        debug_assert!(state.active_spawns > 0, "spawn permit count underflow");
        state.active_spawns = state.active_spawns.saturating_sub(1);
        let spawns_are_idle = state.active_spawns == 0;
        drop(state);
        self.active = false;

        if spawns_are_idle {
            self.barrier.inner.spawns_finished.notify_all();
        }
    }
}

/// Exclusive guard covering descriptor acquisition through `FD_CLOEXEC` setup.
#[derive(Debug)]
pub struct DescriptorAcquisitionGuard {
    barrier: ProcessSpawnBarrier,
    active: bool,
}

impl Drop for DescriptorAcquisitionGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let mut state = self.barrier.lock_state();
        debug_assert!(
            state.active_descriptor_acquisitions > 0,
            "descriptor acquisition guard count underflow"
        );
        state.active_descriptor_acquisitions =
            state.active_descriptor_acquisitions.saturating_sub(1);
        let acquisition_is_idle = state.active_descriptor_acquisitions == 0;
        self.active = false;
        drop(state);

        if acquisition_is_idle {
            self.barrier
                .inner
                .descriptor_acquisitions_finished
                .notify_all();
        }
    }
}

/// Compatibility name for an `SCM_RIGHTS` descriptor-receipt guard.
pub type DescriptorReceiptGuard = DescriptorAcquisitionGuard;

/// Descriptor acquisition could not begin under the selected barrier policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("descriptor acquisition blocked with {active_spawns} active process spawn(s) observed")]
pub struct DescriptorAcquisitionBlocked {
    active_spawns: usize,
}

impl DescriptorAcquisitionBlocked {
    /// Number of active process-spawn sections observed by the failed acquisition.
    ///
    /// This can be zero for a deadline-bounded acquisition that expires after
    /// the last spawn finishes but before descriptor authority is granted.
    #[must_use]
    pub const fn active_spawns(self) -> usize {
        self.active_spawns
    }
}

/// Compatibility name for a blocked `SCM_RIGHTS` descriptor receipt.
pub type DescriptorReceiptBlocked = DescriptorAcquisitionBlocked;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn descriptor_acquisition_fails_closed_while_a_spawn_is_active() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let _spawn = barrier.enter_spawn();

        let error = barrier
            .try_enter_descriptor_acquisition()
            .expect_err("acquisition must fail while a spawn is active");

        assert_eq!(error.active_spawns(), 1);
    }

    #[test]
    fn bounded_descriptor_acquisition_waits_until_spawn_finishes() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let spawn = barrier.enter_spawn();
        let worker_barrier = barrier.clone();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            let acquisition = worker_barrier
                .enter_descriptor_acquisition_before_with_wait_hook(
                    Instant::now() + Duration::from_secs(1),
                    || waiting_tx.send(()).expect("announce barrier wait"),
                    Instant::now,
                )
                .expect("acquisition waits for spawn");
            acquired_tx.send(()).expect("announce acquisition");
            drop(acquisition);
        });

        waiting_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker reaches barrier wait");
        assert!(
            acquired_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "descriptor acquisition must not enter during an active spawn"
        );
        drop(spawn);
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("acquisition proceeds after spawn");
        worker.join().expect("worker exits");
    }

    #[test]
    fn bounded_descriptor_acquisition_fails_when_deadline_expires() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let spawn = barrier.enter_spawn();

        let error = barrier
            .enter_descriptor_acquisition_before(Instant::now())
            .expect_err("expired deadline must not enter descriptor acquisition");

        assert_eq!(error.active_spawns(), 1);
        drop(spawn);
        barrier
            .try_enter_descriptor_acquisition()
            .expect("timed-out waiter must not retain descriptor authority");
    }

    #[test]
    fn bounded_descriptor_acquisition_rejects_an_expired_deadline_without_a_spawn() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();

        let error = barrier
            .enter_descriptor_acquisition_before(Instant::now())
            .expect_err("expired deadline must not grant descriptor authority");

        assert_eq!(error.active_spawns(), 0);
        barrier
            .try_enter_descriptor_acquisition()
            .expect("expired deadline must not retain descriptor authority");
    }

    #[test]
    fn bounded_descriptor_acquisition_rechecks_deadline_after_last_spawn_finishes() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let mut spawn = barrier.enter_spawn();
        let worker_barrier = barrier.clone();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let deadline = Instant::now() + Duration::from_secs(1);
        let before_deadline = deadline
            .checked_sub(Duration::from_secs(1))
            .expect("construct pre-deadline instant");
        let spawn_finished = Arc::new(AtomicBool::new(false));
        let worker_spawn_finished = Arc::clone(&spawn_finished);

        let worker = thread::spawn(move || {
            worker_barrier.enter_descriptor_acquisition_before_with_wait_hook(
                deadline,
                || {
                    waiting_tx.send(()).expect("announce barrier wait");
                },
                || {
                    if worker_spawn_finished.load(Ordering::Acquire) {
                        deadline
                    } else {
                        before_deadline
                    }
                },
            )
        });

        waiting_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker reaches barrier wait");
        let mut state = barrier.lock_state();
        state.active_spawns = 0;
        spawn.active = false;
        spawn_finished.store(true, Ordering::Release);
        barrier.inner.spawns_finished.notify_all();
        drop(state);

        let error = worker
            .join()
            .expect("worker exits")
            .expect_err("deadline expiry must win the last-spawn completion race");
        assert_eq!(error.active_spawns(), 0);
        barrier
            .try_enter_descriptor_acquisition()
            .expect("deadline race must not retain descriptor authority");
    }

    #[test]
    fn spawn_waits_until_descriptor_acquisition_finishes() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let acquisition = barrier
            .try_enter_descriptor_acquisition()
            .expect("initial acquisition starts");
        let worker_barrier = barrier.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            started_tx.send(()).expect("announce worker");
            let _spawn = worker_barrier.enter_spawn();
            acquired_tx.send(()).expect("announce permit");
        });

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker starts");
        assert!(
            acquired_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "spawn must remain blocked during descriptor acquisition"
        );

        drop(acquisition);
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("spawn proceeds after acquisition");
        worker.join().expect("worker exits");
    }

    #[test]
    fn queued_spawn_prevents_new_descriptor_receipts() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let receipt = barrier
            .try_enter_descriptor_receipt()
            .expect("initial receipt starts");
        let worker_barrier = barrier.clone();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _spawn = worker_barrier.enter_spawn();
            acquired_tx.send(()).expect("announce permit");
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while barrier.announced_spawns_for_test() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "spawn intent must become observable"
            );
            thread::yield_now();
        }
        assert_eq!(
            barrier
                .try_enter_descriptor_receipt()
                .expect_err("queued spawn must close the receipt gate")
                .active_spawns(),
            1
        );
        assert!(
            acquired_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "spawn remains queued behind the original receipt"
        );

        drop(receipt);
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("queued spawn proceeds after the receipt");
        worker.join().expect("worker exits");
    }

    #[test]
    fn nested_spawn_permits_are_non_blocking_and_counted() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let outer = barrier.enter_spawn();
        let inner = barrier.enter_spawn();

        assert_eq!(
            barrier
                .try_enter_descriptor_receipt()
                .expect_err("both permits block receipt")
                .active_spawns(),
            2
        );
        drop(inner);
        assert_eq!(
            barrier
                .try_enter_descriptor_receipt()
                .expect_err("outer permit still blocks receipt")
                .active_spawns(),
            1
        );
        drop(outer);
        barrier
            .try_enter_descriptor_receipt()
            .expect("receipt succeeds after both nested permits drop");
    }

    #[test]
    fn command_spawn_helper_releases_the_permit_before_returning() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();

        let result = barrier.with_spawn_permit(|| {
            assert_eq!(
                barrier
                    .try_enter_descriptor_acquisition()
                    .expect_err("spawn operation must hold the shared permit")
                    .active_spawns(),
                1
            );
            "spawned"
        });

        assert_eq!(result, "spawned");
        barrier
            .try_enter_descriptor_acquisition()
            .expect("spawn helper must release its permit before returning");
    }

    #[test]
    fn guards_can_cross_thread_boundaries() {
        fn assert_send<T: Send>() {}

        assert_send::<ProcessSpawnPermit>();
        assert_send::<DescriptorAcquisitionGuard>();
    }
}
