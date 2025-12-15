use std::process::{Child, Command};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_dependency_injection() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let project_path = temp_dir.path();

    let toml = r#"
[project]
name = "dep-test"

[services.api]
command = "python3 -m http.server $PORT"

[services.web]
command = "echo API_URL=$API_URL; python3 -m http.server $PORT"
depends_on = ["api"]
[services.web.env]
API_URL = "${services.api.url}"
"#;
    std::fs::write(project_path.join("locald.toml"), toml)?;

    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    let socket_path = temp_dir.path().join("locald.sock");

    // Start server
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.current_dir(project_path)
        .env("XDG_DATA_HOME", &data_dir)
        .env("LOCALD_SOCKET", &socket_path)
        .env("LOCALD_SANDBOX_ACTIVE", "1")
        .env("RUST_LOG", "info")
        .arg("server")
        .arg("start");

    let mut server_child: Child = cmd.spawn()?;

    // Wait for socket
    let mut socket_ready = false;
    for _ in 0..50 {
        if socket_path.exists() {
            socket_ready = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }

    if !socket_ready {
        anyhow::bail!("locald socket did not appear");
    }

    // Start project
    let status = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .current_dir(project_path)
        .env("XDG_DATA_HOME", &data_dir)
        .env("LOCALD_SOCKET", &socket_path)
        .env("LOCALD_SANDBOX_ACTIVE", "1")
        .arg("up")
        .status()?;

    if !status.success() {
        anyhow::bail!("locald up failed");
    }

    use std::io::{BufRead, BufReader};

    // Check logs for web service
    // We need to wait a bit for logs to be captured
    sleep(Duration::from_secs(2)).await;

    let output = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .current_dir(project_path)
        .env("XDG_DATA_HOME", &data_dir)
        .env("LOCALD_SOCKET", &socket_path)
        .env("LOCALD_SANDBOX_ACTIVE", "1")
        .arg("logs")
        .arg("web")
        .output()?;

    let reader = BufReader::new(output.stdout.as_slice());
    let found = reader
        .lines()
        .filter_map(Result::ok)
        .any(|line| line.contains("API_URL=http://localhost:"));

    assert!(found);

    // Shut down the daemon cleanly so LLVM coverage profiles flush.
    let _ = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .current_dir(project_path)
        .env("XDG_DATA_HOME", &data_dir)
        .env("LOCALD_SOCKET", &socket_path)
        .env("LOCALD_SANDBOX_ACTIVE", "1")
        .arg("server")
        .arg("shutdown")
        .status();

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if server_child.try_wait()?.is_some() {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }

    if server_child.try_wait()?.is_none() {
        anyhow::bail!("locald server did not shut down cleanly");
    }

    Ok(())
}
