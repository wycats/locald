use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    handler::Handler,
    http::{HeaderName, HeaderValue, Method, Uri},
    response::{IntoResponse, Response},
};
use axum_server::tls_rustls::RustlsConfig;
use http_body_util::{BodyExt as _, StreamBody};
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;
use tower::ServiceExt;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

use crate::assets;
use locald_core::ipc::{PublicationState, PublicationStatus};
use locald_core::resolver::{DomainResolution, ServiceResolver};
use locald_core::state::ServiceState;
use locald_core::{DomainName, ProjectAvailabilityStatus, ProjectLifecycleState};
use locald_utils::cert::CertManager;

const RESPONSIVE_BACKEND_TTL: Duration = Duration::from_mins(1);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ResponsiveBackend {
    port: u16,
    runtime_generation: u64,
}

#[derive(Clone, Debug, Default)]
struct ResponsiveBackends {
    entries: Arc<Mutex<HashMap<ResponsiveBackend, Instant>>>,
}

impl ResponsiveBackends {
    fn observe(&self, backend: ResponsiveBackend) -> Option<Instant> {
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, expires_at| *expires_at > now);
        entries.get(&backend).copied()
    }

    fn mark(&self, backend: ResponsiveBackend) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(backend, Instant::now() + RESPONSIVE_BACKEND_TTL);
    }

    fn forget_if_unchanged(&self, backend: ResponsiveBackend, observed: Option<Instant>) {
        let Some(observed) = observed else {
            return;
        };

        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.get(&backend) == Some(&observed) {
            entries.remove(&backend);
        }
    }
}

/// Manages the reverse proxy for routing requests to services.
///
/// The `ProxyManager` handles:
/// - HTTP and HTTPS binding.
/// - Routing based on Host header.
/// - WebSocket upgrades.
/// - Serving the dashboard and docs.
#[derive(Debug)]
pub struct ProxyManager {
    resolver: Arc<dyn ServiceResolver>,
    api_router: Router,
    cert_manager: Option<Arc<CertManager>>,
    responsive_backends: ResponsiveBackends,
}

impl ProxyManager {
    /// Create a new `ProxyManager`.
    ///
    /// # Arguments
    ///
    /// * `resolver` - Service resolver to find ports for domains.
    /// * `api_router` - Router for the internal API (`/api`).
    /// * `cert_manager` - Optional certificate manager for HTTPS.
    pub fn new(
        resolver: Arc<dyn ServiceResolver>,
        api_router: Router,
        cert_manager: Option<Arc<CertManager>>,
    ) -> Self {
        Self {
            resolver,
            api_router,
            cert_manager,
            responsive_backends: ResponsiveBackends::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn make_app(&self) -> Router {
        self.make_app_for_scheme(true)
    }

    pub(crate) fn make_app_for_scheme(&self, is_secure: bool) -> Router {
        let mut connector = hyper_util::client::legacy::connect::HttpConnector::new();
        connector.set_nodelay(true);
        let keepalive_secs = 60;
        connector.set_keepalive(Some(std::time::Duration::from_secs(keepalive_secs)));

        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector);

        let state = AppState {
            resolver: self.resolver.clone(),
            client,
            api_router: Router::new().nest("/api", self.api_router.clone()),
            responsive_backends: self.responsive_backends.clone(),
            is_secure,
        };

        Router::new()
            .fallback_service(handle_proxy.with_state(state))
            .layer(TraceLayer::new_for_http())
    }

    pub async fn bind_http(&self, port: u16) -> anyhow::Result<TcpListener> {
        let listener = if port != 0 && port < 1024 {
            // Privileged port: request from platform helper.
            // macOS: XPC to com.locald.helper. Linux: locald-shim via SCM_RIGHTS.
            #[cfg(target_os = "macos")]
            let l = crate::helper_client::bind_privileged_port(port).await?;
            #[cfg(not(target_os = "macos"))]
            let l = crate::shim_client::bind_privileged_port(port).await?;
            l.set_nonblocking(true)?;
            TcpListener::from_std(l)?
        } else {
            let addr = SocketAddr::from(([0, 0, 0, 0], port));
            TcpListener::bind(addr).await?
        };

        let addr = listener.local_addr()?;
        info!("Proxy bound to http://{addr}");
        Ok(listener)
    }

    pub async fn serve_http(&self, listener: TcpListener) -> anyhow::Result<()> {
        let app = self.make_app_for_scheme(false);
        axum::serve(listener, app).await?;
        Ok(())
    }

    pub async fn bind_https(&self, port: u16) -> anyhow::Result<std::net::TcpListener> {
        let listener = if port != 0 && port < 1024 {
            #[cfg(target_os = "macos")]
            let l = crate::helper_client::bind_privileged_port(port).await?;
            #[cfg(not(target_os = "macos"))]
            let l = crate::shim_client::bind_privileged_port(port).await?;
            l
        } else {
            let addr = SocketAddr::from(([0, 0, 0, 0], port));
            tokio::net::TcpListener::bind(addr).await?.into_std()?
        };

        listener.set_nonblocking(true)?;
        Ok(listener)
    }

    pub async fn serve_https(&self, listener: std::net::TcpListener) -> anyhow::Result<()> {
        let Some(cert_manager) = &self.cert_manager else {
            return Ok(());
        };

        // Note: advertised port is set in lib.rs, not here.
        if let Ok(addr) = listener.local_addr() {
            info!("Proxy bound to https://{addr}");
        }

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(cert_manager.clone());

        let rustls_config = RustlsConfig::from_config(Arc::new(config));
        let app = self.make_app_for_scheme(true);

        axum_server::from_tcp_rustls(listener, rustls_config)
            .serve(app.into_make_service())
            .await?;
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    resolver: Arc<dyn ServiceResolver>,
    client: hyper_util::client::legacy::Client<
        hyper_util::client::legacy::connect::HttpConnector,
        Body,
    >,
    api_router: Router,
    responsive_backends: ResponsiveBackends,
    is_secure: bool,
}

async fn handle_websocket_upgrade(state: AppState, mut req: Request, backend_uri: Uri) -> Response {
    let mut backend_req_builder = Request::builder()
        .uri(backend_uri)
        .method(req.method().clone());

    if let Some(headers) = backend_req_builder.headers_mut() {
        *headers = req.headers().clone();
    }

    let backend_req = match backend_req_builder.body(Body::empty()) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to build backend request: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build backend request",
            )
                .into_response();
        }
    };

    let client_upgrade_fut = hyper::upgrade::on(&mut req);

    match state.client.request(backend_req).await {
        Ok(mut backend_response) => {
            if backend_response.status() == StatusCode::SWITCHING_PROTOCOLS {
                let backend_upgrade_fut = hyper::upgrade::on(&mut backend_response);

                tokio::spawn(async move {
                    match tokio::try_join!(client_upgrade_fut, backend_upgrade_fut) {
                        Ok((client_io, backend_io)) => {
                            let mut client_io = TokioIo::new(client_io);
                            let mut backend_io = TokioIo::new(backend_io);

                            if let Err(e) =
                                copy_bidirectional(&mut client_io, &mut backend_io).await
                            {
                                error!("WebSocket bridge error: {e}");
                            }
                        }
                        Err(e) => error!("WebSocket upgrade error: {e}"),
                    }
                });

                let mut res = Response::new(Body::empty());
                *res.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                *res.headers_mut() = backend_response.headers().clone();
                res
            } else {
                backend_response.into_response()
            }
        }
        Err(e) => {
            error!("Proxy error: {e}");
            error_response(StatusCode::BAD_GATEWAY, format!("Proxy error: {e}"))
        }
    }
}

