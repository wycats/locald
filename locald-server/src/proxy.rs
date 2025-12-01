use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{State, WebSocketUpgrade, ws::WebSocket, Request},
    response::{IntoResponse, Response},
    routing::get,
    Router,
    body::Body,
    http::Uri,
};
use hyper::StatusCode;
use tokio::net::TcpListener;
use tracing::{error, info};
use rust_embed::RustEmbed;

use crate::manager::ProcessManager;

#[derive(RustEmbed)]
#[folder = "src/assets/"]
struct Assets;

pub struct ProxyManager {
    process_manager: ProcessManager,
}

impl ProxyManager {
    pub fn new(process_manager: ProcessManager) -> Self {
        Self { process_manager }
    }

    pub async fn start(&self, port: u16) -> anyhow::Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = TcpListener::bind(addr).await?;
        info!("Proxy listening on http://{}", addr);

        let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(hyper_util::client::legacy::connect::HttpConnector::new());
        
        let state = AppState {
            pm: self.process_manager.clone(),
            client,
        };

        let app = Router::new()
            .route("/api/state", get(handle_state))
            .route("/api/logs", get(handle_ws))
            .fallback(handle_proxy)
            .with_state(state);

        axum::serve(listener, app).await?;
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    pm: ProcessManager,
    client: hyper_util::client::legacy::Client<hyper_util::client::legacy::connect::HttpConnector, Body>,
}

async fn handle_state(State(state): State<AppState>) -> impl IntoResponse {
    let services = state.pm.list().await;
    axum::Json(services)
}

async fn handle_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state.pm))
}

async fn handle_socket(mut socket: WebSocket, pm: ProcessManager) {
    let mut rx = pm.log_sender.subscribe();
    let recent = pm.get_recent_logs().await;
    for entry in recent {
        if let Ok(msg) = serde_json::to_string(&entry) {
             if socket.send(axum::extract::ws::Message::Text(msg)).await.is_err() {
                 return;
             }
        }
    }

    while let Ok(entry) = rx.recv().await {
        if let Ok(msg) = serde_json::to_string(&entry) {
            if socket.send(axum::extract::ws::Message::Text(msg)).await.is_err() {
                break;
            }
        }
    }
}

async fn handle_assets(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(axum::http::header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
    }
}

async fn handle_proxy(
    State(state): State<AppState>,
    mut req: Request,
) -> Response {
    let host = match req.headers().get("host") {
        Some(h) => h.to_str().unwrap_or_default().split(':').next().unwrap_or_default(),
        None => return (StatusCode::BAD_REQUEST, "Missing Host header").into_response(),
    };

    if host == "locald.local" {
        return handle_assets(req.uri().clone()).await.into_response();
    }

    if let Some(port) = state.pm.find_port_by_domain(host).await {
        let uri_string = format!("http://127.0.0.1:{}{}", port, req.uri().path_and_query().map(|x| x.as_str()).unwrap_or(""));
        if let Ok(uri) = uri_string.parse() {
            *req.uri_mut() = uri;
        } else {
             return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid URI").into_response();
        }
        
        // Forward the request
        match state.client.request(req).await {
            Ok(res) => res.into_response(),
            Err(e) => {
                error!("Proxy error: {}", e);
                (StatusCode::BAD_GATEWAY, format!("Proxy error: {}", e)).into_response()
            }
        }
    } else {
        (StatusCode::NOT_FOUND, format!("Domain {} not found", host)).into_response()
    }
}
