//! Tests for `locald plugin` subcommands.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

fn locald() -> Command {
    let bin = assert_cmd::cargo::cargo_bin!("locald");
    Command::new(bin)
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
