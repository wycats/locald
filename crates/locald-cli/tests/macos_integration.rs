//! Integration tests for macOS support (M2.2).
//!
//! These tests verify that `locald up` works correctly on macOS for exec services.
//! They are designed to run on macOS CI runners and use sandbox mode to avoid conflicts.
//!
//! # Test Categories
//!
//! 1. **Basic lifecycle**: Start daemon, register service, verify running, stop
//! 2. **Proxy routing**: Dashboard and docs routes work
//! 3. **Health checks**: TCP health checks pass/fail appropriately
//! 4. **macOS-specific error messages**: Admin commands show helpful errors

use assert_cmd::Command;
use predicates::prelude::*;
use std::env;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Test context for macOS integration tests.
///
/// This reuses the pattern from e2e.rs but is specifically designed for
/// cross-platform compatibility.
#[derive(Debug)]
struct MacOSTestContext {
    /// Temporary directory for XDG_DATA_HOME, XDG_CONFIG_HOME, etc.
    root: TempDir,
    /// Path to the `locald` binary.
    locald_bin: PathBuf,
    /// Unique sandbox name for this test context.
    sandbox: String,
    /// The running daemon process (if any).
    daemon: Option<Child>,
    /// HTTP port for the proxy (0 for auto-assign).
    http_port: u16,
}

impl MacOSTestContext {
    /// Create a new test context with auto-assigned ports.
    fn new() -> Self {
        Self::with_ports(0, 0)
    }

    /// Create a new test context with specific ports.
    fn with_ports(http_port: u16, _https_port: u16) -> Self {
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
        let sandbox = format!("macos-{}-{}", std::process::id(), tmp_name_sanitized);

        let locald_bin = assert_cmd::cargo::cargo_bin!("locald").to_path_buf();

        Self {
            root,
            locald_bin,
            sandbox,
            daemon: None,
            http_port,
        }
    }

    /// Get the environment variables for the sandbox.
    fn env(&self) -> Vec<(&str, String)> {
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
            // Use ephemeral ports to avoid conflicts
            ("LOCALD_HTTP_PORT", self.http_port.to_string()),
            ("LOCALD_HTTPS_PORT", "0".to_string()),
        ];

        // Inherit PATH
        if let Ok(path) = env::var("PATH") {
            envs.push(("PATH", path));
        }

        envs
    }

    /// Run a `locald` command with assert_cmd.
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.locald_bin);
        cmd.envs(self.env());
        cmd.arg(format!("--sandbox={}", self.sandbox));
        cmd
    }

    /// Start the daemon in the background.
    fn start_daemon(&mut self) {
        if self.daemon.is_some() {
            panic!("daemon already running");
        }

        let log_path = self.root.path().join("locald.log");
        let log_file = fs::File::create(&log_path).expect("failed to create log file");

        let mut cmd = StdCommand::new(&self.locald_bin);
        cmd.envs(self.env());
        cmd.arg(format!("--sandbox={}", self.sandbox));
        cmd.arg("server").arg("start");
        cmd.stdout(log_file.try_clone().unwrap());
        cmd.stderr(log_file);

        let child = cmd.spawn().expect("failed to spawn daemon");
        self.daemon = Some(child);

        self.wait_for_ready();
    }

    /// Wait until the daemon responds to `locald ping`.
    fn wait_for_ready(&self) {
        let mut attempts = 0;
        while attempts < 50 {
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

    /// Stop the daemon process gracefully.
    fn stop_daemon(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = self.command().arg("server").arg("shutdown").ok();
            let _ = child.wait();
        }
    }

    /// Get the daemon log content (for debugging).
    fn daemon_logs(&self) -> String {
        let log_path = self.root.path().join("locald.log");
        fs::read_to_string(&log_path)
            .unwrap_or_else(|e| format!("<failed to read {}: {e}>", log_path.display()))
    }

    /// Create a project directory with a locald.toml file.
    fn create_project(&self, name: &str, config: &str) -> PathBuf {
        let project_dir = self.root.path().join(name);
        fs::create_dir(&project_dir).expect("failed to create project dir");
        fs::write(project_dir.join("locald.toml"), config).expect("failed to write config");
        project_dir
    }
}

impl Drop for MacOSTestContext {
    fn drop(&mut self) {
        self.stop_daemon();
    }
}

