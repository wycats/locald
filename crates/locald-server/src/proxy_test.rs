use std::sync::Arc;
use tokio::sync::Mutex;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    routing::{get, post},
};
use locald_core::registry::Registry;
use locald_core::resolver::DomainResolution;
use locald_core::state::ServiceState;
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

    let attachments = std::sync::Arc::new(tokio::sync::Mutex::new(
        locald_core::attachments::AttachmentStore::new(
            locald_core::attachments::AttachmentStore::path(),
        ),
    ));
    let pm = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
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

    let attachments = std::sync::Arc::new(tokio::sync::Mutex::new(
        locald_core::attachments::AttachmentStore::new(
            locald_core::attachments::AttachmentStore::path(),
        ),
    ));
    let pm = ProcessManager::new(notify_path, state_manager, registry, attachments, None)
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

#[tokio::test]
async fn test_project_service_overrides_platform_fallback() {
    let resolver = Arc::new(MockResolver {
        port: None,
        status: ServiceState::Stopped,
    });
    let proxy = ProxyManager::new(resolver, Router::new(), None);
    let app = proxy.make_app();

    for host in ["locald.localhost", "docs.localhost", "dev.locald.localhost"] {
        let req = Request::builder()
            .uri("/")
            .header("Host", host)
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{host}");
    }
}

#[derive(Debug)]
struct MockResolver {
    port: Option<u16>,
    status: ServiceState,
}

#[async_trait::async_trait]
impl locald_core::resolver::ServiceResolver for MockResolver {
    async fn resolve_service_by_domain(&self, _domain: &str) -> Option<DomainResolution> {
        Some(DomainResolution::Service {
            name: "mock-service".to_string(),
            port: self.port,
            status: self.status,
            runtime_generation: 1,
        })
    }
    async fn set_http_port(&self, _port: Option<u16>) {}
    async fn set_https_port(&self, _port: Option<u16>) {}
}

#[derive(Debug)]
struct UnknownDomainResolver;

#[async_trait::async_trait]
impl locald_core::resolver::ServiceResolver for UnknownDomainResolver {
    async fn resolve_service_by_domain(&self, _domain: &str) -> Option<DomainResolution> {
        None
    }

    async fn set_http_port(&self, _port: Option<u16>) {}
    async fn set_https_port(&self, _port: Option<u16>) {}
}

async fn response_body(response: axum::response::Response) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn test_active_service_owns_its_api_paths() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nservice-api";
        tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes())
            .await
            .unwrap();
    });

    let resolver = Arc::new(MockResolver {
        port: Some(port),
        status: ServiceState::Running,
    });
    let api = Router::new().route("/session-token", get(|| async { "locald-api" }));
    let app = ProxyManager::new(resolver, api, None).make_app();
    let req = Request::builder()
        .uri("/api/session-token")
        .header("Host", "workbench.agent-lab.localhost")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_body(response).await, "service-api");
}

#[tokio::test]
async fn test_dashboard_hosts_serve_locald_api() {
    let api = Router::new().route("/state", get(|| async { "locald-api" }));
    let app = ProxyManager::new(Arc::new(UnknownDomainResolver), api, None).make_app();

    for host in [
        "locald.localhost",
        "locald.local",
        "localhost",
        "dev.locald.localhost",
    ] {
        let req = Request::builder()
            .uri("/api/state")
            .header("Host", host)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{host}");
        assert_eq!(response_body(response).await, "locald-api", "{host}");
    }
}

#[tokio::test]
async fn test_non_dashboard_hosts_cannot_reach_locald_api() {
    let api = Router::new().route("/state", get(|| async { "locald-api" }));
    let app = ProxyManager::new(Arc::new(UnknownDomainResolver), api, None).make_app();

    for host in ["docs.localhost", "docs.local", "unknown.localhost"] {
        let req = Request::builder()
            .uri("/api/state")
            .header("Host", host)
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{host}");
        assert!(
            !response_body(response).await.contains("locald-api"),
            "{host}"
        );
    }
}