async fn handle_proxy(State(state): State<AppState>, req: Request) -> Response {
    let raw_host = match req.headers().get("host") {
        Some(h) => h
            .to_str()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string(),
        None => return (StatusCode::BAD_REQUEST, "Missing Host header").into_response(),
    };
    let host = match raw_host.parse::<DomainName>() {
        Ok(host) => host.to_string(),
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };

    // Project services override platform fallbacks so locald can develop its
    // own dashboard and docs through the same managed-domain workflow.
    let resolution = state.resolver.resolve_service_by_domain(&host).await;
    if let Some(resolution) = resolution {
        if let DomainResolution::PublishedUnavailable { publication, .. }
        | DomainResolution::PublishedReady { publication, .. } = &resolution
        {
            if let Some(response) = published_alias_redirect(req.uri(), &host, publication) {
                return response;
            }
            if !state.is_secure {
                return published_https_redirect(req.uri(), publication);
            }
        }
        if domain_resolution_supports_resume(&resolution) && is_resume_api_request(&req) {
            return route_locald_api(&state, req).await;
        }
        return proxy_to_domain_resolution(&state, req, &host, resolution).await;
    }

    if is_dashboard_host(&host) && is_api_request(&req) {
        return route_locald_api(&state, req).await;
    }

    // Dev UI fallback: support standalone Vite/Astro development when no
    // locald-managed project currently owns the dev domain.
    if dev_ui_enabled() {
        if host == "dev.locald.localhost" || host == "dev.locald.local" {
            let port = dev_ui_port("LOCALD_DASHBOARD_DEV_PORT", 5173);
            return proxy_to_local_port(state, req, port, "dashboard").await;
        }

        if host == "dev.docs.localhost" || host == "dev.docs.local" {
            let port = dev_ui_port("LOCALD_DOCS_DEV_PORT", 4321);
            return proxy_to_local_port(state, req, port, "docs").await;
        }
    }

    if host == "docs.localhost" || host == "docs.local" {
        return assets::handle_docs(req.uri()).into_response();
    }

    // Fallback to embedded dashboard if no service claims the domain
    if host == "locald.localhost" || host == "locald.local" || host == "localhost" {
        return assets::handle_dashboard(req.uri()).into_response();
    }

    (StatusCode::NOT_FOUND, format!("Domain {host} not found")).into_response()
}

fn domain_resolution_supports_resume(resolution: &DomainResolution) -> bool {
    match resolution {
        DomainResolution::Service { port, .. } => port.is_none(),
        DomainResolution::PublishedUnavailable { publication, .. } => {
            publication.state == PublicationState::RoutePaused
        }
        DomainResolution::PublishedReady { .. } => false,
        DomainResolution::OwnershipOnly => true,
    }
}

