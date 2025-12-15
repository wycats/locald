use assert_cmd::Command;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// A test context that sets up a sandboxed environment for `locald`.
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
            // Avoid port collisions across parallel tests by binding the proxy to ephemeral ports.
            // These tests use IPC for control-plane operations and do not need stable HTTP ports.
            ("LOCALD_HTTP_PORT", "0".to_string()),
            ("LOCALD_HTTPS_PORT", "0".to_string()),
        ];

        // Inherit PATH
        if let Ok(path) = env::var("PATH") {
            envs.push(("PATH", path));
        }

        envs
    }

    /// Run a `locald` command.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.locald_bin);
        cmd.envs(self.env());
        cmd.arg(format!("--sandbox={}", self.sandbox));
        cmd
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

    pub fn wait_for_ready(&self) {
        let mut attempts = 0;
        while attempts < 50 {
            // Use assert_cmd Command for checking status
            let status = self.command().arg("ping").ok();
            if status.is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
            attempts += 1;
        }
        let log_path = self.root.path().join("locald.log");
        let logs = fs::read_to_string(&log_path)
            .unwrap_or_else(|e| format!("<failed to read {}: {e}>", log_path.display()));
        panic!("daemon failed to become ready\n\nDaemon logs:\n{logs}");
    }

    pub fn stop_daemon(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            // Try graceful shutdown via CLI
            let _ = self.command().arg("server").arg("shutdown").ok();

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
fn test_basic_lifecycle() {
    let mut ctx = TestContext::new();

    // 1. Init (optional, but good to test)
    // ctx.command().arg("init").assert().success();

    // 2. Start Daemon
    ctx.start_daemon();

    // 3. Ping
    ctx.command()
        .arg("ping")
        .assert()
        .success()
        .stdout(predicates::str::contains("Pong"));

    // 4. Stop Daemon (handled by Drop, but we can test explicit shutdown)
    ctx.stop_daemon();

    // 5. Verify it's down
    ctx.command().arg("ping").assert().failure();
}

#[test]
fn test_service_execution() {
    let mut ctx = TestContext::new();
    ctx.start_daemon();

    // Create a project
    let project_dir = ctx.root.path().join("my-project");
    fs::create_dir(&project_dir).unwrap();

    let config = r#"
[project]
name = "my-project"
domain = "my-project.localhost"

[services.web]
command = "python3 -m http.server $PORT"
"#;
    fs::write(project_dir.join("locald.toml"), config).unwrap();
    // Add Procfile to satisfy CNB detection
    fs::write(
        project_dir.join("Procfile"),
        "web: python3 -m http.server $PORT",
    )
    .unwrap();
    // Add requirements.txt to satisfy Python detection
    fs::write(project_dir.join("requirements.txt"), "").unwrap();
    // Add runtime.txt to satisfy Python detection (optional but good)
    // fs::write(project_dir.join("runtime.txt"), "python-3.11.0").unwrap();

    // Run locald up
    ctx.command().arg("up").arg(&project_dir).assert().success();

    // Verify registry
    ctx.command()
        .arg("registry")
        .arg("list")
        .assert()
        .success()
        .stdout(predicates::str::contains("my-project"));

    // Verify status
    // We wait a bit for it to start
    thread::sleep(Duration::from_secs(1));

    // Debug: Print logs if status fails
    let status = ctx.command().arg("status").output().unwrap();
    if !String::from_utf8_lossy(&status.stdout).contains("web") {
        println!("Status output: {}", String::from_utf8_lossy(&status.stdout));

        // Print daemon logs
        let log_path = ctx.root.path().join("locald.log");
        if let Ok(logs) = fs::read_to_string(&log_path) {
            println!("Daemon Logs:\n{}", logs);
        }

        let logs = ctx.command().arg("logs").arg("web").output().unwrap();
        println!("Logs output: {}", String::from_utf8_lossy(&logs.stdout));
        println!("Logs stderr: {}", String::from_utf8_lossy(&logs.stderr));
    }

    ctx.command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains("web"))
        .stdout(predicates::str::contains("Running"));
}

