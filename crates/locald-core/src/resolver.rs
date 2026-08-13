use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::watch;

use crate::availability::ProjectAvailabilityStatus;
use crate::ipc::PublicationStatus;
use crate::state::ServiceState;

#[derive(Debug, Clone)]
pub enum DomainResolution {
    /// A claim with a concrete runtime service target.
    Service {
        name: String,
        port: Option<u16>,
        status: ServiceState,
        /// Daemon-local identity for the current service controller.
        ///
        /// A replacement controller receives a new generation even when it
        /// reuses the service's sticky internal port.
        runtime_generation: u64,
    },
    /// A durable published-service identity that currently has no admitted
    /// external endpoint lease. The proxy preserves the semantic origin and
    /// renders the declaration's truthful availability guidance.
    PublishedUnavailable {
        name: String,
        publication: PublicationStatus,
    },
    /// A published binding whose exact declaration, health policy, traffic
    /// scope, and lease deadline authorized this route selection.
    PublishedReady {
        name: String,
        publication: PublicationStatus,
        route: PublishedRoute,
    },
    /// A persisted project claim that cannot currently be mapped to loaded
    /// service context. The domain remains owned and must not fall through to a
    /// platform surface or an unknown-domain response.
    OwnershipOnly,
}

/// One request-scoped capability for an exact healthy published binding.
///
/// The opaque listener guard keeps the kernel binding alive through delayed
/// connect and upstream I/O. Cancellation is monotonic and is raised whenever
/// that exact traffic scope loses authority.
#[derive(Clone)]
pub struct PublishedRoute {
    pub port: u16,
    pub binding_revision: u64,
    pub traffic_scope_revision: u64,
    pub semantic_origin: String,
    pub cancellation: watch::Receiver<bool>,
    pub capability_guard: Arc<dyn Send + Sync>,
}

impl std::fmt::Debug for PublishedRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublishedRoute")
            .field("port", &self.port)
            .field("binding_revision", &self.binding_revision)
            .field("traffic_scope_revision", &self.traffic_scope_revision)
            .field("semantic_origin", &self.semantic_origin)
            .field("capability_guard", &"<retained>")
            .finish_non_exhaustive()
    }
}

/// Abstraction layer between the "World State" (Manager) and the "Gateway" (Proxy).
///
/// This trait allows the Proxy to resolve service locations (ports) without knowing
/// the implementation details of how services are managed or where they are running.
///
/// # Thread Safety
/// All methods are `async` and non-blocking. Implementations must ensure that
/// internal state updates are thread-safe (e.g., using `tokio::sync::Mutex`).
#[async_trait]
pub trait ServiceResolver: Send + Sync + std::fmt::Debug {
    /// Find the service associated with a given domain.
    ///
    /// Returns `Some(DomainResolution)` for project-service ownership, including
    /// ownership-only claims. Platform-owned domains return `None` so the proxy
    /// can serve the corresponding platform surface; unclaimed domains also
    /// return `None`.
    async fn resolve_service_by_domain(&self, domain: &str) -> Option<DomainResolution>;

    /// Return the authoritative project lifecycle status for one owned domain.
    ///
    /// The proxy requests this only while rendering an unavailable project
    /// surface, keeping healthy request routing independent of availability IO.
    async fn project_availability_by_domain(
        &self,
        _domain: &str,
    ) -> Option<ProjectAvailabilityStatus> {
        None
    }

    /// Update the port the HTTP proxy is bound to.
    ///
    /// This allows the Manager to know where the Proxy is listening, which is
    /// useful for generating self-referential URLs or status reports.
    async fn set_http_port(&self, port: Option<u16>);

    /// Update the port the HTTPS proxy is bound to.
    async fn set_https_port(&self, port: Option<u16>);
}