fn is_resume_api_request(req: &Request) -> bool {
    req.method() == Method::POST && req.uri().path() == "/api/projects/resume-domain"
}

fn is_api_request(req: &Request) -> bool {
    let path = req.uri().path();
    path == "/api" || path.starts_with("/api/")
}

fn is_dashboard_host(host: &str) -> bool {
    matches!(host, "locald.localhost" | "locald.local" | "localhost")
        || (dev_ui_enabled() && matches!(host, "dev.locald.localhost" | "dev.locald.local"))
}

async fn route_locald_api(state: &AppState, req: Request) -> Response {
    state
        .api_router
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|error| match error {})
}

async fn proxy_to_domain_resolution(
    state: &AppState,
    mut req: Request,
    host: &str,
    resolution: DomainResolution,
) -> Response {
    if let DomainResolution::PublishedUnavailable { name, publication } = &resolution {
        return published_service_response(host, name, publication);
    }
    if let DomainResolution::PublishedReady {
        name,
        publication: _,
        route,
    } = resolution
    {
        return proxy_to_published(req, &name, route).await;
    }
    let DomainResolution::Service {
        name: service_name,
        port,
        status,
        runtime_generation,
    } = resolution
    else {
        let availability = state.resolver.project_availability_by_domain(host).await;
        return unavailable_project_response(
            host,
            None,
            "locald has preserved this project domain, but its service mapping is not currently loaded.",
            availability.as_ref(),
        );
    };

    if port.is_none() {
        if matches!(status, ServiceState::Building) {
            return loading_response(&service_name);
        }

        let availability = state.resolver.project_availability_by_domain(host).await;
        return unavailable_project_response(
            host,
            Some(&service_name),
            "The project is known to locald, but this service is not currently available.",
            availability.as_ref(),
        );
    }

    let port = port.unwrap_or_default();
    let backend = ResponsiveBackend {
        port,
        runtime_generation,
    };
    let uri_string = format!(
        "http://localhost:{port}{}",
        req.uri().path_and_query().map_or("", |x| x.as_str())
    );
    let uri: Uri = match uri_string.parse() {
        Ok(u) => u,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid URI").into_response(),
    };

    tracing::debug!("Proxying to: {}", uri);

    // Check for WebSocket upgrade
    if req
        .headers()
        .get(hyper::header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
    {
        return handle_websocket_upgrade(state.clone(), req, uri).await;
    }

    let header_passthrough = req.headers().get("x-locald-passthrough").is_some();
    let accepts_html = req
        .headers()
        .get(hyper::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);
    let responsive_observation = state.responsive_backends.observe(backend);
    let backend_is_responsive = accepts_html && responsive_observation.is_some();
    let is_passthrough = header_passthrough || backend_is_responsive;

    *req.uri_mut() = uri;

    let backend_future = state.client.request(req);

    if is_passthrough || !accepts_html {
        return match backend_future.await {
            Ok(res) => {
                if header_passthrough {
                    state.responsive_backends.mark(backend);
                }
                res.into_response()
            }
            Err(e) => {
                error!("Proxy error: {e}");
                state
                    .responsive_backends
                    .forget_if_unchanged(backend, responsive_observation);
                error_response(StatusCode::BAD_GATEWAY, format!("Proxy error: {e}"))
            }
        };
    }

    tokio::select! {
        res = backend_future => {
            match res {
                Ok(res) => res.into_response(),
                Err(e) => {
                    error!("Proxy error: {e}");
                    error_response(StatusCode::BAD_GATEWAY, format!("Proxy error: {e}"))
                }
            }
        }
        () = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
            loading_response(&service_name)
        }
    }
}

