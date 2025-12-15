use axum::{
    Router,
    body::Body,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use std::fmt::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::broadcast;
use tower::ServiceExt;
use tower_http::services::ServeDir;
use tracing::info;

use crate::toolbar::TOOLBAR_JS;
use locald_core::ipc::LogEntry;

#[derive(Clone)]
struct AppState {
    log_tx: broadcast::Sender<LogEntry>,
    root: PathBuf,
}

pub async fn run_static_server(
    port: u16,
    root: PathBuf,
    log_tx: broadcast::Sender<LogEntry>,
) -> anyhow::Result<()> {
    let state = AppState {
        log_tx,
        root: root.clone(),
    };

    let app = Router::new()
        .route("/_locald/ws", get(handle_ws))
        .route("/_locald/toolbar.js", get(handle_toolbar_js))
        .fallback(handle_static_request)
        .layer(middleware::from_fn(inject_toolbar_middleware))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!("Static server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn handle_static_request(State(state): State<AppState>, req: Request<Body>) -> Response {
    let path = req.uri().path();
    let decoded_path = percent_encoding::percent_decode_str(path).decode_utf8_lossy();

    if decoded_path.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let relative_path = decoded_path.trim_start_matches('/');
    let full_path = state.root.join(relative_path);

    if full_path.is_dir() && !full_path.join("index.html").exists() {
        return generate_directory_listing(&full_path, &decoded_path).into_response();
    }

    match ServeDir::new(&state.root).oneshot(req).await {
        Ok(res) => res.into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Unhandled internal error: {}", err),
        )
            .into_response(),
    }
}

fn generate_directory_listing(path: &PathBuf, request_path: &str) -> Response {
    let mut entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let mut html = format!(
        "<html><head><title>Index of {}</title></head><body><h1>Index of {}</h1><ul>",
        request_path, request_path
    );

    if request_path != "/" {
        html.push_str("<li><a href=\"..\">..</a></li>");
    }

    let mut items = Vec::new();
    while let Some(Ok(entry)) = entries.next() {
        if let Ok(name) = entry.file_name().into_string() {
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            items.push((name, is_dir));
        }
    }

    items.sort();

    for (name, is_dir) in items {
        let display_name = if is_dir {
            format!("{}/", name)
        } else {
            name.clone()
        };
        let _ = write!(
            html,
            "<li><a href=\"{}\">{}</a></li>",
            display_name, display_name
        );
    }

    html.push_str("</ul></body></html>");

    ([(axum::http::header::CONTENT_TYPE, "text/html")], html).into_response()
}

async fn handle_toolbar_js() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        TOOLBAR_JS,
    )
}

async fn handle_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.log_tx.subscribe();

    while let Ok(log_entry) = rx.recv().await {
        if let Ok(json) = serde_json::to_string(&log_entry) {
            if socket.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    }
}

async fn inject_toolbar_middleware(req: Request<Body>, next: Next) -> Response {
    let response = next.run(req).await;

    let (parts, body) = response.into_parts();

    // Only inject into HTML responses
    let is_html = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);

    if is_html {
        // Read the body
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(b) => b,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };

        let mut html = String::from_utf8_lossy(&bytes).to_string();

        // Inject the script tag before </body>
        let script_tag = r#"<script src="/_locald/toolbar.js"></script></body>"#;
        if html.contains("</body>") {
            html = html.replace("</body>", script_tag);
        } else {
            html.push_str(script_tag);
        }

        return Response::from_parts(parts, html.into());
    }

    Response::from_parts(parts, body)
}
