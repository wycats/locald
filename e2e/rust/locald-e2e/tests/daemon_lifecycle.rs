use anyhow::Result;
use locald_e2e::TestContext;

#[tokio::test]
async fn test_daemon_startup() -> Result<()> {
    let mut ctx = TestContext::new().await?;
    ctx.start_daemon().await?;

    let output = ctx.run_cli(&["status"]).await?;
    if !output.status.success() {
        eprintln!("Stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("Stderr: {}", String::from_utf8_lossy(&output.stderr));
    }
    assert!(output.status.success());

    Ok(())
}
