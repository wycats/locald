#![cfg(target_os = "linux")]

use anyhow::{Context, Result};
use locald_e2e::TestContext;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::sleep;

fn cgroup_dir_for_service(sandbox: &str, service_id: &str) -> PathBuf {
    let strategy = locald_utils::cgroup::detect_root_strategy();
    let cgroup_path = locald_utils::cgroup::cgroup_path_for_service(strategy, sandbox, service_id);
    locald_utils::cgroup::cgroup_fs_root().join(cgroup_path.trim_start_matches('/'))
}

async fn wait_for_exists(path: &std::path::Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if path.exists() {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("Timed out waiting for {} to exist", path.display());
}

async fn wait_for_gone(path: &std::path::Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !path.exists() {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("Timed out waiting for {} to be removed", path.display());
}

#[tokio::test]
#[ignore]
async fn test_service_stop_prunes_cgroup_leaf() -> Result<()> {
    // This test is intentionally ignored by default.
    // It is designed to be runnable as an unprivileged user, but it requires:
    // - cgroup v2 mounted
    // - a setuid-root locald-shim installed next to the locald binary
    // - the locald cgroup root configured (e.g. sudo ./target/debug/locald admin setup)

    if !std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
        anyhow::bail!(
            "cgroup v2 does not appear to be available (missing /sys/fs/cgroup/cgroup.controllers)"
        );
    }

    let shim = locald_utils::shim::find_privileged()?.context(
        "locald-shim is not configured; run sudo ./target/debug/locald admin setup first",
    )?;

    let strategy = locald_utils::cgroup::detect_root_strategy();
    if !locald_utils::cgroup::is_root_ready(strategy) {
        anyhow::bail!(
            "locald cgroup root is not ready; run sudo ./target/debug/locald admin setup (shim at {})",
            shim.display()
        );
    }

    let mut ctx = TestContext::new().await?;
    ctx.start_daemon().await?;

    let config = r#"
[project]
name = "test-proj"

[services.myservice]
type = "container"
image = "alpine:latest"
command = "sleep 300"
"#;
    let project_path = ctx.create_project("test-proj", config).await?;

    let output = ctx.run_cli(&["up", project_path.to_str().unwrap()]).await?;
    if !output.status.success() {
        ctx.dump_logs().await?;
        anyhow::bail!(
            "locald up failed (status: {}). stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // While running, the cgroup leaf should exist.
    let service_id = "test-proj:myservice";
    let cgroup_dir = cgroup_dir_for_service(&ctx.sandbox, service_id);
    wait_for_exists(&cgroup_dir, Duration::from_secs(5)).await?;

    let output = ctx.run_cli(&["stop", service_id]).await?;
    if !output.status.success() {
        ctx.dump_logs().await?;
        anyhow::bail!(
            "locald stop failed (status: {}). stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // After stop, Phase 99 promises a scorched-earth cgroup kill + prune.
    wait_for_gone(&cgroup_dir, Duration::from_secs(10)).await?;

    Ok(())
}
