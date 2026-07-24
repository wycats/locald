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
    let output = ctx.run_up_with_test_owner(&project_path).await?;
    assert!(output.status.success());

    // 3. Check status
    let project = project_path.to_string_lossy();
    let output = ctx.run_cli(&["status", project.as_ref()]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    println!("Status output:\n{}", stdout);

    // The routable web service has one semantic HTTPS URL. The portless
    // worker remains visible as a service without advertising a route.
    assert!(
        stdout.contains("https://url-test.localhost"),
        "Web service should have a URL"
    );
    assert!(stdout.contains("url-test:worker: running"));
    assert!(
        !stdout.contains("worker.url-test.localhost"),
        "Worker service should not have a URL"
    );

    Ok(())
}