#[tokio::test]
async fn test_stopped_project_domain_exposes_only_resume_api() {
    let resolver = Arc::new(MockResolver {
        port: None,
        status: ServiceState::Stopped,
    });
    let api = Router::new()
        .route("/projects/resume-domain", post(|| async { "resumed" }))
        .route("/state", get(|| async { "locald-api" }));
    let app = ProxyManager::new(resolver, api, None).make_app();

    let resume = Request::builder()
        .method("POST")
        .uri("/api/projects/resume-domain")
        .header("Host", "paused.localhost")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(resume).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_body(response).await, "resumed");

    let state = Request::builder()
        .uri("/api/state")
        .header("Host", "paused.localhost")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(state).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response_body(response).await.contains("locald-api"));
}

#[tokio::test]
async fn test_proxy_error_page() {
    // Setup with a mock resolver that returns a port where nothing is listening
    let resolver = Arc::new(MockResolver {
        port: Some(12345),
        status: ServiceState::Running,
    });
    let proxy = ProxyManager::new(resolver, Router::new(), None);
    let app = proxy.make_app();

    // Request to a domain that resolves to the closed port
    let req = Request::builder()
        .uri("/")
        .header("Host", "broken.localhost")
        .header("Accept", "text/html")
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
async fn test_disabled_service_page() {
    let resolver = Arc::new(MockResolver {
        port: None,
        status: ServiceState::Stopped,
    });
    let proxy = ProxyManager::new(resolver, Router::new(), None);
    let app = proxy.make_app();

    let req = Request::builder()
        .uri("/")
        .header("Host", "disabled.localhost")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let content_type = response.headers().get("content-type").unwrap();
    assert_eq!(content_type, "text/html; charset=utf-8");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("Resume project"));
    assert!(body.contains("/api/projects/resume-domain"));
    assert!(body.contains("disabled.localhost"));
    assert!(body.contains("https://locald.localhost"));
    assert!(!body.contains("/api/services/"));
}

#[derive(Debug)]
struct OwnershipOnlyResolver {
    state: locald_core::ProjectLifecycleState,
}

#[async_trait::async_trait]
impl locald_core::resolver::ServiceResolver for OwnershipOnlyResolver {
    async fn resolve_service_by_domain(&self, _domain: &str) -> Option<DomainResolution> {
        Some(DomainResolution::OwnershipOnly)
    }

    async fn project_availability_by_domain(
        &self,
        _domain: &str,
    ) -> Option<locald_core::ProjectAvailabilityStatus> {
        Some(locald_core::ProjectAvailabilityStatus {
            desired: matches!(
                self.state,
                locald_core::ProjectLifecycleState::Starting
                    | locald_core::ProjectLifecycleState::Ready
                    | locald_core::ProjectLifecycleState::Degraded
                    | locald_core::ProjectLifecycleState::Failed
            ),
            state: self.state,
            always_on: true,
            paused: self.state == locald_core::ProjectLifecycleState::Paused,
            reasons: match self.state {
                locald_core::ProjectLifecycleState::Paused => {
                    vec![locald_core::AvailabilityReason {
                        code: "paused".to_owned(),
                        message: "Paused until meaningful activity resumes the project.".to_owned(),
                    }]
                }
                locald_core::ProjectLifecycleState::Missing => {
                    vec![locald_core::AvailabilityReason {
                        code: "missing".to_owned(),
                        message: "The project worktree is missing.".to_owned(),
                    }]
                }
                _ => Vec::new(),
            },
            demands: Vec::new(),
            next_transition_at: None,
            last_error: None,
        })
    }

    async fn set_http_port(&self, _port: Option<u16>) {}
    async fn set_https_port(&self, _port: Option<u16>) {}
}

#[tokio::test]
async fn test_legacy_owned_domain_has_a_project_resume_surface() {
    let proxy = ProxyManager::new(
        Arc::new(OwnershipOnlyResolver {
            state: locald_core::ProjectLifecycleState::Paused,
        }),
        Router::new(),
        None,
    );
    let app = proxy.make_app();

    for host in ["legacy.localhost", "docs.localhost"] {
        let req = Request::builder()
            .uri("/")
            .header("Host", host)
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{host}");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Project is Paused"));
        assert!(body.contains("Paused until meaningful activity resumes the project."));
        assert!(body.contains("Always On remains enabled"));
        assert!(body.contains("locald up"));
        assert!(body.contains("Resume project"));
        assert!(body.contains("/api/projects/resume-domain"));
        assert!(!body.contains("/api/services/"));
    }
}

