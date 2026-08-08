//! Supported Rust client for locald's version-1 published-service protocol.
//!
//! The client owns strict discovery interpretation, origin-ordering typestate,
//! frame construction, conservative renewal scheduling, and typed authority
//! loss. Socket authentication and descriptor transfer are injected backend
//! boundaries in this slice; the dedicated Unix transport activates later.

mod backend;
#[allow(
    clippy::redundant_pub_crate,
    reason = "sibling-only client authority helpers intentionally use pub(super)"
)]
mod client;
mod clock;
mod installation;
#[allow(
    clippy::redundant_pub_crate,
    reason = "the private supervisor module exposes only a pub(super) sibling boundary"
)]
mod supervisor;
mod wake;

pub use backend::{
    AuthenticatedDaemonDiscovery, AuthenticatedValue, BackendError, BackendErrorKind,
    DeliveryCertainty, PublisherTransport, TransportFailure, TransportReply,
    UnixCommandSocketDiscovery,
};
pub use client::{
    ClientError, InstalledOrigin, Lease, OriginInstallError, PreparedPublication, PreparedRebind,
    ProjectPublisher, PublisherClient, Reacquisition, ReacquisitionReason, RebindInstalledOrigin,
    TerminalPreparedPublication, TerminalPreparedRebind, WaitOutcome,
};
pub use clock::{
    ClockError, RenewalSchedule, SuspendAwareClock, SuspendInstant, SystemSuspendAwareClock,
};
pub use installation::{
    InstallationError, InstallationEvidence, InstalledPublisher, probe_installation,
};
pub use supervisor::{LeaseSnapshot, LeaseState};
pub use wake::{InactiveWakeMonitor, WakeError, WakeMonitor, WakeRegistration, WakeSink};

pub use locald_publisher_protocol as protocol;
