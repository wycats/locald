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
        drop(state);
        self.active = false;
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

/// Descriptor acquisition could not begin because process creation was active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("descriptor acquisition blocked by {active_spawns} active process spawn(s)")]
pub struct DescriptorAcquisitionBlocked {
    active_spawns: usize,
}

impl DescriptorAcquisitionBlocked {
    /// Number of active process-spawn sections observed by the failed acquisition.
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
    fn guards_can_cross_thread_boundaries() {
        fn assert_send<T: Send>() {}

        assert_send::<ProcessSpawnPermit>();
        assert_send::<DescriptorAcquisitionGuard>();
    }
}
