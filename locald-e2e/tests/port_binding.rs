use anyhow::Result;
use locald_e2e::TestContext;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_port_binding() -> Result<()> {
    let mut ctx = TestContext::new().await?;
    ctx.start_daemon().await?;

    // 1. Create a project with a specific port
    let config = r#"
[project]
name = "port-test"

[services.web]
command = "sh -c 'python3 -m http.server $PORT'"
port = 9090
"#;
    let project_path = ctx.create_project("port-test", config).await?;

    // 2. Run `locald up`
    let output = ctx.run_cli(&["up", project_path.to_str().unwrap()]).await?;
    assert!(output.status.success());

    // 3. Check status
    let output = ctx.run_cli(&["status"]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.contains("9090") {
        ctx.dump_logs().await?;
    }

    assert!(stdout.contains("9090"));
    assert!(stdout.contains("Running"));

    // 4. Verify port is open
    // We can try to connect to localhost:9090
    // Since we are in a sandbox, we need to make sure we are connecting to the right place.
    // The daemon spawns processes on the host (even in sandbox mode, it just isolates config/socket).
    // So localhost:9090 should be reachable.

    let client = reqwest::Client::new();
    let mut success = false;
    for _ in 0..10 {
        if client.get("http://127.0.0.1:9090").send().await.is_ok() {
            success = true;
            break;
        }
        sleep(Duration::from_millis(500)).await;
    }

    assert!(success, "Failed to connect to service on port 9090");

    Ok(())
}
