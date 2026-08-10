//! Process-global coordination between process creation and descriptor receipt.
//!
//! macOS does not provide an atomic `MSG_CMSG_CLOEXEC` equivalent for
//! descriptors received with `SCM_RIGHTS`. The publisher transport therefore
//! has to receive a descriptor and set `FD_CLOEXEC` while no other thread can
//! create a child process. Every daemon process-spawn path takes a shared spawn
//! permit, while descriptor receipt takes an exclusive, fail-closed guard.
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

/// Daemon-wide exclusion between process creation and descriptor receipt.
#[derive(Clone)]
pub struct ProcessSpawnBarrier {
    inner: Arc<BarrierInner>,
}

impl ProcessSpawnBarrier {
    /// Return the process-global barrier shared by every locald spawn path.
    #[must_use]
    pub fn global() -> &'static Self {
        GLOBAL_PROCESS_SPAWN_BARRIER.get_or_init(Self::new)
    }

    fn new() -> Self {
        Self {
            inner: Arc::new(BarrierInner {
                state: Mutex::new(BarrierState::default()),
                descriptor_receipts_finished: Condvar::new(),
            }),
        }
    }

    /// Announce a process spawn, then wait until descriptor receipt is idle.
    ///
    /// More than one spawn permit may be active at once. This is intentional:
    /// code that guards an opaque helper which internally spawns may also guard
    /// its own direct spawn without deadlocking.
    #[must_use]
    pub fn enter_spawn(&self) -> ProcessSpawnPermit {
        let mut state = self.lock_state();
        // Publish spawn intent before waiting. Otherwise a stream of new
        // descriptor receipts can repeatedly enter while this spawn is queued,
        // starving every daemon child-process launch.
        state.active_spawns = state.active_spawns.saturating_add(1);
        while state.active_descriptor_receipts != 0 {
            state = self
                .inner
                .descriptor_receipts_finished
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(state);

        ProcessSpawnPermit {
            barrier: self.clone(),
            active: true,
        }
    }

    /// Enter the descriptor-receipt exclusion or fail if a spawn is active.
    ///
    /// Receipt never waits for an in-progress spawn. The caller must reject the
    /// publisher request and let the publisher retry, keeping authentication
    /// and descriptor ownership fail-closed.
    ///
    /// # Errors
    ///
    /// Returns [`DescriptorReceiptBlocked`] when at least one process-spawn
    /// permit is active.
    pub fn try_enter_descriptor_receipt(
        &self,
    ) -> Result<DescriptorReceiptGuard, DescriptorReceiptBlocked> {
        let mut state = self.lock_state();
        if state.active_spawns != 0 {
            return Err(DescriptorReceiptBlocked {
                active_spawns: state.active_spawns,
            });
        }
        state.active_descriptor_receipts = state.active_descriptor_receipts.saturating_add(1);
        drop(state);

        Ok(DescriptorReceiptGuard {
            barrier: self.clone(),
            active: true,
        })
    }

    fn lock_state(&self) -> MutexGuard<'_, BarrierState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    fn isolated_for_test() -> Self {
        Self::new()
    }

    #[cfg(test)]
    fn announced_spawns_for_test(&self) -> usize {
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
                "active_descriptor_receipts",
                &state.active_descriptor_receipts,
            )
            .finish()
    }
}

#[derive(Debug)]
struct BarrierInner {
    state: Mutex<BarrierState>,
    descriptor_receipts_finished: Condvar,
}

#[derive(Debug, Default)]
struct BarrierState {
    active_spawns: usize,
    active_descriptor_receipts: usize,
}

/// Shared permit announcing that a child process may be created.
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

/// Exclusive guard covering `recvmsg` through successful `FD_CLOEXEC` setup.
#[derive(Debug)]
pub struct DescriptorReceiptGuard {
    barrier: ProcessSpawnBarrier,
    active: bool,
}

impl Drop for DescriptorReceiptGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let mut state = self.barrier.lock_state();
        debug_assert!(
            state.active_descriptor_receipts > 0,
            "descriptor receipt guard count underflow"
        );
        state.active_descriptor_receipts = state.active_descriptor_receipts.saturating_sub(1);
        let receipt_is_idle = state.active_descriptor_receipts == 0;
        self.active = false;
        drop(state);

        if receipt_is_idle {
            self.barrier.inner.descriptor_receipts_finished.notify_all();
        }
    }
}

/// Descriptor receipt could not begin because process creation was active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("descriptor receipt blocked by {active_spawns} active process spawn(s)")]
pub struct DescriptorReceiptBlocked {
    active_spawns: usize,
}

impl DescriptorReceiptBlocked {
    /// Number of active process-spawn sections observed by the failed receipt.
    #[must_use]
    pub const fn active_spawns(self) -> usize {
        self.active_spawns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn descriptor_receipt_fails_closed_while_a_spawn_is_active() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let _spawn = barrier.enter_spawn();

        let error = barrier
            .try_enter_descriptor_receipt()
            .expect_err("receipt must fail while a spawn is active");

        assert_eq!(error.active_spawns(), 1);
    }

    #[test]
    fn spawn_waits_until_descriptor_receipt_finishes() {
        let barrier = ProcessSpawnBarrier::isolated_for_test();
        let receipt = barrier
            .try_enter_descriptor_receipt()
            .expect("initial receipt starts");
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
            "spawn must remain blocked during descriptor receipt"
        );

        drop(receipt);
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("spawn proceeds after receipt");
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
        assert_send::<DescriptorReceiptGuard>();
    }
}
