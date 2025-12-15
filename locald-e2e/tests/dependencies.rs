use anyhow::Result;
use locald_e2e::TestContext;

#[tokio::test]
async fn test_dependencies() -> Result<()> {
    let mut ctx = TestContext::new().await?;
    ctx.start_daemon().await?;

    // 1. Create a project with dependencies
    // db takes a bit to become healthy (we simulate this by sleeping before creating the file)
    // But wait, the command runs immediately. The health check polls.
    // So:
    // db: command="sleep 2 && touch ready && sleep 300", health_check="test -f ready"
    // web: depends_on=["db"]

    let config = r#"
[project]
name = "dep-test"

[services.db]
command = "sh -c 'sleep 2 && touch ready && sleep 300'"
health_check = "test -f ready"

[services.web]
command = "echo WEB STARTED && sleep 300"
depends_on = ["db"]
"#;
    let project_path = ctx.create_project("dep-test", config).await?;

    // 2. Run `locald up`
    let output = ctx.run_cli(&["up", project_path.to_str().unwrap()]).await?;
    assert!(output.status.success());

    // 3. Verify output order
    // We check daemon logs for startup order
    let daemon_log = tokio::fs::read_to_string(ctx.root.path().join("daemon.out")).await?;
    println!("DAEMON LOG:\n{}", daemon_log);

    let db_start_idx = daemon_log
        .find("Starting service dep-test:db")
        .expect("Should start db");
    let web_start_idx = daemon_log
        .find("Starting service dep-test:web")
        .expect("Should start web");

    assert!(db_start_idx < web_start_idx, "DB should start before Web");

    // We should also see "Waiting for service"
    assert!(
        daemon_log.contains("Waiting for service dep-test:db"),
        "Should wait for db"
    );

    Ok(())
}
