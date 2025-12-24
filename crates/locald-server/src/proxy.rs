use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    handler::Handler,
    http::Uri,
    response::{IntoResponse, Response},
};
use axum_server::tls_rustls::RustlsConfig;
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

use crate::assets;
use locald_core::resolver::ServiceResolver;
use locald_utils::cert::CertManager;

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
        }
    }

    pub(crate) fn make_app(&self) -> Router {
        let mut connector = hyper_util::client::legacy::connect::HttpConnector::new();
        connector.set_nodelay(true);
        connector.set_keepalive(Some(std::time::Duration::from_secs(60)));

        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector);

        let state = AppState {
            resolver: self.resolver.clone(),
            client,
        };

        Router::new()
            .nest("/api", self.api_router.clone())
            .fallback_service(handle_proxy.with_state(state))
            .layer(TraceLayer::new_for_http())
    }

    pub async fn bind_http(&self, port: u16) -> anyhow::Result<TcpListener> {
        // Port 0 means "pick any free port" and does not require privileges.
        let listener = if port != 0 && port < 1024 {
            match crate::shim_client::bind_privileged_port(port).await {
                Ok(l) => {
                    l.set_nonblocking(true)?;
                    TcpListener::from_std(l)?
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to acquire privileged port {} via shim: {}. Falling back to direct bind.",
                        port,
                        e
                    );
                    let addr = SocketAddr::from(([0, 0, 0, 0], port));
                    TcpListener::bind(addr).await?
                }
            }
        } else {
            let addr = SocketAddr::from(([0, 0, 0, 0], port));
            TcpListener::bind(addr).await?
        };

        let addr = listener.local_addr()?;
        info!("Proxy bound to http://{addr}");
        self.resolver.set_http_port(Some(addr.port())).await;
        Ok(listener)
    }

    pub async fn serve_http(&self, listener: TcpListener) -> anyhow::Result<()> {
        let app = self.make_app();
        axum::serve(listener, app).await?;
        Ok(())
    }

    pub async fn bind_https(&self, port: u16) -> anyhow::Result<std::net::TcpListener> {
        // Port 0 means "pick any free port" and does not require privileges.
        let listener = if port != 0 && port < 1024 {
            match crate::shim_client::bind_privileged_port(port).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(
                        "Failed to acquire privileged port {} via shim: {}. Falling back to direct bind.",
                        port,
                        e
                    );
                    let addr = SocketAddr::from(([0, 0, 0, 0], port));
                    tokio::net::TcpListener::bind(addr).await?.into_std()?
                }
            }
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

        // Update process manager
        if let Ok(addr) = listener.local_addr() {
            info!("Proxy bound to https://{addr}");
            self.resolver.set_https_port(Some(addr.port())).await;
        }

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(cert_manager.clone());

        let rustls_config = RustlsConfig::from_config(Arc::new(config));
        let app = self.make_app();

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

async fn handle_proxy(State(state): State<AppState>, mut req: Request) -> Response {
    let host = match req.headers().get("host") {
        Some(h) => h
            .to_str()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default(),
        None => return (StatusCode::BAD_REQUEST, "Missing Host header").into_response(),
    };

    if host == "docs.localhost" || host == "docs.local" {
        return assets::handle_docs(req.uri()).into_response();
    }

    // Check if there is a running service for this domain first (e.g. locald-dashboard in dev mode)
    if let Some((service_name, port)) = state.resolver.resolve_service_by_domain(host).await {
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
            return handle_websocket_upgrade(state, req, uri).await;
        }

        let is_passthrough = req.headers().get("x-locald-passthrough").is_some();
        let accepts_html = req
            .headers()
            .get(hyper::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("text/html"))
            .unwrap_or(false);

        *req.uri_mut() = uri;

        let backend_future = state.client.request(req);

        if is_passthrough || !accepts_html {
            match backend_future.await {
                Ok(res) => return res.into_response(),
                Err(e) => {
                    error!("Proxy error: {e}");
                    return error_response(StatusCode::BAD_GATEWAY, format!("Proxy error: {e}"));
                }
            }
        }

        let response = tokio::select! {
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
        };

        return response;
    }

    // Fallback to embedded dashboard if no service claims the domain
    if host == "locald.localhost" || host == "locald.local" || host == "localhost" {
        return assets::handle_dashboard(req.uri()).into_response();
    }

    (StatusCode::NOT_FOUND, format!("Domain {host} not found")).into_response()
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
        <h1>Building {service_name}...</h1>
        <p>Waiting for service to become ready</p>
        <div id="terminal" class="terminal"></div>
    </div>
    <script>
        const serviceName = "{service_name}";
        const terminal = document.getElementById('terminal');
        const ansi_up = new AnsiUp();

        // 1. Poll for readiness
        function poll() {{
            fetch(window.location.href, {{
                headers: {{ 'X-Locald-Passthrough': 'true' }}
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
