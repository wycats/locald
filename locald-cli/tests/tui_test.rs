//! End-to-end test for the interactive TUI progress renderer.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// A test context that sets up a sandboxed environment for `locald`.
#[derive(Debug)]
pub struct TestContext {
    /// Temporary directory for XDG_DATA_HOME, XDG_CONFIG_HOME, etc.
    pub root: TempDir,
    /// Path to the `locald` binary.
    pub locald_bin: PathBuf,
    /// Unique sandbox name for this test context (prevents parallel-test collisions).
    pub sandbox: String,
    /// The running daemon process (if any).
    pub daemon: Option<Child>,
}

impl TestContext {
    /// Create a new test context.
    /// This will compile the binaries if they are not up to date (handled by cargo test).
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("failed to create temp dir");

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

        // Find binaries
        let locald_bin = assert_cmd::cargo::cargo_bin!("locald").to_path_buf();

        Self {
            root,
            locald_bin,
            sandbox,
            daemon: None,
        }
    }

    /// Get the environment variables for the sandbox.
    pub fn env(&self) -> Vec<(&str, String)> {
        let home = self.root.path();
        let mut envs = vec![
            ("HOME", home.to_string_lossy().to_string()),
            (
                "XDG_DATA_HOME",
                home.join(".local/share").to_string_lossy().to_string(),
            ),
            (
                "XDG_CONFIG_HOME",
                home.join(".config").to_string_lossy().to_string(),
            ),
            (
                "XDG_CACHE_HOME",
                home.join(".cache").to_string_lossy().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR",
                home.join(".run").to_string_lossy().to_string(),
            ),
        ];

        // Inherit PATH
        if let Ok(path) = env::var("PATH") {
            envs.push(("PATH", path));
        }

        envs
    }

    /// Start the daemon in the background.
    pub fn start_daemon(&mut self) {
        if self.daemon.is_some() {
            panic!("daemon already running");
        }

        let log_path = self.root.path().join("locald.log");
        let log_file = fs::File::create(&log_path).expect("failed to create log file");

        // Use StdCommand for spawning background process
        let mut cmd = StdCommand::new(&self.locald_bin);
        cmd.envs(self.env());
        cmd.arg(format!("--sandbox={}", self.sandbox));
        cmd.arg("server").arg("start");
        cmd.stdout(log_file.try_clone().unwrap());
        cmd.stderr(log_file);

        let child = cmd.spawn().expect("failed to spawn daemon");
        self.daemon = Some(child);

        // Wait for socket to be ready
        self.wait_for_ready();
    }

    /// Wait until the daemon responds to `locald ping`.
    pub fn wait_for_ready(&self) {
        let mut attempts = 0;
        while attempts < 50 {
            let mut cmd = StdCommand::new(&self.locald_bin);
            cmd.envs(self.env());
            cmd.arg(format!("--sandbox={}", self.sandbox));
            cmd.arg("ping");

            if cmd.status().map(|s| s.success()).unwrap_or(false) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
            attempts += 1;
        }
        panic!("daemon failed to become ready");
    }

    /// Stop the daemon process (best-effort).
    pub fn stop_daemon(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            // Try graceful shutdown via CLI
            let mut cmd = StdCommand::new(&self.locald_bin);
            cmd.envs(self.env());
            cmd.arg(format!("--sandbox={}", self.sandbox));
            cmd.arg("server").arg("shutdown");
            let _ = cmd.status();

            // Wait for process to exit
            let _ = child.wait();
        }
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        self.stop_daemon();
    }
}

#[test]
fn test_tui_progress() {
    let mut ctx = TestContext::new();
    ctx.start_daemon();

    let project_dir = ctx.root.path().join("tui-test");
    fs::create_dir(&project_dir).unwrap();

    // Create a minimal Python project
    fs::write(
        project_dir.join("Procfile"),
        "web: python3 -m http.server $PORT",
    )
    .unwrap();
    fs::write(project_dir.join("requirements.txt"), "").unwrap();

    // Construct command for rexpect
    let mut cmd = StdCommand::new(&ctx.locald_bin);
    cmd.envs(ctx.env());
    cmd.arg("up");
    cmd.arg(&project_dir);
    cmd.arg(format!("--sandbox={}", ctx.sandbox));

    // Increase timeout for build
    let mut p = rexpect::session::spawn_command(cmd, Some(30000)).expect("failed to spawn rexpect");

    // Expect TUI output
    // We look for key phrases that indicate the progress renderer is active
    p.exp_string("Loading configuration")
        .expect("failed to find Loading configuration");
    p.exp_string("Starting service")
        .expect("failed to find Starting service");

    // Wait for process to exit
    p.process.wait().expect("failed to wait");
}
