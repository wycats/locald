use std::fmt;
use std::sync::Arc;

use thiserror::Error;

/// Failure to establish or retain wake observation for publisher renewal.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WakeError {
    /// No conforming platform wake monitor is active.
    #[error("publisher wake monitoring is unavailable")]
    Unavailable,
    /// An active monitor failed and can no longer establish wake ordering.
    #[error("publisher wake monitoring failed: {0}")]
    Failed(String),
}

/// Receiver registered by the client-owned lease supervisor.
pub trait WakeSink: Send + Sync + fmt::Debug {
    /// Notify the supervisor that the system resumed.
    fn resumed(&self);
    /// Fail the supervisor closed when wake observation becomes unreliable.
    fn failed(&self, error: WakeError);
}

/// Keeps one wake subscription active until dropped.
pub trait WakeRegistration: Send + fmt::Debug {}

/// Injectable source of system wake events.
///
/// The supported client owns the reaction and renewal ordering. Platform code
/// owns only observation and reports events through the registered sink.
pub trait WakeMonitor: Send + Sync + fmt::Debug {
    /// Register one lease supervisor.
    ///
    /// # Errors
    ///
    /// Returns [`WakeError`] when wake observation cannot be established.
    fn register(&self, sink: Arc<dyn WakeSink>) -> Result<Box<dyn WakeRegistration>, WakeError>;
}

/// Deliberately inactive monitor used while production publisher transport is
/// not advertised. Acquisition fails before a lease can depend on it.
#[derive(Debug, Clone, Copy, Default)]
pub struct InactiveWakeMonitor;

impl WakeMonitor for InactiveWakeMonitor {
    fn register(&self, _sink: Arc<dyn WakeSink>) -> Result<Box<dyn WakeRegistration>, WakeError> {
        Err(WakeError::Unavailable)
    }
}
