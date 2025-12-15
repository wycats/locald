//! Optional (ignored) integration test for the Postgres service type.

use sqlx::postgres::PgPoolOptions;
use std::process::{Child, Command};
use std::time::Duration;
use tokio::time::sleep;

struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore]
async fn test_postgres_service() -> anyhow::Result<()> {
    // 1. Setup temp project
    let temp_dir = tempfile::tempdir()?;
    let project_path = temp_dir.path();

    // Use a fixed port to simplify connection check
    let port = 54321;
    let toml = format!(
        r#"
[project]
name = "pg-test"

[services.db]
type = "postgres"
version = "15.3.0"
port = {}
"#,
        port
    );
    std::fs::write(project_path.join("locald.toml"), toml)?;

    // 2. Start locald
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    let socket_path = temp_dir.path().join("locald.sock");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.current_dir(project_path)
        .env("XDG_DATA_HOME", &data_dir)
        .env("LOCALD_SOCKET", &socket_path)
        .env("LOCALD_SANDBOX_ACTIVE", "1")
        .env("RUST_LOG", "info")
        .arg("server")
        .arg("start"); // Foreground by default

    let child = cmd.spawn()?;
    let _server_guard = ProcessGuard(child);

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

    // Register project via `locald start` (which sends IPC Start)
    // Note: `locald up` does check-version logic and might restart daemon.
    // `locald start` is not a command?
    // `locald up` sends Start IPC.
    // But `locald up` might try to restart daemon if it thinks version mismatch?
    // Since we are running same binary, version matches.
    // `locald up` also checks if running.

    // Let's use `locald up`. It should detect running daemon (via socket) and just send Start.
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
    eprintln!("locald up finished successfully");

    // 3. Wait for service to be healthy
    let mut healthy = false;
    for i in 0..60 {
        // Wait up to 60 seconds
        sleep(Duration::from_secs(1)).await;

        let mut status_cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
        let output = status_cmd
            .current_dir(project_path)
            .env("XDG_DATA_HOME", &data_dir)
            .env("LOCALD_SOCKET", &socket_path)
            .env("LOCALD_SANDBOX_ACTIVE", "1")
            .arg("status")
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("Status check {}: {}", i, stdout);
        // Check if "db" is "healthy"
        if stdout.contains("db") && stdout.contains("Healthy") {
            healthy = true;
            break;
        }
    }

    if !healthy {
        anyhow::bail!("Service did not become healthy within timeout");
    }

    // 4. Connect to DB
    // Default credentials for postgresql_embedded?
    // Usually it creates a default user/db.
    // We need to check what `PostgresRunner` will do.
    // Assuming it sets up `postgres` user with no password or `postgres` password.
    // Let's assume `postgres:postgres` for now.
    let url = format!("postgres://postgres:postgres@localhost:{}/postgres", port);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;

    let row: (i32,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await?;

    assert_eq!(row.0, 1);

    Ok(())
}
