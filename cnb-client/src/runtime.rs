use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info};

#[derive(Debug, Copy, Clone)]
pub struct ShimRuntime;

impl ShimRuntime {
    pub async fn run_container(
        bundle_path: &Path,
        container_id: &str,
        verbose: bool,
        log_dir: Option<&Path>,
        log_callback: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Result<()> {
        // Locate the shim
        let shim_path = Self::find_shim()?;

        debug!(
            "Executing shim at {}: bundle run --bundle {} --id {}",
            shim_path.display(),
            bundle_path.display(),
            container_id
        );

        let mut child = Command::new(&shim_path)
            .env_remove("LD_LIBRARY_PATH")
            .arg("bundle")
            .arg("run")
            .arg("--bundle")
            .arg(bundle_path)
            .arg("--id")
            .arg(container_id)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to execute locald-shim bundle")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

        // Spawn tasks to read output concurrently
        let cb = log_callback.clone();
        let stdout_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            let mut log = Vec::new();
            while let Ok(Some(line)) = reader.next_line().await {
                // Heuristic: High-level CNB phases start with "===>"
                if verbose || should_show_line(&line) {
                    info!("{}", line);
                    if let Some(cb) = &cb {
                        cb(line.clone());
                    }
                } else {
                    debug!("[stdout] {line}");
                }
                log.push(format!("[stdout] {line}"));
            }
            log
        });

        let cb = log_callback.clone();
        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut log = Vec::new();
            while let Ok(Some(line)) = reader.next_line().await {
                debug!("[stderr] {line}");
                // Stderr is usually debug info in CNB, but if verbose, show it?
                // Or maybe just show it if it looks important?
                // For now, let's only stream stdout for "user logs" unless verbose is on?
                // Actually, if verbose is on, we probably want stderr too.
                #[allow(clippy::collapsible_if)]
                if verbose {
                    if let Some(cb) = &cb {
                        cb(line.clone());
                    }
                }
                log.push(format!("[stderr] {line}"));
            }
            log
        });

        let status = child.wait().await?;
        let stdout_log = stdout_handle.await.unwrap_or_default();
        let stderr_log = stderr_handle.await.unwrap_or_default();

        if !status.success() {
            // Crash Protocol: Write logs to file
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let default_crash_dir = PathBuf::from(".locald/crashes");
            let crash_dir = log_dir.unwrap_or(&default_crash_dir);

            if let Err(e) = tokio::fs::create_dir_all(crash_dir).await {
                error!("Failed to create crash directory: {}", e);
                // Fallback to dumping to error!
                for line in &stdout_log {
                    error!("{}", line);
                }
                for line in &stderr_log {
                    error!("{}", line);
                }
                return Err(anyhow::anyhow!(
                    "Container execution failed with status: {status}",
                ));
            }

            let crash_file_path = crash_dir.join(format!("crash-{container_id}-{timestamp}.log"));

            match tokio::fs::File::create(&crash_file_path).await {
                Ok(mut file) => {
                    use std::fmt::Write;
                    use tokio::io::AsyncWriteExt;
                    let mut content = String::new();
                    writeln!(
                        content,
                        "Command: bundle run --bundle {} --id {}",
                        bundle_path.display(),
                        container_id
                    )?;
                    writeln!(content, "Exit Status: {status}")?;
                    writeln!(content, "--- STDOUT ---")?;
                    for line in &stdout_log {
                        writeln!(content, "{line}")?;
                    }
                    writeln!(content, "--- STDERR ---")?;
                    for line in &stderr_log {
                        writeln!(content, "{line}")?;
                    }

                    if let Err(e) = file.write_all(content.as_bytes()).await {
                        error!("Failed to write crash log content: {}", e);
                    }

                    // Also log to error! so it's in the daemon logs
                    error!(
                        "Container execution failed. Crash log written to {}",
                        crash_file_path.display()
                    );

                    return Err(anyhow::anyhow!(
                        "Container execution failed (exit code {}). Details written to {}",
                        status.code().unwrap_or(-1),
                        crash_file_path.display()
                    ));
                }
                Err(e) => {
                    error!("Failed to write crash log: {}", e);
                    // Fallback
                    for line in &stdout_log {
                        error!("{}", line);
                    }
                    for line in &stderr_log {
                        error!("{}", line);
                    }
                    return Err(anyhow::anyhow!(
                        "Container execution failed with status: {status}",
                    ));
                }
            }
        }

        Ok(())
    }

    pub async fn cleanup_path(path: &Path) -> Result<()> {
        let shim_path = Self::find_shim()?;

        debug!("Delegating cleanup of {} to shim", path.display());

        let status = Command::new(&shim_path)
            .env_remove("LD_LIBRARY_PATH")
            .arg("admin")
            .arg("cleanup")
            .arg(path)
            .status()
            .await?;

        if !status.success() {
            return Err(anyhow::anyhow!("Shim cleanup failed with status: {status}",));
        }

        Ok(())
    }

    pub fn find_shim() -> Result<PathBuf> {
        locald_utils::shim::find_privileged()?
            .ok_or_else(|| anyhow::anyhow!(
                "Privileged locald-shim not found. Run `locald admin setup` (interactive) to install/repair the setuid shim."
            ))
    }
}

fn should_show_line(line: &str) -> bool {
    line.starts_with("===>")
}
