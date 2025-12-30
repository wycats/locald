use super::progress::BuildProgress;
use anyhow::Result;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::LinesStream;

#[derive(Debug)]
pub struct Lifecycle {
    pub root: PathBuf,
}

impl Lifecycle {
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn analyzer_path(&self) -> PathBuf {
        self.root.join("analyzer")
    }
    pub fn detector_path(&self) -> PathBuf {
        self.root.join("detector")
    }
    pub fn restorer_path(&self) -> PathBuf {
        self.root.join("restorer")
    }
    pub fn builder_path(&self) -> PathBuf {
        self.root.join("builder")
    }
    pub fn exporter_path(&self) -> PathBuf {
        self.root.join("exporter")
    }
    pub fn launcher_path(&self) -> PathBuf {
        self.root.join("launcher")
    }

    pub async fn run_phase(
        &self,
        phase_name: &str,
        binary_path: PathBuf,
        args: &[&str],
        env: &[(&str, &str)],
        progress: &impl BuildProgress,
    ) -> Result<()> {
        progress.phase_started(phase_name);

        let mut cmd = Command::new(&binary_path);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // TODO: In the future, we might want to use the Runtime trait here
        // to support containerized execution. For now, we use tokio::process::Command
        // which corresponds to the "Native" strategy.

        let mut child = cmd.spawn().map_err(|e| {
            let err = format!("Failed to spawn {phase_name}: {e}");
            progress.phase_failed(phase_name, &err);
            anyhow::anyhow!(err)
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

        let stdout_stream = LinesStream::new(BufReader::new(stdout).lines());
        let stderr_stream = LinesStream::new(BufReader::new(stderr).lines());

        // Merge stdout and stderr into a single stream
        let mut stream = stdout_stream.merge(stderr_stream);

        while let Some(line_res) = stream.next().await {
            match line_res {
                Ok(line) => progress.phase_output(phase_name, &line),
                Err(e) => {
                    // Log error but continue? Or abort?
                    // Usually IO errors on stdout/stderr are fatal or mean the process died.
                    progress.phase_output(phase_name, &format!("Error reading output: {e}"));
                }
            }
        }

        let status = child.wait().await?;

        if status.success() {
            progress.phase_completed(phase_name);
            Ok(())
        } else {
            let err = format!("Phase {phase_name} failed with status {status}");
            progress.phase_failed(phase_name, &err);
            anyhow::bail!(err);
        }
    }
}