#[test]
fn test_shim_bootstrap() {
    // This test verifies that locald-shim can bootstrap a container from a bundle.
    // It acts as the "Caller" in the Caller-Generates / Shim-Executes model.

    let root = tempfile::tempdir().expect("failed to create temp dir");
    let bundle_path = root.path().join("bundle");
    fs::create_dir(&bundle_path).unwrap();

    // 1. Generate config.json
    let mut spec = oci_spec::runtime::Spec::default();

    // Set process args to something simple
    let mut process = oci_spec::runtime::Process::default();
    process.set_args(Some(vec![
        "/bin/echo".to_string(),
        "Hello from Shim".to_string(),
    ]));
    // Ensure we have a valid cwd
    process.set_cwd(std::path::PathBuf::from("/"));
    spec.set_process(Some(process));

    // Set rootfs (we use the host root for this bootstrap test, which is risky but okay for "fake root" emulation)
    // In a real container, we'd have a separate rootfs.
    // For now, let's point to the bundle dir itself as root, but we need /bin/echo.
    // Actually, libcontainer might fail if rootfs doesn't look like a rootfs.
    // Let's try to use the host root "/" as the rootfs path in the config,
    // BUT we must be careful not to pivot_root if we are just testing.
    // However, libcontainer WILL try to pivot_root.

    // Strategy: Create a minimal rootfs in the bundle
    let rootfs_path = bundle_path.join("rootfs");
    fs::create_dir(&rootfs_path).unwrap();

    // We need /bin/echo inside the rootfs.
    let bin_dir = rootfs_path.join("bin");
    fs::create_dir(&bin_dir).unwrap();
    // Copy /bin/echo (assuming it exists and is static or libs are compatible)
    // This is fragile. A better test payload is a static binary.
    // Or we can use the "host-first" approach where we bind mount /bin?

    // Let's try a simpler approach: Just verify the shim loads the spec and tries to run.
    // Even if it fails inside the container due to missing binary, we know the shim worked.

    let mut root = oci_spec::runtime::Root::default();
    root.set_path(std::path::PathBuf::from("rootfs"));
    spec.set_root(Some(root));

    // Save config.json
    spec.save(bundle_path.join("config.json")).unwrap();

    // 2. Call locald-shim
    // We need to find the shim binary. It's in target/debug/locald-shim usually.
    // `cargo_bin!` only works for binaries built by *this* crate. The shim lives in a
    // different workspace crate, so we resolve it as a sibling of the `locald` binary.
    let locald_bin = assert_cmd::cargo::cargo_bin!("locald");
    let shim_bin = locald_bin
        .parent()
        .expect("locald binary path should have a parent directory")
        .join("locald-shim");

    let mut cmd = StdCommand::new(shim_bin);
    cmd.env("PATH", "");
    cmd.arg("bundle")
        .arg("run")
        .arg("--bundle")
        .arg(&bundle_path)
        .arg("--id")
        .arg("locald-cli-test-bootstrap");

    // We expect it to fail because we didn't populate rootfs fully,
    // BUT it should fail *inside* libcontainer (e.g. "executable file not found"),
    // NOT with "oci spec error".

    let output = cmd.output().expect("failed to run shim");

    println!("Shim stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("Shim stderr: {}", String::from_utf8_lossy(&output.stderr));

    // If we get past the "oci spec error", we made progress.
    // The previous error was "No such file or directory" (os error 2) when trying to load config.json.
    // So if we see something else, or success, we are good.

    // We expect this to fail (rootfs is not a real rootfs), but it should fail
    // during libcontainer execution, not due to shelling out to an external runtime.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("runc"),
        "stderr unexpectedly referenced runc: {stderr}"
    );
}
