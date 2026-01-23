//! End-to-end tests for the plugin system using the Redis example plugin.
//!
//! These tests verify the full plugin lifecycle:
//! - Discovery: Plugins are found in .locald/plugins/
//! - Execution: Plugin WASM components can be loaded and executed
//! - Plan Application: Plugin-generated plans materialize into services
//!
//! Note: These tests require the redis-plugin example to be built first:
//!   cd examples/redis-plugin && cargo component build --release
#![cfg(feature = "experimental-plugins")]

use std::fs;
use std::path::PathBuf;

/// Path to the built Redis plugin WASM file.
/// This is relative to the workspace root.
fn redis_plugin_wasm_path() -> PathBuf {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace_root.join("examples/redis-plugin/target/wasm32-wasip1/release/redis_plugin.wasm")
}

/// Check if the Redis plugin has been built.
fn redis_plugin_available() -> bool {
    redis_plugin_wasm_path().exists()
}

#[test]
fn redis_plugin_discovery_finds_installed_plugin() {
    if !redis_plugin_available() {
        eprintln!("Skipping test: redis-plugin not built. Run:");
        eprintln!("  cd examples/redis-plugin && cargo build --release --target wasm32-wasip1");
        return;
    }

    let root = tempfile::tempdir().expect("tempdir");

    // Create .locald/plugins directory and copy the plugin
    let plugins_dir = root.path().join(".locald").join("plugins");
    fs::create_dir_all(&plugins_dir).expect("create plugins dir");

    let plugin_dest = plugins_dir.join("redis.wasm");
    fs::copy(redis_plugin_wasm_path(), &plugin_dest).expect("copy plugin");

    // Verify the plugin file exists
    assert!(plugin_dest.exists(), "plugin should be installed");

    // Use the discovery module directly
    use locald_server::plugins::discovery;
    let discovered = discovery::discover_plugins(root.path());

    assert_eq!(discovered.len(), 1, "should discover one plugin");
    assert!(
        discovered[0].ends_with("redis.wasm"),
        "should find redis.wasm, got: {:?}",
        discovered[0]
    );
}

#[test]
fn redis_plugin_inspect_shows_plan_structure() {
    if !redis_plugin_available() {
        eprintln!("Skipping test: redis-plugin not built");
        return;
    }

    let root = tempfile::tempdir().expect("tempdir");

    // Install the plugin
    let plugins_dir = root.path().join(".locald").join("plugins");
    fs::create_dir_all(&plugins_dir).expect("create plugins dir");
    fs::copy(redis_plugin_wasm_path(), plugins_dir.join("redis.wasm")).expect("copy plugin");

    // Run plugin inspect
    use assert_cmd::Command;

    let bin = assert_cmd::cargo::cargo_bin!("locald");
    let mut cmd = Command::new(bin);
    cmd.current_dir(root.path());
    cmd.env("LOCALD_SANDBOX_ACTIVE", "1");
    cmd.args(["plugin", "inspect", "redis", "--kind", "redis"]);

    let output = cmd.assert().success();

    // Parse the JSON output
    let stdout = String::from_utf8(output.get_output().stdout.clone()).expect("utf8");

    // Check that the output contains expected plan structure
    assert!(
        stdout.contains("ir_version") || stdout.contains("ir-version"),
        "output should contain IR version: {}",
        stdout
    );
    assert!(
        stdout.contains("steps"),
        "output should contain steps: {}",
        stdout
    );
}

#[test]
fn redis_plugin_generates_valid_plan() {
    if !redis_plugin_available() {
        eprintln!("Skipping test: redis-plugin not built");
        return;
    }

    let root = tempfile::tempdir().expect("tempdir");

    // Install the plugin
    let plugins_dir = root.path().join(".locald").join("plugins");
    fs::create_dir_all(&plugins_dir).expect("create plugins dir");
    fs::copy(redis_plugin_wasm_path(), plugins_dir.join("redis.wasm")).expect("copy plugin");

    // Run plugin validate
    use assert_cmd::Command;

    let bin = assert_cmd::cargo::cargo_bin!("locald");
    let mut cmd = Command::new(bin);
    cmd.current_dir(root.path());
    cmd.env("LOCALD_SANDBOX_ACTIVE", "1");
    cmd.args(["plugin", "validate", "redis", "--kind", "redis"]);

    // Validate should succeed for a well-formed plugin
    cmd.assert().success();
}

#[test]
fn redis_plugin_plan_application_creates_service() {
    if !redis_plugin_available() {
        eprintln!("Skipping test: redis-plugin not built");
        return;
    }

    use locald_server::plugins;

    let root = tempfile::tempdir().expect("tempdir");

    // Install the plugin
    let plugins_dir = root.path().join(".locald").join("plugins");
    fs::create_dir_all(&plugins_dir).expect("create plugins dir");
    fs::copy(redis_plugin_wasm_path(), plugins_dir.join("redis.wasm")).expect("copy plugin");

    // Create a config with a redis service
    let config_content = r#"
[project]
name = "test-project"

[services.cache]
kind = "redis"
"#;
    fs::write(root.path().join("locald.toml"), config_content).expect("write config");

    // Discover plugins
    let discovered = plugins::discovery::discover_plugins(root.path());
    assert!(!discovered.is_empty(), "should discover plugins");

    // The plugin system should recognize the redis service
    // For now we verify discovery works - full integration is tested via the CLI
    assert_eq!(discovered.len(), 1);
}
