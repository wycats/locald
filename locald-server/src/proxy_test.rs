use std::sync::Arc;
use tokio::sync::Mutex;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use locald_core::registry::Registry;
use tower::ServiceExt; // for `oneshot`

use crate::{manager::ProcessManager, proxy::ProxyManager, state::StateManager};

#[tokio::test]
async fn test_dashboard_routing() {
    // Setup
    let temp_dir = std::env::temp_dir().join("locald-test-dashboard");
    let _ = std::fs::create_dir_all(&temp_dir);
    let notify_path = temp_dir.join("notify.sock");

    let state_manager = Arc::new(StateManager::with_path(temp_dir.join("state.json")));
    let registry = Arc::new(Mutex::new(Registry::default()));

    let pm = ProcessManager::new(notify_path, None, state_manager, registry, None)
        .expect("Failed to create ProcessManager");
    let pm = Arc::new(pm);
    let proxy = ProxyManager::new(pm, Router::new(), None);
    let app = proxy.make_app();

    // Test locald.localhost
    let req = Request::builder()
        .uri("/")
        .header("Host", "locald.localhost")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify content type (should be html)
    let content_type = response.headers().get("content-type").unwrap();
    assert_eq!(content_type, "text/html");

    // Test locald.local alias
    let req = Request::builder()
        .uri("/")
        .header("Host", "locald.local")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_docs_routing() {
    // Setup
    let temp_dir = std::env::temp_dir().join("locald-test-docs");
    let _ = std::fs::create_dir_all(&temp_dir);
    let notify_path = temp_dir.join("notify.sock");

    let state_manager = Arc::new(StateManager::with_path(temp_dir.join("state.json")));
    let registry = Arc::new(Mutex::new(Registry::default()));

    let pm = ProcessManager::new(notify_path, None, state_manager, registry, None)
        .expect("Failed to create ProcessManager");
    let pm = Arc::new(pm);
    let proxy = ProxyManager::new(pm, Router::new(), None);
    let app = proxy.make_app();

    // Test docs.localhost
    let req = Request::builder()
        .uri("/")
        .header("Host", "docs.localhost")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[derive(Debug)]
struct MockResolver {
    port: Option<u16>,
}

#[async_trait::async_trait]
impl locald_core::resolver::ServiceResolver for MockResolver {
    async fn resolve_service_by_domain(&self, _domain: &str) -> Option<(String, u16)> {
        self.port.map(|p| ("mock-service".to_string(), p))
    }
    async fn set_http_port(&self, _port: Option<u16>) {}
    async fn set_https_port(&self, _port: Option<u16>) {}
}

#[tokio::test]
async fn test_proxy_error_page() {
    // Setup with a mock resolver that returns a port where nothing is listening
    let resolver = Arc::new(MockResolver { port: Some(12345) });
    let proxy = ProxyManager::new(resolver, Router::new(), None);
    let app = proxy.make_app();

    // Request to a domain that resolves to the closed port
    let req = Request::builder()
        .uri("/")
        .header("Host", "broken.localhost")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();

    // Should be 502 Bad Gateway
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    // Should be HTML
    let content_type = response.headers().get("content-type").unwrap();
    assert_eq!(content_type, "text/html; charset=utf-8");

    // Body should contain the error message
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();

    assert!(body_str.contains("Proxy Error"));
    assert!(body_str.contains("locald could not connect"));
}

#[tokio::test]
async fn test_proxy_connection_success() {
    // 1. Start a dummy backend server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        // Simple HTTP response
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\nHello World!";
        use tokio::io::AsyncWriteExt;
        socket.write_all(response.as_bytes()).await.unwrap();
    });

    // 2. Setup Proxy with MockResolver pointing to that port
    let resolver = Arc::new(MockResolver { port: Some(port) });
    let proxy = ProxyManager::new(resolver, Router::new(), None);
    let app = proxy.make_app();

    // 3. Send request
    let req = Request::builder()
        .uri("/")
        .header("Host", "test.localhost")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();

    // 4. Verify success
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body_bytes[..], b"Hello World!");
}
