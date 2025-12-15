use std::path::Path;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;
use tracing::debug;

/// Checks if an HTTP(S) URL is reachable and returns a success status code.
pub async fn check_http(url: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build();

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
pub async fn check_tcp(addr: &str) -> bool {
    match TcpStream::connect(addr).await {
        Ok(_) => true,
        Err(e) => {
            debug!("TCP probe failed for {}: {}", addr, e);
            false
        }
    }
}

/// Checks if a command executes successfully (exit code 0).
pub async fn check_command(cmd: &str, cwd: Option<&Path>) -> bool {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    // Suppress output
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());

    match command.status().await {
        Ok(status) => status.success(),
        Err(e) => {
            debug!("Command probe failed for '{}': {}", cmd, e);
            false
        }
    }
}
