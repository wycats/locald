//! Integration test: dependency injection via `${services.*}` interpolation.
//!
//! Spawns a sandboxed daemon, registers a project with inter-service deps,
//! and verifies the interpolated env var appears in logs.

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::process::{Child, Command};
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

/// Guard that kills the daemon on drop (including panics).
#[cfg(target_os = "linux")]
struct DaemonGuard {
    child: Child,
    bin: std::path::PathBuf,
    sandbox: String,
    env_vars: Vec<(&'static str, String)>,
}

#[cfg(target_os = "linux")]
impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = Command::new(&self.bin)
            .envs(self.env_vars.clone())
            .arg(format!("--sandbox={}", self.sandbox))
            .arg("server")
            .arg("shutdown")
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[cfg(target_os = "linux")]
fn test_dependency_injection() {
    let root = tempfile::tempdir().expect("failed to create temp dir");
    let project_dir = root.path().join("project");
    fs::create_dir(&project_dir).unwrap();

    let toml = r#"
[project]
name = "dep-test"

[services.api]
command = "python3 -m http.server $PORT"

[services.web]
command = "echo API_URL=$API_URL; python3 -m http.server $PORT"
depends_on = ["api"]
[services.web.env]
API_URL = "${services.api.url}"
"#;
    fs::write(project_dir.join("locald.toml"), toml).unwrap();

    let locald_bin = assert_cmd::cargo::cargo_bin!("locald").to_path_buf();
    let sandbox = format!("dep-test-{}", std::process::id());

    let home = root.path();
    let env_vars: Vec<(&str, String)> = vec![
        ("HOME", home.to_string_lossy().to_string()),
        (
            "XDG_DATA_HOME",
            home.join(".local/share").to_string_lossy().to_string(),
        ),
        (
            "XDG_CONFIG_HOME",
            home.join(".config").to_string_lossy().to_string(),
        ),
        ("LOCALD_HTTP_PORT", "0".to_string()),
        ("LOCALD_HTTPS_PORT", "0".to_string()),
    ];

    // Start daemon with stdout/stderr redirected to a log file (not inherited pipes).
    let log_path = root.path().join("locald.log");
    let log_file = fs::File::create(&log_path).expect("failed to create log file");

    let child = Command::new(&locald_bin)
        .envs(env_vars.clone())
        .arg(format!("--sandbox={}", sandbox))
        .arg("server")
        .arg("start")
        .stdout(log_file.try_clone().unwrap())
        .stderr(log_file)
        .spawn()
        .expect("failed to spawn daemon");

    let _guard = DaemonGuard {
        child,
        bin: locald_bin.clone(),
        sandbox: sandbox.clone(),
        env_vars: env_vars.clone(),
    };

    // Wait for daemon to respond to ping.
    let mut ready = false;
    for _ in 0..50 {
        let ok = Command::new(&locald_bin)
            .envs(env_vars.clone())
            .arg(format!("--sandbox={}", sandbox))
            .arg("ping")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "daemon failed to become ready");

    // Register project. In production `locald up` stays attached to stream logs;
    // the hidden test flag lets this command return after registration.
    let status = Command::new(&locald_bin)
        .envs(env_vars.clone())
        .arg(format!("--sandbox={}", sandbox))
        .arg("up")
        .arg("--exit-after-register")
        .arg(&project_dir)
        .status()
        .expect("failed to run locald up");
    assert!(status.success(), "locald up failed");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut found = false;
    while Instant::now() < deadline {
        let output = Command::new(&locald_bin)
            .envs(env_vars.clone())
            .arg(format!("--sandbox={}", sandbox))
            .arg("logs")
            .arg("dep-test:web")
            .output()
            .expect("failed to get logs");

        found = String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.contains("API_URL=http://localhost:"));
        if found {
            break;
        }

        thread::sleep(Duration::from_millis(250));
    }

    assert!(found, "expected API_URL interpolation in web service logs")
}