#[tokio::test]
async fn test_missing_owned_domain_explains_how_to_restore_the_worktree() {
    let proxy = ProxyManager::new(
        Arc::new(OwnershipOnlyResolver {
            state: locald_core::ProjectLifecycleState::Missing,
        }),
        Router::new(),
        None,
    );
    let app = proxy.make_app();

    let req = Request::builder()
        .uri("/")
        .header("Host", "missing.localhost")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("Project is Missing"));
    assert!(body.contains("Restore the project worktree"));
    assert!(body.contains("locald up"));
    assert!(!body.contains("Resume project"));
    assert!(!body.contains("resume-btn"));
    assert!(!body.contains("/api/projects/resume-domain"));
}

#[tokio::test]
async fn test_available_owned_domain_directs_user_to_service_diagnostics() {
    for state in [
        locald_core::ProjectLifecycleState::Starting,
        locald_core::ProjectLifecycleState::Ready,
    ] {
        let proxy = ProxyManager::new(
            Arc::new(OwnershipOnlyResolver { state }),
            Router::new(),
            None,
        );
        let app = proxy.make_app();

        let req = Request::builder()
            .uri("/")
            .header("Host", "worker.localhost")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains(&format!("Project is {state}")));
        assert!(body.contains("service status and logs") || body.contains("status and logs"));
        assert!(body.contains("Open dashboard"));
        assert!(!body.contains("Resume project"));
        assert!(!body.contains("resume-btn"));
        assert!(!body.contains("/api/projects/resume-domain"));
    }
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
    let resolver = Arc::new(MockResolver {
        port: Some(port),
        status: ServiceState::Running,
    });
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

#[tokio::test]
async fn test_loading_passthrough_hands_slow_warm_pages_to_the_browser() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen_cookies = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let backend_cookies = seen_cookies.clone();
    let backend = Router::new().fallback(move |headers: HeaderMap| {
        let backend_cookies = backend_cookies.clone();
        async move {
            backend_cookies.lock().await.push(
                headers
                    .get(hyper::header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
            );
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            "slow-app"
        }
    });
    tokio::spawn(async move {
        axum::serve(listener, backend).await.unwrap();
    });

    let resolver = Arc::new(MockResolver {
        port: Some(port),
        status: ServiceState::Running,
    });
    let app = ProxyManager::new(resolver, Router::new(), None).make_app();

    let initial = Request::builder()
        .uri("/")
        .header("Host", "slow.localhost")
        .header("Accept", "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(initial).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response_body(response)
            .await
            .contains("Waiting for first response")
    );

    let poll = Request::builder()
        .uri("/")
        .header("Host", "slow.localhost")
        .header("X-Locald-Passthrough", "true")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(poll).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let reload = Request::builder()
        .uri("/")
        .header("Host", "slow.localhost")
        .header("Accept", "text/html")
        .header("Cookie", "session=abc; theme=dark")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(reload).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_body(response).await, "slow-app");

    {
        let seen_cookies = seen_cookies.lock().await;
        assert_eq!(
            seen_cookies.last().and_then(Option::as_deref),
            Some("session=abc; theme=dark")
        );
    }

    let later_navigation = Request::builder()
        .uri("/another-slow-page")
        .header("Host", "slow.localhost")
        .header("Accept", "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(later_navigation).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_body(response).await, "slow-app");
}

#[tokio::test]
async fn test_loading_handoff_is_shared_across_service_aliases_and_redirects() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let backend = Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::temporary("/final") }),
        )
        .route("/cached", get(|| async { StatusCode::NOT_MODIFIED }))
        .route(
            "/final",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                "redirected-slow-app"
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, backend).await.unwrap();
    });

    let resolver = Arc::new(MockResolver {
        port: Some(port),
        status: ServiceState::Running,
    });
    let app = ProxyManager::new(resolver, Router::new(), None).make_app();

    let redirect = Request::builder()
        .uri("/")
        .header("Host", "alias.localhost")
        .header("X-Locald-Passthrough", "true")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(redirect).await.unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/final")
    );

    let final_navigation = Request::builder()
        .uri("/final")
        .header("Host", "canonical.localhost")
        .header("Accept", "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(final_navigation).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_body(response).await, "redirected-slow-app");

    let cached_navigation = Request::builder()
        .uri("/cached")
        .header("Host", "canonical.localhost")
        .header("Accept", "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(cached_navigation).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}