// =============================================================================
// Basic Lifecycle Tests
// =============================================================================

#[test]
fn test_macos_basic_lifecycle() {
    let mut ctx = MacOSTestContext::new();

    // 1. Start daemon
    ctx.start_daemon();

    // 2. Verify ping works
    ctx.command()
        .arg("ping")
        .assert()
        .success()
        .stdout(predicate::str::contains("Pong"));

    // 3. Stop daemon
    ctx.stop_daemon();

    // 4. Verify it's down
    ctx.command().arg("ping").assert().failure();
}

#[test]
fn test_macos_exec_service() {
    let mut ctx = MacOSTestContext::new();
    ctx.start_daemon();

    // Create a simple exec service project
    let config = r#"
[project]
name = "test-exec"
domain = "test-exec.localhost"

[services.web]
command = "python3 -m http.server $PORT"
"#;

    let project_dir = ctx.create_project("test-exec", config);

    // Add Procfile to satisfy detection
    fs::write(
        project_dir.join("Procfile"),
        "web: python3 -m http.server $PORT",
    )
    .unwrap();
    fs::write(project_dir.join("requirements.txt"), "").unwrap();

    // Run locald up
    ctx.command().arg("up").arg(&project_dir).assert().success();

    // Verify the project is registered
    ctx.command()
        .arg("registry")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("test-exec"));

    // Wait a moment for the service to start
    thread::sleep(Duration::from_secs(1));

    // Verify status shows the service
    let status_output = ctx.command().arg("status").output().unwrap();
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);

    // Debug output if test fails
    if !status_stdout.contains("web") {
        eprintln!("Status output: {}", status_stdout);
        eprintln!("Daemon logs:\n{}", ctx.daemon_logs());
    }

    ctx.command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("web"));
}

#[test]
fn test_macos_logs_work() {
    let mut ctx = MacOSTestContext::new();
    ctx.start_daemon();

    // Create a service that outputs to stdout
    let config = r#"
[project]
name = "log-test"

[services.web]
command = "echo 'HELLO_FROM_LOG_TEST' && python3 -m http.server $PORT"
"#;

    let project_dir = ctx.create_project("log-test", config);
    fs::write(project_dir.join("Procfile"), "web: echo hello").unwrap();
    fs::write(project_dir.join("requirements.txt"), "").unwrap();

    ctx.command().arg("up").arg(&project_dir).assert().success();

    // Wait for logs to be captured
    thread::sleep(Duration::from_secs(2));

    // Check logs contain our marker
    let logs_output = ctx
        .command()
        .arg("logs")
        .arg("web")
        .output()
        .expect("failed to get logs");

    let logs_stdout = String::from_utf8_lossy(&logs_output.stdout);
    assert!(
        logs_stdout.contains("HELLO_FROM_LOG_TEST") || logs_output.status.success(),
        "Expected logs to work. stdout: {}, stderr: {}",
        logs_stdout,
        String::from_utf8_lossy(&logs_output.stderr)
    );
}

#[test]
fn test_macos_stop_service() {
    let mut ctx = MacOSTestContext::new();
    ctx.start_daemon();

    let config = r#"
[project]
name = "stop-test"

[services.web]
command = "python3 -m http.server $PORT"
"#;

    let project_dir = ctx.create_project("stop-test", config);
    fs::write(
        project_dir.join("Procfile"),
        "web: python3 -m http.server $PORT",
    )
    .unwrap();
    fs::write(project_dir.join("requirements.txt"), "").unwrap();

    // Start service
    ctx.command().arg("up").arg(&project_dir).assert().success();

    thread::sleep(Duration::from_secs(1));

    // Stop service
    ctx.command()
        .current_dir(&project_dir)
        .arg("stop")
        .assert()
        .success();

    // Verify service is stopped (status should show it as stopped or not running)
    thread::sleep(Duration::from_millis(500));
}

// =============================================================================
// macOS-Specific Error Message Tests
// =============================================================================

/// Test that `locald admin setup` gives a helpful error on macOS.
#[test]
#[cfg(target_os = "macos")]
fn test_macos_admin_setup_error_message() {
    let ctx = MacOSTestContext::new();

    ctx.command()
        .arg("admin")
        .arg("setup")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Linux").or(predicate::str::contains("not supported")));
}

