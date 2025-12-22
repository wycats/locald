use axum::{
    Router,
    extract::{Path, State, WebSocketUpgrade, ws::WebSocket},
    response::{
        IntoResponse,
        sse::{Event as SseEvent, Sse},
    },
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt, stream::Stream};
use hyper::StatusCode;
use locald_core::service::ServiceController;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;

use crate::manager::ProcessManager;
use locald_core::ipc::Event;

pub fn router(pm: ProcessManager) -> Router {
    Router::new()
        .route("/state", get(handle_state))
        .route("/logs", get(handle_ws))
        .route("/events", get(handle_events))
        .route("/services/stop-all", post(handle_stop_all))
        .route("/services/restart-all", post(handle_restart_all))
        .route("/services/:name/start", post(handle_service_start))
        .route("/services/:name/stop", post(handle_service_stop))
        .route("/services/:name/restart", post(handle_service_restart))
        .route("/services/:name/reset", post(handle_service_reset))
        .route("/services/:name/pty", get(handle_pty_ws))
        .route("/services/:name", get(handle_service_inspect))
        .with_state(Arc::new(pm))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum PtyRequest {
    Input { data: String },
    Resize { rows: u16, cols: u16 },
}

async fn handle_pty_ws(
    ws: WebSocketUpgrade,
    Path(name): Path<String>,
    State(pm): State<Arc<ProcessManager>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_pty_socket(socket, name, pm))
}

async fn handle_pty_socket(socket: WebSocket, name: String, pm: Arc<ProcessManager>) {
    let controller = match pm.get_service_controller(&name).await {
        Some(c) => c,
        None => return,
    };

    let (mut sender, mut receiver) = socket.split();

    // Subscribe to PTY output
    let mut pty_rx = {
        let c = controller.lock().await;
        match c.subscribe_pty() {
            Some(rx) => rx,
            None => return, // Service doesn't support PTY
        }
    };

    // Task to forward PTY output to WebSocket
    let mut send_task = tokio::spawn(async move {
        while let Ok(data) = pty_rx.recv().await {
            if sender
                .send(axum::extract::ws::Message::Binary(data))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Task to forward WebSocket input to PTY
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                axum::extract::ws::Message::Text(text) => {
                    if let Ok(req) = serde_json::from_str::<PtyRequest>(&text) {
                        let controller = controller.lock().await;
                        match req {
                            PtyRequest::Input { data } => {
                                let _ = controller.write_stdin(data.as_bytes()).await;
                            }
                            PtyRequest::Resize { rows, cols } => {
                                let _ = controller.resize_pty(rows, cols).await;
                            }
                        }
                    }
                }
                axum::extract::ws::Message::Binary(data) => {
                    let controller = controller.lock().await;
                    let _ = controller.write_stdin(&data).await;
                }
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}

async fn handle_state(State(pm): State<Arc<ProcessManager>>) -> impl IntoResponse {
    let services = pm.list().await;
    axum::Json(services)
}

async fn handle_ws(
    ws: WebSocketUpgrade,
    State(pm): State<Arc<ProcessManager>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, pm))
}

async fn handle_events(
    State(pm): State<Arc<ProcessManager>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = pm.event_sender.subscribe();
    let recent_logs = pm.get_recent_logs();

    let stream = async_stream::stream! {
        for entry in recent_logs {
             if let Ok(data) = serde_json::to_string(&Event::Log(entry)) {
                yield Ok(SseEvent::default().data(data));
            }
        }

        while let Ok(event) = rx.recv().await {
            if let Ok(data) = serde_json::to_string(&event) {
                yield Ok(SseEvent::default().data(data));
            }
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

async fn handle_socket(mut socket: WebSocket, pm: Arc<ProcessManager>) {
    let mut rx = pm.log_sender.subscribe();
    let recent = pm.get_recent_logs();
    for entry in recent {
        if let Ok(msg) = serde_json::to_string(&entry)
            && socket
                .send(axum::extract::ws::Message::Text(msg))
                .await
                .is_err()
        {
            return;
        }
    }

    while let Ok(entry) = rx.recv().await {
        if let Ok(msg) = serde_json::to_string(&entry)
            && socket
                .send(axum::extract::ws::Message::Text(msg))
                .await
                .is_err()
        {
            break;
        }
    }
}

async fn handle_stop_all(State(pm): State<Arc<ProcessManager>>) -> impl IntoResponse {
    match pm.stop_all().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn handle_restart_all(State(pm): State<Arc<ProcessManager>>) -> impl IntoResponse {
    match pm.restart_all().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn handle_service_start(
    Path(name): Path<String>,
    State(pm): State<Arc<ProcessManager>>,
) -> impl IntoResponse {
    if let Some(path) = pm.get_service_path(&name).await {
        match pm.start(path, None, false).await {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    } else {
        (StatusCode::NOT_FOUND, "Service not found").into_response()
    }
}

async fn handle_service_stop(
    Path(name): Path<String>,
    State(pm): State<Arc<ProcessManager>>,
) -> impl IntoResponse {
    match pm.stop(&name).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn handle_service_restart(
    Path(name): Path<String>,
    State(pm): State<Arc<ProcessManager>>,
) -> impl IntoResponse {
    if let Err(e) = pm.stop(&name).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    if let Some(path) = pm.get_service_path(&name).await {
        match pm.start(path, None, false).await {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    } else {
        (StatusCode::NOT_FOUND, "Service not found").into_response()
    }
}

async fn handle_service_reset(
    Path(name): Path<String>,
    State(pm): State<Arc<ProcessManager>>,
) -> impl IntoResponse {
    match pm.reset(&name).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn handle_service_inspect(
    Path(name): Path<String>,
    State(pm): State<Arc<ProcessManager>>,
) -> impl IntoResponse {
    match pm.inspect(&name).await {
        Ok(info) => axum::Json(info).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}
