//! Supported Rust client for locald's version-1 published-service protocol.
//!
//! The client owns strict discovery interpretation, origin-ordering typestate,
//! frame construction, conservative renewal scheduling, typed authority loss,
//! and the authenticated one-shot Unix transport used by production
//! publishers.
//!
//! On macOS, every host process using the production transport must hold a
//! [`ProcessSpawnPermit`] around each operation that can spawn or exec. The
//! transport takes the opposite side of the same process-global
//! [`ProcessSpawnBarrier`] across every descriptor acquisition until the new
//! descriptor is either close-on-exec or closed. Acquire the permit immediately
//! before `Command::spawn`, drop it when `spawn` returns, and wait for the child
//! separately.

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
    UnixCommandSocketDiscovery, UnixPublisherTransport,
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
    InstallationError, InstallationEvidence, InstalledPublisher, SandboxProbeError,
    SandboxPublisherContext, probe_installation, probe_sandbox_publisher,
};
pub use supervisor::{LeaseSnapshot, LeaseState};
pub use wake::{
    InactiveWakeMonitor, SystemWakeMonitor, WakeError, WakeMonitor, WakeRegistration, WakeSink,
};

pub use locald_utils::process_spawn::{ProcessSpawnBarrier, ProcessSpawnPermit};

pub use locald_publisher_protocol as protocol;
