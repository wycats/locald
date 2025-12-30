use anyhow::{Context, Result};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::Sender;
use tracing::info;

/// Execute a container using locald-shim in the foreground.
///
/// This command blocks until the container exits.
pub async fn run(
    bundle_path: &Path,
    container_id: &str,
    log_tx: Option<Sender<(String, bool)>>,
) -> Result<()> {
    info!(
        "Running container {} from bundle {:?}",
        container_id, bundle_path
    );

    // Daemon-safe: require an already-installed privileged shim.
    let mut cmd = locald_utils::shim::tokio_command_privileged()?;
    cmd.arg("bundle")
        .arg("run")
        .arg("--bundle")
        .arg(bundle_path)
        .arg("--id")
        .arg(container_id);

    info!(
        "Executing shim: locald-shim bundle run --bundle {} --id {}",
        bundle_path.display(),
        container_id
    );

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to execute locald-shim")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let tx_stdout = tx.clone();
    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut buf = [0u8; 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break, // EOF or Error
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    if tx_stdout.send((chunk, false)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let tx_stderr = tx.clone();
    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = [0u8; 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break, // EOF or Error
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    if tx_stderr.send((chunk, true)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Drop the original sender so the receiver closes when both tasks are done
    drop(tx);

    while let Some((line, is_stderr)) = rx.recv().await {
        if let Some(tx) = &log_tx
            && tx.send((line, is_stderr)).await.is_err()
        {
            // Receiver closed, but we should keep running the container?
            // Or maybe we should stop?
            // For now, just ignore and keep running.
        }
    }

    let status = child.wait().await?;

    if !status.success() {
        anyhow::bail!("locald-shim exited with status: {status}");
    }

    Ok(())
}

/// Delete a container.
pub fn delete(container_id: &str) -> Result<()> {
    info!("Deleting container {} (No-op with Fat Shim)", container_id);
    Ok(())
}