async fn proxy_to_local_port(
    state: AppState,
    mut req: Request,
    port: u16,
    label: &str,
) -> Response {
    let uri_string = format!(
        "http://127.0.0.1:{port}{}",
        req.uri().path_and_query().map_or("", |x| x.as_str())
    );
    let uri: Uri = match uri_string.parse() {
        Ok(u) => u,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid URI").into_response(),
    };

    // WebSocket upgrade (needed for Vite HMR)
    if req
        .headers()
        .get(hyper::header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
    {
        return handle_websocket_upgrade(state, req, uri).await;
    }

    *req.uri_mut() = uri;
    match state.client.request(req).await {
        Ok(res) => res.into_response(),
        Err(e) => error_response(
            StatusCode::BAD_GATEWAY,
            format!("Dev {label} proxy failed: {e}. Is the dev server running on port {port}?"),
        ),
    }
}

async fn proxy_to_published(
    mut req: Request,
    service_name: &str,
    route: locald_core::resolver::PublishedRoute,
) -> Response {
    if let Err(response) = canonicalize_published_headers(&mut req, &route.semantic_origin) {
        return *response;
    }
    let uri_string = format!(
        "http://127.0.0.1:{}{}",
        route.port,
        req.uri()
            .path_and_query()
            .map_or("/", axum::http::uri::PathAndQuery::as_str)
    );
    let uri: Uri = match uri_string.parse() {
        Ok(uri) => uri,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid URI").into_response(),
    };
    *req.uri_mut() = uri;

    let mut connector = hyper_util::client::legacy::connect::HttpConnector::new();
    connector.set_nodelay(true);
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .pool_max_idle_per_host(0)
        .build(connector);

    if req
        .headers()
        .get(hyper::header::UPGRADE)
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"))
    {
        return proxy_published_websocket(req, client, route).await;
    }

    let mut cancellation = route.cancellation.clone();
    let request = client.request(req);
    let response = tokio::select! {
        biased;
        () = wait_for_route_cancellation(&mut cancellation) => {
            return published_service_cancelled(service_name);
        }
        result = request => match result {
            Ok(response) => response,
            Err(error) => {
                error!("Published proxy error: {error}");
                return error_response(StatusCode::BAD_GATEWAY, format!("Proxy error: {error}"));
            }
        }
    };
    cancellable_published_response(response, client, route)
}

type PublishedClient =
    hyper_util::client::legacy::Client<hyper_util::client::legacy::connect::HttpConnector, Body>;

async fn proxy_published_websocket(
    mut request: Request,
    client: PublishedClient,
    route: locald_core::resolver::PublishedRoute,
) -> Response {
    let client_upgrade = hyper::upgrade::on(&mut request);
    let mut cancellation = route.cancellation.clone();
    let mut backend_response = tokio::select! {
        biased;
        () = wait_for_route_cancellation(&mut cancellation) => {
            return published_service_cancelled("published service");
        }
        result = client.request(request) => match result {
            Ok(response) => response,
            Err(error) => return error_response(StatusCode::BAD_GATEWAY, format!("Proxy error: {error}")),
        }
    };
    if backend_response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return cancellable_published_response(backend_response, client, route);
    }

    let backend_upgrade = hyper::upgrade::on(&mut backend_response);
    let headers = backend_response.headers().clone();
    tokio::spawn(async move {
        let _capability_guard = route.capability_guard;
        let _client = client;
        tokio::select! {
            biased;
            () = wait_for_route_cancellation(&mut cancellation) => {}
            result = async {
                let (client_io, backend_io) = tokio::try_join!(client_upgrade, backend_upgrade)?;
                let mut client_io = TokioIo::new(client_io);
                let mut backend_io = TokioIo::new(backend_io);
                copy_bidirectional(&mut client_io, &mut backend_io).await?;
                Ok::<(), anyhow::Error>(())
            } => {
                if let Err(error) = result {
                    error!("Published WebSocket bridge error: {error}");
                }
            }
        }
    });
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    *response.headers_mut() = headers;
    response
}

fn cancellable_published_response(
    response: hyper::Response<hyper::body::Incoming>,
    client: PublishedClient,
    route: locald_core::resolver::PublishedRoute,
) -> Response {
    let (parts, mut upstream) = response.into_parts();
    let mut cancellation = route.cancellation.clone();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        let _capability_guard = route.capability_guard;
        let _client = client;
        loop {
            let frame = tokio::select! {
                biased;
                () = wait_for_route_cancellation(&mut cancellation) => return,
                frame = upstream.frame() => match frame {
                    Some(frame) => frame,
                    None => return,
                }
            };
            tokio::select! {
                biased;
                () = wait_for_route_cancellation(&mut cancellation) => return,
                sent = sender.send(frame) => if sent.is_err() { return; },
            }
        }
    });
    let body = StreamBody::new(tokio_stream::wrappers::ReceiverStream::new(receiver));
    Response::from_parts(parts, Body::new(body))
}

async fn wait_for_route_cancellation(cancellation: &mut tokio::sync::watch::Receiver<bool>) {
    while !*cancellation.borrow() {
        if cancellation.changed().await.is_err() {
            return;
        }
    }
}

fn published_service_cancelled(service_name: &str) -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        format!("Published route for {service_name} is no longer authorized"),
    )
}

fn canonicalize_published_headers(
    request: &mut Request,
    semantic_origin: &str,
) -> Result<(), Box<Response>> {
    let origin: Uri = semantic_origin.parse().map_err(|_| {
        Box::new((StatusCode::INTERNAL_SERVER_ERROR, "Invalid semantic origin").into_response())
    })?;
    let authority = origin
        .authority()
        .map(axum::http::uri::Authority::as_str)
        .ok_or_else(|| {
            Box::new((StatusCode::INTERNAL_SERVER_ERROR, "Invalid semantic origin").into_response())
        })?;
    let port = origin.port_u16().unwrap_or(443);
    let remove = request
        .headers()
        .keys()
        .filter(|name| {
            name.as_str().eq_ignore_ascii_case("forwarded")
                || name
                    .as_str()
                    .to_ascii_lowercase()
                    .starts_with("x-forwarded-")
                || name.as_str().eq_ignore_ascii_case("x-real-ip")
        })
        .cloned()
        .collect::<Vec<HeaderName>>();
    for name in remove {
        request.headers_mut().remove(name);
    }
    let headers = request.headers_mut();
    headers.insert(
        hyper::header::HOST,
        HeaderValue::from_str(authority).map_err(|_| {
            Box::new(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Invalid semantic authority",
                )
                    .into_response(),
            )
        })?,
    );
    for (name, value) in [
        (
            "forwarded",
            format!("for=127.0.0.1;host=\"{authority}\";proto=https"),
        ),
        ("x-forwarded-for", "127.0.0.1".to_owned()),
        ("x-forwarded-host", authority.to_owned()),
        ("x-forwarded-proto", "https".to_owned()),
        ("x-forwarded-port", port.to_string()),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_str(&value).map_err(|_| {
                Box::new(
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Invalid forwarding context",
                    )
                        .into_response(),
                )
            })?,
        );
    }
    Ok(())
}

