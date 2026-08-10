use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::Path;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;
use tracing::debug;

/// Checks if an HTTP(S) URL is reachable and returns a success status code.
pub async fn check_http(url: &str, timeout: Duration) -> bool {
    let client = reqwest::Client::builder().timeout(timeout).build();

    match client {
        Ok(client) => match client.get(url).send().await {
            Ok(res) => res.status().is_success(),
            Err(e) => {
                debug!("HTTP probe failed for {}: {}", url, e);
                false
            }
        },
        Err(e) => {
            debug!("Failed to build HTTP client: {}", e);
            false
        }
    }
}

/// Checks if a TCP port is open.
pub async fn check_tcp(addr: &str, timeout: Duration) -> bool {
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            debug!("TCP probe failed for {}: {}", addr, e);
            false
        }
        Err(_) => {
            debug!("TCP probe timed out for {}", addr);
            false
        }
    }
}

/// Checks if a command executes successfully (exit code 0).
pub async fn check_command(cmd: &str, cwd: Option<&Path>, timeout: Duration) -> bool {
    check_command_with_env(cmd, cwd, &HashMap::new(), timeout).await
}

/// Checks if a command executes successfully with the service environment.
pub async fn check_command_with_env<S: BuildHasher + Sync>(
    cmd: &str,
    cwd: Option<&Path>,
    env: &HashMap<String, String, S>,
    timeout: Duration,
) -> bool {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(cmd).envs(env);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    // Suppress output
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());

    let spawn_permit = crate::process_spawn::ProcessSpawnBarrier::global().enter_spawn();
    let spawn_result = command.spawn();
    drop(spawn_permit);

    let mut child = match spawn_result {
        Ok(child) => child,
        Err(e) => {
            debug!("Command probe failed for '{}': {}", cmd, e);
            return false;
        }
    };

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            debug!("Command probe failed for '{}': {}", cmd, e);
            false
        }
        Err(_) => {
            if let Err(e) = child.kill().await {
                debug!("Failed to kill timed out command probe '{}': {}", cmd, e);
            }
            debug!("Command probe timed out for '{}'", cmd);
            false
        }
    }
}
