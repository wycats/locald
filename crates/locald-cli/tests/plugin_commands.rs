//! Tests for `locald plugin` subcommands.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn locald() -> Command {
    let bin = assert_cmd::cargo::cargo_bin!("locald");
    let mut cmd = Command::new(bin);
    // Skip shim verification in tests (we're testing CLI logic, not privileged setup)
    cmd.env("LOCALD_SKIP_SHIM_CHECK", "1");
    cmd
}

#[test]
fn plugin_install_project_copies_file_to_local_plugins_dir() {
    let root = tempfile::tempdir().expect("tempdir");

    let source = root.path().join("redis.component.wasm");
    fs::write(&source, b"not-a-real-wasm").expect("write source");

    let mut cmd = locald();
    cmd.current_dir(root.path());
    cmd.args(["plugin", "install"])
        .arg(source.as_os_str())
        .arg("--project");

    cmd.assert().success().stdout(contains("installed"));

    let installed = root
        .path()
        .join(".local")
        .join("plugins")
        .join("redis.component.wasm");

    assert!(
        installed.exists(),
        "expected {} to exist",
        installed.display()
    );
}

#[test]
fn plugin_install_project_sanitizes_name() {
    let root = tempfile::tempdir().expect("tempdir");

    let source = root.path().join("source.wasm");
    fs::write(&source, b"irrelevant").expect("write source");

    let mut cmd = locald();
    cmd.current_dir(root.path());
    cmd.args(["plugin", "install"])
        .arg(source.as_os_str())
        .args(["--project", "--name", "foo/bar.wasm"]);

    cmd.assert().success();

    let installed = root
        .path()
        .join(".local")
        .join("plugins")
        .join("foo-bar.wasm");

    assert!(
        installed.exists(),
        "expected {} to exist",
        installed.display()
    );
}

#[test]
fn plugin_install_project_downloads_from_http_url() {
    let root = tempfile::tempdir().expect("tempdir");

    // Serve a single HTTP response from a local ephemeral port.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    let payload = b"not-a-real-wasm-from-http".to_vec();
    let payload_len = payload.len();

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");

        // Read until end-of-headers. We don't need to parse the request.
        let mut req = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).expect("read");
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {payload_len}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(headers.as_bytes()).expect("write headers");
        stream.write_all(&payload).expect("write body");
        stream.flush().ok();
    });

    let url = format!("http://{addr}/redis.component.wasm");

    let mut cmd = locald();
    cmd.current_dir(root.path());
    cmd.args(["plugin", "install"]).arg(&url).arg("--project");

    cmd.assert().success().stdout(contains("installed"));

    let installed = root
        .path()
        .join(".local")
        .join("plugins")
        .join("redis.component.wasm");

    assert!(
        installed.exists(),
        "expected {} to exist",
        installed.display()
    );

    let got = fs::read(&installed).expect("read installed");
    assert_eq!(got, b"not-a-real-wasm-from-http");

    handle.join().expect("server thread");
}

#[test]
fn plugin_validate_errors_when_plugin_not_found() {
    let root = tempfile::tempdir().expect("tempdir");

    let mut cmd = locald();
    cmd.current_dir(root.path());
    cmd.args(["plugin", "validate", "does-not-exist", "--kind", "redis"]);

    cmd.assert()
        .failure()
        .stderr(contains("plugin 'does-not-exist' not found"));
}

#[test]
fn plugin_inspect_errors_when_plugin_not_found() {
    let root = tempfile::tempdir().expect("tempdir");

    let mut cmd = locald();
    cmd.current_dir(root.path());
    cmd.args(["plugin", "inspect", "does-not-exist", "--kind", "redis"]);

    cmd.assert()
        .failure()
        .stderr(contains("plugin 'does-not-exist' not found"));
}

#[test]
fn plugin_install_user_uses_xdg_data_home() {
    let root = tempfile::tempdir().expect("tempdir");

    let xdg_data = root.path().join("xdg-data");
    fs::create_dir_all(&xdg_data).expect("create xdg-data");

    let source = root.path().join("plugin.wasm");
    fs::write(&source, b"irrelevant").expect("write source");

    let mut cmd = locald();
    cmd.current_dir(root.path());
    cmd.env("XDG_DATA_HOME", &xdg_data);
    cmd.args(["plugin", "install"]).arg(source.as_os_str());

    cmd.assert().success();

    // directories::ProjectDirs typically appends the app name (locald) under XDG_DATA_HOME.
    let candidate1 = xdg_data.join("locald").join("plugins").join("plugin.wasm");
    let candidate2 = xdg_data.join("plugins").join("plugin.wasm");

    assert!(
        candidate1.exists() || candidate2.exists(),
        "expected plugin to be installed under XDG_DATA_HOME (checked {} and {})",
        candidate1.display(),
        candidate2.display()
    );
}
