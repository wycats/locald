use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand};
use tokio::process::Command;
use tracing::info;

pub struct TestContext {
    pub root: tempfile::TempDir,
    pub locald_bin: PathBuf,
    pub daemon_process: Option<Child>,
    pub socket_path: PathBuf,
    pub sandbox: String,
}

impl TestContext {
    pub async fn new() -> Result<Self> {
        let root = tempfile::tempdir()?;
        #[allow(deprecated)]
        let locald_bin = assert_cmd::Command::cargo_bin("locald")
            .map_err(|e| anyhow::anyhow!("Failed to find locald binary: {}", e))?
            .get_program()
            .into();

        let socket_path = root.path().join("locald.sock");

        let tmp_name = root
            .path()
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_else(|| "tmp".into());
        let tmp_name_sanitized: String = tmp_name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let sandbox = format!("e2e-{}-{}", std::process::id(), tmp_name_sanitized);

        Ok(Self {
            root,
            locald_bin,
            daemon_process: None,
            socket_path,
            sandbox,
        })
    }

    pub async fn start_daemon(&mut self) -> Result<()> {
        info!("Starting daemon at {}", self.socket_path.display());

        // We need to set XDG_RUNTIME_DIR or similar to control socket path?
        // Or use the --sandbox flag if we implemented it fully?
        // Phase 32 implemented --sandbox. Let's use that.

        #[allow(clippy::disallowed_methods)]
        let child = StdCommand::new(&self.locald_bin)
            .arg("server")
            .arg("start")
            .env("LOCALD_SOCKET", &self.socket_path)
            .env("LOCALD_SANDBOX_ACTIVE", "1")
            .env("LOCALD_SANDBOX_NAME", &self.sandbox)
            // Isolate state for the daemon.
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .stdout(std::fs::File::create(self.root.path().join("daemon.out"))?)
            .stderr(std::fs::File::create(self.root.path().join("daemon.err"))?)
            .spawn()
            .context("Failed to spawn daemon")?;

        self.daemon_process = Some(child);

        // Wait for socket
        self.wait_for_socket().await?;

        Ok(())
    }

    async fn wait_for_socket(&self) -> Result<()> {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            if self.socket_path.exists() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        anyhow::bail!("Timed out waiting for daemon socket")
    }

    pub async fn run_cli(&self, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new(&self.locald_bin)
            .args(args)
            .env("LOCALD_SOCKET", &self.socket_path)
            .env("LOCALD_SANDBOX_ACTIVE", "1")
            .env("LOCALD_SANDBOX_NAME", &self.sandbox)
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .output()
            .await
            .context("Failed to run CLI command")?;

        Ok(output)
    }

    pub async fn create_project(&self, name: &str, config_content: &str) -> Result<PathBuf> {
        let project_path = self.root.path().join(name);
        tokio::fs::create_dir_all(&project_path).await?;
        tokio::fs::write(project_path.join("locald.toml"), config_content).await?;
        Ok(project_path)
    }

    pub async fn dump_logs(&self) -> Result<()> {
        let stdout = tokio::fs::read_to_string(self.root.path().join("daemon.out"))
            .await
            .unwrap_or_default();
        let stderr = tokio::fs::read_to_string(self.root.path().join("daemon.err"))
            .await
            .unwrap_or_default();
        println!("=== DAEMON STDOUT ===\n{}\n=====================", stdout);
        println!("=== DAEMON STDERR ===\n{}\n=====================", stderr);
        Ok(())
    }
}

impl Drop for TestContext {
    #[allow(clippy::disallowed_methods)]
    fn drop(&mut self) {
        if std::thread::panicking() {
            let stdout_path = self.root.path().join("daemon.out");
            let stderr_path = self.root.path().join("daemon.err");

            if let Ok(content) = std::fs::read_to_string(&stdout_path) {
                println!("=== DAEMON STDOUT ===\n{}\n=====================", content);
            }
            if let Ok(content) = std::fs::read_to_string(&stderr_path) {
                println!("=== DAEMON STDERR ===\n{}\n=====================", content);
            }
        }

        if let Some(mut child) = self.daemon_process.take() {
            // Attempt a graceful shutdown so LLVM coverage profiles flush.
            let _ = StdCommand::new(&self.locald_bin)
                .arg("server")
                .arg("shutdown")
                .env("LOCALD_SOCKET", &self.socket_path)
                .env("LOCALD_SANDBOX_ACTIVE", "1")
                .env("LOCALD_SANDBOX_NAME", &self.sandbox)
                .env("XDG_DATA_HOME", self.root.path().join("data"))
                .env("XDG_CONFIG_HOME", self.root.path().join("config"))
                .env("XDG_STATE_HOME", self.root.path().join("state"))
                .status();

            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(5) {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    Err(_) => break,
                }
            }

            // Avoid panicking during unwinding.
            if !std::thread::panicking() {
                panic!("daemon did not exit after shutdown");
            }
        }
    }
}
