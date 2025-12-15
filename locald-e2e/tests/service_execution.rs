use anyhow::Result;
use locald_e2e::TestContext;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_service_execution_lifecycle() -> Result<()> {
    let mut ctx = TestContext::new().await?;
    ctx.start_daemon().await?;

    // 1. Create a project
    let config = r#"
[project]
name = "test-proj"

[services.myservice]
type = "worker"
command = "sleep 300"
"#;
    let project_path = ctx.create_project("test-proj", config).await?;

    // 2. Run `locald up`
    let output = ctx.run_cli(&["up", project_path.to_str().unwrap()]).await?;
    assert!(output.status.success());

    // 3. Check status
    let output = ctx.run_cli(&["status"]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("myservice"));
    assert!(stdout.contains("Running"));

    // 4. Check logs
    assert!(stdout.contains("Running"));

    // 4. Check logs
    // Give it a moment to flush logs
    sleep(Duration::from_millis(500)).await;

    // TODO: Verify logs once we have a reliable way to capture them in tests
    // let output = ctx.run_cli(&["logs", "myservice"]).await?;
    // let stdout = String::from_utf8_lossy(&output.stdout);
    // assert!(stdout.contains("SERVICE STARTED"));

    // 5. Stop service
    let output = ctx.run_cli(&["stop", "test-proj:myservice"]).await?;
    assert!(output.status.success());

    // 6. Check status again
    let output = ctx.run_cli(&["status"]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Stopped"));

    Ok(())
}