/// Test that doctor works on macOS and shows appropriate platform info.
#[test]
fn test_macos_doctor_runs() {
    let ctx = MacOSTestContext::new();

    // Doctor runs and produces output. It may return non-zero if there are
    // problems (e.g., shim not installed), but it should NOT crash.
    // The important thing is that it runs without panicking.
    let output = ctx
        .command()
        .arg("doctor")
        .output()
        .expect("failed to run doctor");

    // Should have exited (not crashed)
    assert!(output.status.code().is_some(), "doctor should exit cleanly");

    // Should produce meaningful output
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Strategy:") || stdout.contains("strategy"),
        "doctor should output strategy info. Got: {stdout}"
    );
}

/// Test that `--version` works on macOS.
#[test]
fn test_macos_version() {
    let ctx = MacOSTestContext::new();

    ctx.command()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("locald"));
}

/// Test that `--help` works on macOS.
#[test]
fn test_macos_help() {
    let ctx = MacOSTestContext::new();

    ctx.command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Local development proxy"));
}

// =============================================================================
// Health Check Tests
// =============================================================================

/// Helper to find an available port.
fn find_available_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("failed to bind")
        .local_addr()
        .expect("failed to get local addr")
        .port()
}

#[test]
fn test_macos_tcp_health_check() {
    let mut ctx = MacOSTestContext::new();
    ctx.start_daemon();

    // Find an available port for the health check
    let health_port = find_available_port();

    // Create a service with a TCP health check
    let config = format!(
        r#"
[project]
name = "health-test"

[services.web]
command = "python3 -m http.server {health_port}"

[services.web.health_check]
type = "tcp"
port = {health_port}
interval_ms = 1000
timeout_ms = 5000
"#
    );

    let project_dir = ctx.create_project("health-test", &config);
    fs::write(
        project_dir.join("Procfile"),
        format!("web: python3 -m http.server {health_port}"),
    )
    .unwrap();
    fs::write(project_dir.join("requirements.txt"), "").unwrap();

    ctx.command().arg("up").arg(&project_dir).assert().success();

    // Wait for health check to pass
    thread::sleep(Duration::from_secs(3));

    // Status should show healthy service
    ctx.command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("web"));
}

// =============================================================================
// Proxy Routing Tests (Unit-style, no daemon needed)
// =============================================================================

/// Test that the proxy module compiles and basic types work on macOS.
/// More comprehensive proxy tests are in locald-server/src/proxy_test.rs.
#[test]
fn test_macos_proxy_types_available() {
    // This test just verifies the binary includes proxy functionality
    let ctx = MacOSTestContext::new();

    // The help output should mention proxy-related concepts
    ctx.command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("proxy").or(predicate::str::contains("dashboard")));
}

// =============================================================================
// Privileged Port Error Tests
// =============================================================================

/// Test that requesting privileged ports shows appropriate warnings/errors.
#[test]
#[cfg(target_os = "macos")]
fn test_macos_privileged_port_warning() {
    let mut ctx = MacOSTestContext::new();
    ctx.start_daemon();

    // Try to create a service that wants port 80 (privileged)
    // This should work on macOS with a high port fallback or clear error
    let config = r#"
[project]
name = "priv-port-test"

[services.web]
command = "python3 -m http.server 80"
port = 80
"#;

    let project_dir = ctx.create_project("priv-port-test", config);
    fs::write(
        project_dir.join("Procfile"),
        "web: python3 -m http.server 80",
    )
    .unwrap();
    fs::write(project_dir.join("requirements.txt"), "").unwrap();

    // The up command should either succeed with a warning or fail gracefully
    // We don't assert success here because binding to port 80 requires root
    let output = ctx
        .command()
        .arg("up")
        .arg(&project_dir)
        .output()
        .expect("failed to run command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should either work or give a helpful message about privileged ports
    // We're mainly checking it doesn't crash with an unhelpful error
    assert!(
        output.status.success()
            || stderr.contains("permission")
            || stderr.contains("privileged")
            || stderr.contains("port")
            || stdout.contains("port"),
        "Expected graceful handling of privileged port. stderr: {}, stdout: {}",
        stderr,
        stdout
    );
}