fn dev_ui_enabled() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }

    std::env::var("LOCALD_DEV_UI").is_ok_and(|v| v != "0" && v.to_lowercase() != "false")
}

fn dev_ui_port(var: &str, default: u16) -> u16 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(default)
}

fn error_response(status: StatusCode, message: impl std::fmt::Display) -> Response {
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Proxy Error - locald</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background-color: #f9fafb;
            color: #1f2937;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
        }}
        .container {{
            background: white;
            padding: 2rem;
            border-radius: 8px;
            box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
            max-width: 500px;
            width: 100%;
            text-align: center;
        }}
        h1 {{
            color: #dc2626;
            margin-bottom: 1rem;
            font-size: 1.5rem;
        }}
        p {{
            margin-bottom: 1.5rem;
            line-height: 1.5;
        }}
        .error-details {{
            background-color: #f3f4f6;
            padding: 1rem;
            border-radius: 4px;
            font-family: monospace;
            font-size: 0.875rem;
            color: #374151;
            overflow-x: auto;
            margin-bottom: 1.5rem;
            text-align: left;
        }}
        .btn {{
            display: inline-block;
            background-color: #2563eb;
            color: white;
            padding: 0.5rem 1rem;
            border-radius: 4px;
            text-decoration: none;
            font-weight: 500;
            transition: background-color 0.2s;
        }}
        .btn:hover {{
            background-color: #1d4ed8;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>Proxy Error</h1>
        <p>locald could not connect to the upstream service.</p>
        <div class="error-details">
            {message}
        </div>
        <a href="javascript:window.location.reload()" class="btn">Retry</a>
    </div>
</body>
</html>"#
    );

    (
        status,
        [(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn published_alias_redirect(
    request_uri: &Uri,
    requested_host: &str,
    publication: &PublicationStatus,
) -> Option<Response> {
    let canonical_origin = publication.origin.parse::<Uri>().ok()?;
    let canonical_host = canonical_origin.host()?;
    if canonical_host.eq_ignore_ascii_case(requested_host) {
        return None;
    }

    let path_and_query = request_uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);
    let location = format!("{}{path_and_query}", publication.origin);
    Some(permanent_redirect(&location))
}

fn published_https_redirect(request_uri: &Uri, publication: &PublicationStatus) -> Response {
    let path_and_query = request_uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);
    permanent_redirect(&format!("{}{path_and_query}", publication.origin))
}

fn permanent_redirect(location: &str) -> Response {
    HeaderValue::from_str(location).map_or_else(
        |_| (StatusCode::INTERNAL_SERVER_ERROR, "Invalid redirect target").into_response(),
        |location| {
            (
                StatusCode::PERMANENT_REDIRECT,
                [(hyper::header::LOCATION, location)],
            )
                .into_response()
        },
    )
}

fn published_service_response(
    host: &str,
    service_name: &str,
    publication: &PublicationStatus,
) -> Response {
    let status_label = match publication.state {
        PublicationState::WaitingForPublisher => "Waiting for publisher",
        PublicationState::CheckingEndpoint => "Checking endpoint",
        PublicationState::EndpointUnhealthy => "Endpoint unhealthy",
        PublicationState::Ready => "Ready",
        PublicationState::RoutePaused => "Route paused",
        PublicationState::InstanceMissing => "Worktree missing",
    };
    let default_next_step = match publication.state {
        PublicationState::WaitingForPublisher => {
            "Start this service with the workflow that owns its external runtime."
        }
        PublicationState::CheckingEndpoint => {
            "Wait for the publisher's endpoint health check to finish."
        }
        PublicationState::EndpointUnhealthy => {
            "Inspect the owning workflow and its endpoint health."
        }
        PublicationState::Ready => "Reload this page.",
        PublicationState::RoutePaused => "Resume the project to allow publication.",
        PublicationState::InstanceMissing => {
            "Restore the worktree, or forget the project if this identity is no longer needed."
        }
    };
    let next_step = publication
        .next_step
        .as_deref()
        .unwrap_or(default_next_step);
    let (resume_action, resume_script) = if publication.state == PublicationState::RoutePaused {
        let domain_js = inline_script_json_string(host);
        (
            r#"<button class="btn" id="resume-btn">Resume project</button>"#.to_owned(),
            format!(
                r"<script>
        const domain = {domain_js};
        const btn = document.getElementById('resume-btn');
        if (btn) {{
            btn.addEventListener('click', async () => {{
                btn.disabled = true;
                btn.textContent = 'Resuming...';
                try {{
                    const res = await fetch('/api/projects/resume-domain', {{
                        method: 'POST',
                        headers: {{ 'Content-Type': 'application/json' }},
                        body: JSON.stringify({{ domain }})
                    }});
                    if (!res.ok) throw new Error(await res.text() || 'Failed to resume project');
                    window.location.reload();
                }} catch (err) {{
                    console.error(err);
                    btn.disabled = false;
                    btn.textContent = 'Resume project';
                }}
            }});
        }}
    </script>"
            ),
        )
    } else {
        (String::new(), String::new())
    };
    let template = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Published service is __STATUS__</title>
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background:#0b0b0f; color:#e4e4e7; display:flex; justify-content:center; align-items:center; min-height:100vh; margin:0; }
        .card { background:#111827; padding:2rem; border:1px solid #1f2937; border-radius:12px; width:min(560px, 90vw); }
        h1 { margin:0 0 .75rem; font-size:1.5rem; }
        p { color:#d4d4d8; line-height:1.5; }
        .hint { color:#a1a1aa; font-size:.9rem; }
        code { background:#0f172a; color:#93c5fd; padding:.15rem .35rem; border-radius:6px; }
        .actions { display:flex; gap:.75rem; margin-top:1.25rem; }
        .btn { background:#2563eb; color:white; padding:.6rem 1.1rem; border:0; border-radius:8px; text-decoration:none; font-weight:600; cursor:pointer; }
        .secondary { background:#1f2937; }
        .btn:disabled { opacity:.6; cursor:not-allowed; }
    </style>
</head>
<body>
    <main class="card">
        <h1>Published service is __STATUS__</h1>
        <p>__EXPLANATION__</p>
        <p class="hint">Service: <code>__SERVICE__</code></p>
        <p class="hint">Stable origin: <code>__ORIGIN__</code></p>
        <p>__NEXT_STEP__</p>
        <div class="actions">
            __RESUME_ACTION__
            <a class="btn secondary" href="https://locald.localhost">Open dashboard</a>
        </div>
    </main>
    __RESUME_SCRIPT__
</body>
</html>"#;
    let escaped_status = escape_html(status_label);
    let escaped_explanation = escape_html(&publication.explanation);
    let escaped_service = escape_html(service_name);
    let escaped_origin = escape_html(&publication.origin);
    let escaped_next_step = escape_html(next_step);
    let html = render_template_once(
        template,
        &[
            ("__STATUS__", escaped_status.as_str()),
            ("__EXPLANATION__", escaped_explanation.as_str()),
            ("__SERVICE__", escaped_service.as_str()),
            ("__ORIGIN__", escaped_origin.as_str()),
            ("__NEXT_STEP__", escaped_next_step.as_str()),
            ("__RESUME_ACTION__", resume_action.as_str()),
            ("__RESUME_SCRIPT__", resume_script.as_str()),
        ],
    );

    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn unavailable_project_response(
    host: &str,
    service_name: Option<&str>,
    fallback_message: &str,
    availability: Option<&ProjectAvailabilityStatus>,
) -> Response {
    let escaped_host = escape_html(host);
    let status_label = availability
        .map(|availability| availability.state.to_string())
        .unwrap_or_else(|| "Not Available".to_owned());
    let message = availability
        .and_then(|availability| availability.last_error.as_deref())
        .or_else(|| {
            availability.and_then(|availability| {
                availability
                    .reasons
                    .first()
                    .map(|reason| reason.message.as_str())
            })
        })
        .unwrap_or(fallback_message);
    let escaped_status = escape_html(&status_label);
    let escaped_message = escape_html(message);
    let always_on_hint = availability
        .filter(|availability| availability.always_on)
        .map_or_else(String::new, |_| {
            r#"<p class="hint policy">Always On remains enabled for this project.</p>"#.to_owned()
        });
    let service_hint = service_name.map_or_else(String::new, |service_name| {
        format!(
            r#"<p class="hint">Service: <code>{}</code></p>"#,
            escape_html(service_name)
        )
    });
    let lifecycle_state = availability.map(|availability| availability.state);
    let can_resume = matches!(
        lifecycle_state,
        None | Some(
            ProjectLifecycleState::Paused
                | ProjectLifecycleState::Stopped
                | ProjectLifecycleState::Failed
                | ProjectLifecycleState::Degraded
                | ProjectLifecycleState::CoolingDown
        )
    );
    let (resume_action, resume_status, resume_script) = if can_resume {
        let domain_js = inline_script_json_string(host);
        (
            r#"<button class="btn" id="resume-btn">Resume project</button>
            <a class="btn secondary" href="https://locald.localhost">Open dashboard</a>"#
                .to_owned(),
            r#"<p class="hint" id="resume-status">You can also run <code>locald up</code> from the project directory.</p>"#
                .to_owned(),
            format!(
                r"<script>
        const domain = {domain_js};
        const btn = document.getElementById('resume-btn');
        const status = document.getElementById('resume-status');

        if (btn) {{
            btn.addEventListener('click', async () => {{
                btn.disabled = true;
                btn.textContent = 'Resuming...';
                if (status) status.textContent = 'Starting the project and waiting for readiness...';
                try {{
                    const res = await fetch('/api/projects/resume-domain', {{
                        method: 'POST',
                        headers: {{ 'Content-Type': 'application/json' }},
                        body: JSON.stringify({{ domain }})
                    }});
                    if (!res.ok) {{
                        const detail = await res.text();
                        throw new Error(detail || 'Failed to resume project');
                    }}
                    window.location.reload();
                }} catch (err) {{
                    console.error(err);
                    btn.disabled = false;
                    btn.textContent = 'Resume project';
                    if (status) {{
                        status.textContent = err instanceof Error
                            ? err.message
                            : 'Could not resume the project. Try `locald up` or use the dashboard.';
                    }}
                }}
            }});
        }}
    </script>"
            ),
        )
    } else {
        let guidance = match lifecycle_state {
            Some(ProjectLifecycleState::Missing) => {
                "Restore the project worktree, then run <code>locald up</code> from that directory."
            }
            Some(ProjectLifecycleState::Starting) => {
                "This project is already starting. Open the dashboard to inspect its service status and logs."
            }
            Some(ProjectLifecycleState::Ready) => {
                "This project is available, but this service does not currently expose a reachable web endpoint. Open the dashboard to inspect its status and logs."
            }
            _ => unreachable!("every non-resumable lifecycle state has guidance"),
        };
        (
            r#"<a class="btn secondary" href="https://locald.localhost">Open dashboard</a>"#
                .to_owned(),
            format!(r#"<p class="hint">{guidance}</p>"#),
            String::new(),
        )
    };
    let template = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Local project is not available</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background-color: #0b0b0f;
            color: #e4e4e7;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
        }
        .container {
            background: #111827;
            padding: 2rem;
            border-radius: 12px;
            border: 1px solid #1f2937;
            max-width: 560px;
            width: 90%;
        }
        h1 {
            margin: 0 0 0.75rem;
            font-size: 1.5rem;
        }
        p {
            margin: 0 0 1rem;
            line-height: 1.5;
            color: #d4d4d8;
        }
        .hint {
            font-size: 0.9rem;
            color: #a1a1aa;
        }
        .actions {
            display: flex;
            gap: 0.75rem;
            flex-wrap: wrap;
            margin-top: 1.25rem;
        }
        .btn {
            display: inline-block;
            background-color: #2563eb;
            color: white;
            padding: 0.6rem 1.1rem;
            border-radius: 8px;
            text-decoration: none;
            font-weight: 600;
            border: none;
            cursor: pointer;
        }
        .btn.secondary {
            background-color: #1f2937;
            color: #e4e4e7;
        }
        .btn:disabled {
            opacity: 0.6;
            cursor: not-allowed;
        }
        code {
            background: #0f172a;
            padding: 0.15rem 0.35rem;
            border-radius: 6px;
            color: #93c5fd;
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>Project is __STATUS__</h1>
        <p>__MESSAGE__</p>
        <p class="hint">Domain: <code>__SERVICE_HOST__</code></p>
        __SERVICE_HINT__
        __ALWAYS_ON_HINT__
        <div class="actions">
            __RESUME_ACTION__
        </div>
        __RESUME_STATUS__
    </div>
    __RESUME_SCRIPT__
</body>
</html>"#;

    let html = render_template_once(
        template,
        &[
            ("__STATUS__", escaped_status.as_str()),
            ("__MESSAGE__", escaped_message.as_str()),
            ("__SERVICE_HOST__", escaped_host.as_str()),
            ("__SERVICE_HINT__", service_hint.as_str()),
            ("__ALWAYS_ON_HINT__", always_on_hint.as_str()),
            ("__RESUME_ACTION__", resume_action.as_str()),
            ("__RESUME_STATUS__", resume_status.as_str()),
            ("__RESUME_SCRIPT__", resume_script.as_str()),
        ],
    );

    (
        StatusCode::SERVICE_UNAVAILABLE,
        [("content-type", "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn render_template_once(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut remaining = template;

    while let Some((index, token, value)) = replacements
        .iter()
        .filter_map(|(token, value)| remaining.find(token).map(|index| (index, *token, *value)))
        .min_by_key(|(index, _, _)| *index)
    {
        rendered.push_str(&remaining[..index]);
        rendered.push_str(value);
        remaining = &remaining[index + token.len()..];
    }

    rendered.push_str(remaining);
    rendered
}

fn inline_script_json_string(value: &str) -> String {
    serde_json::to_string(value)
        .expect("serializing a string as JSON cannot fail")
        .replace("</", r"<\/")
}

#[cfg(test)]
mod inline_script_tests {
    use super::{inline_script_json_string, render_template_once};

    #[test]
    fn inline_script_json_never_contains_an_html_end_tag() {
        let encoded = inline_script_json_string("safe</script><script>alert(1)</script>");

        assert_eq!(encoded, r#""safe<\/script><script>alert(1)<\/script>""#);
        assert!(!encoded.contains("</script>"));
    }

    #[test]
    fn template_rendering_does_not_reprocess_inserted_placeholder_text() {
        let rendered = render_template_once(
            "__MESSAGE__ __RESUME_SCRIPT__",
            &[
                ("__MESSAGE__", "__RESUME_SCRIPT__"),
                ("__RESUME_SCRIPT__", "<script>safe()</script>"),
            ],
        );

        assert_eq!(
            rendered, "__RESUME_SCRIPT__ <script>safe()</script>",
            "inserted values must never be treated as template source"
        );
    }
}

fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn loading_response(service_name: &str) -> Response {
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Building {service_name}...</title>
    <script src="https://cdn.jsdelivr.net/npm/ansi_up@5.0.1/ansi_up.min.js"></script>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background-color: #09090b; /* Zinc 950 */
            color: #e4e4e7; /* Zinc 200 */
            display: flex;
            flex-direction: column;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            overflow: hidden;
        }}
        .container {{
            display: flex;
            flex-direction: column;
            align-items: center;
            width: 100%;
            max-width: 800px;
            padding: 2rem;
        }}
        .spinner {{
            border: 3px solid #27272a; /* Zinc 800 */
            border-top: 3px solid #3b82f6; /* Blue 500 */
            border-radius: 50%;
            width: 32px;
            height: 32px;
            animation: spin 1s linear infinite;
            margin-bottom: 1.5rem;
        }}
        @keyframes spin {{
            0% {{ transform: rotate(0deg); }}
            100% {{ transform: rotate(360deg); }}
        }}
        h1 {{
            font-size: 1.25rem;
            font-weight: 600;
            margin: 0 0 0.5rem 0;
            color: #f4f4f5; /* Zinc 100 */
        }}
        p {{
            color: #a1a1aa; /* Zinc 400 */
            font-size: 0.875rem;
            margin: 0 0 2rem 0;
        }}
        .terminal {{
            background-color: #18181b; /* Zinc 900 */
            border: 1px solid #27272a; /* Zinc 800 */
            border-radius: 8px;
            width: 100%;
            height: 400px;
            padding: 1rem;
            font-family: "Menlo", "Monaco", "Courier New", monospace;
            font-size: 0.8rem;
            line-height: 1.4;
            overflow-y: auto;
            white-space: pre-wrap;
            box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
        }}
        .terminal::-webkit-scrollbar {{
            width: 8px;
        }}
        .terminal::-webkit-scrollbar-track {{
            background: #18181b;
        }}
        .terminal::-webkit-scrollbar-thumb {{
            background: #3f3f46;
            border-radius: 4px;
        }}
        .terminal::-webkit-scrollbar-thumb:hover {{
            background: #52525b;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="spinner"></div>
        <h1>Starting {service_name}...</h1>
        <p>Waiting for first response</p>
        <div id="terminal" class="terminal"></div>
    </div>
    <script>
        const serviceName = "{service_name}";
        const terminal = document.getElementById('terminal');
        const ansi_up = new AnsiUp();

        // 1. Poll for readiness
        function poll() {{
            fetch(window.location.href, {{
                headers: {{ 'X-Locald-Passthrough': 'true' }},
                redirect: 'manual'
            }}).then(() => {{
                window.location.reload();
            }}).catch(() => {{
                setTimeout(poll, 1000);
            }});
        }}
        poll();

        // 2. Stream logs
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        // We assume the API is available on the same host/port as the proxy for now,
        // or we need to know where locald is listening.
        // Since this page is served BY locald proxy, window.location.host points to the proxy.
        // The proxy forwards /api requests to the API router.
        // So wss://<host>/api/logs should work.
        const wsUrl = `${{protocol}}//${{window.location.host}}/api/logs`;
        
        const ws = new WebSocket(wsUrl);
        
        ws.onmessage = (event) => {{
            try {{
                const entry = JSON.parse(event.data);
                if (entry.service === serviceName) {{
                    const html = ansi_up.ansi_to_html(entry.message);
                    const line = document.createElement('div');
                    line.innerHTML = html;
                    terminal.appendChild(line);
                    terminal.scrollTop = terminal.scrollHeight;
                }}
            }} catch (e) {{
                console.error('Failed to parse log entry', e);
            }}
        }};

        ws.onopen = () => {{
            console.log('Connected to log stream');
        }};
        
        ws.onerror = (e) => {{
            console.error('WebSocket error', e);
        }};
    </script>
</body>
</html>"#
    );

    (
        StatusCode::OK,
        [
            (hyper::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (hyper::header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

#[cfg(test)]
mod responsive_backend_tests {
    use super::*;

    #[test]
    fn replacement_controller_does_not_inherit_responsiveness() {
        let responsive = ResponsiveBackends::default();
        let original = ResponsiveBackend {
            port: 4_242,
            runtime_generation: 7,
        };
        let replacement = ResponsiveBackend {
            runtime_generation: 8,
            ..original
        };

        responsive.mark(original);

        assert!(responsive.observe(original).is_some());
        assert!(responsive.observe(replacement).is_none());
    }

    #[test]
    fn older_failure_does_not_clear_a_newer_observation() {
        let responsive = ResponsiveBackends::default();
        let backend = ResponsiveBackend {
            port: 4_242,
            runtime_generation: 7,
        };
        let first_deadline = Instant::now() + Duration::from_secs(10);
        let newer_deadline = Instant::now() + Duration::from_secs(20);

        responsive
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(backend, first_deadline);
        let observed = responsive.observe(backend);
        responsive
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(backend, newer_deadline);

        responsive.forget_if_unchanged(backend, observed);

        assert_eq!(responsive.observe(backend), Some(newer_deadline));
    }
}
