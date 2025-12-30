use anyhow::Result;
use locald_e2e::TestContext;

#[tokio::test]
async fn test_service_url_generation() -> Result<()> {
    let mut ctx = TestContext::new().await?;
    ctx.start_daemon().await?;

    // 1. Create a project with mixed service types
    let config = r#"
[project]
name = "url-test"

[services.web]
type = "exec"
command = "python3 -m http.server $PORT"

[services.worker]
type = "worker"
command = "while true; do sleep 1; done"
"#;
    let project_path = ctx.create_project("url-test", config).await?;

    // 2. Run `locald up`
    let output = ctx.run_cli(&["up", project_path.to_str().unwrap()]).await?;
    assert!(output.status.success());

    // 3. Check status
    let output = ctx.run_cli(&["status"]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Status output:\n{}", stdout);

    // Parse output to verify URLs
    // Expected format:
    // NAME                 STATUS     PORT       URL
    // url-test:web         Running    12345      http(s)://url-test.localhost[:PORT]
    // url-test:worker      Running    -          -

    let lines: Vec<&str> = stdout.lines().collect();

    let web_line = lines
        .iter()
        .find(|l| l.contains("url-test:web"))
        .expect("Web service not found");
    let worker_line = lines
        .iter()
        .find(|l| l.contains("url-test:worker"))
        .expect("Worker service not found");

    // Web service should have a URL. In CI, HTTPS may be disabled if `locald trust`
    // hasn't been run, so accept either scheme.
    assert!(
        web_line.contains("https://") || web_line.contains("http://"),
        "Web service should have a URL"
    );

    // Worker service should NOT have a URL (should be "-")
    assert!(
        !worker_line.contains("https://") && !worker_line.contains("http://"),
        "Worker service should not have a URL"
    );
    assert!(
        worker_line.contains("-"),
        "Worker service should show '-' for URL"
    );

    Ok(())
}
