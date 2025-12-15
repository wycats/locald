use async_trait::async_trait;

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
    /// Find the service name and port associated with a given domain.
    ///
    /// Returns `Some((service_name, port))` if a service is running and claims the domain,
    /// or `None` if no service matches.
    async fn resolve_service_by_domain(&self, domain: &str) -> Option<(String, u16)>;

    /// Update the port the HTTP proxy is bound to.
    ///
    /// This allows the Manager to know where the Proxy is listening, which is
    /// useful for generating self-referential URLs or status reports.
    async fn set_http_port(&self, port: Option<u16>);

    /// Update the port the HTTPS proxy is bound to.
    async fn set_https_port(&self, port: Option<u16>);
}
