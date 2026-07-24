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
    let output = ctx.run_up_with_test_owner(&project_path).await?;
    assert!(output.status.success());

    // 3. Check status
    let project = project_path.to_string_lossy();
    let output = ctx.run_cli(&["status", project.as_ref()]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Availability: Ready (desired up)"));
    assert!(stdout.contains("test-proj:myservice: running"));

    // 4. Check logs
    // Give it a moment to flush logs
    sleep(Duration::from_millis(500)).await;

    // TODO: Verify logs once we have a reliable way to capture them in tests
    // let output = ctx.run_cli(&["logs", "myservice"]).await?;
    // let stdout = String::from_utf8_lossy(&output.stdout);
    // assert!(stdout.contains("SERVICE STARTED"));

    // 5. Stop service
    let output = ctx
        .run_cli(&["service", "stop", "test-proj:myservice"])
        .await?;
    assert!(output.status.success());

    // 6. Check status again
    let output = ctx.run_cli(&["status", project.as_ref()]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test-proj:myservice: stopped"));

    Ok(())
}
