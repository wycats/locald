//! Tests for macOS-specific error messages and platform detection.
//!
//! These tests verify that the platform-specific code paths work correctly.
//! They can run on any platform but test different behaviors based on the target OS.

use std::process::Command;

/// Test that `locald admin setup` gives appropriate output based on platform.
#[test]
fn test_admin_setup_platform_behavior() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .args(["admin", "setup"])
        .output()
        .expect("failed to run locald admin setup");

    #[allow(unused_variables)]
    let stderr = String::from_utf8_lossy(&output.stderr);
    #[allow(unused_variables)]
    let stdout = String::from_utf8_lossy(&output.stdout);
    #[allow(unused_variables)]
    let combined = format!("{stdout}{stderr}");

    #[cfg(target_os = "linux")]
    {
        // On Linux, it should either succeed (if root) or ask for sudo
        // It should NOT say "not available on macOS"
        assert!(
            !combined.contains("not available on macOS"),
            "Linux build should not mention macOS unavailability"
        );
    }

    #[cfg(target_os = "macos")]
    {
        // On macOS, it should fail with helpful message
        assert!(!output.status.success(), "admin setup should fail on macOS");
        assert!(
            combined.contains("Linux-only") || combined.contains("not available"),
            "Should explain Linux-only nature. Got: {combined}"
        );
        assert!(
            combined.contains("locald up") || combined.contains("just run"),
            "Should suggest locald up. Got: {combined}"
        );
    }
}

/// Test that `locald admin sync-hosts` gives appropriate output based on platform.
#[test]
fn test_admin_sync_hosts_platform_behavior() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .args(["admin", "sync-hosts"])
        .output()
        .expect("failed to run locald admin sync-hosts");

    #[allow(unused_variables)]
    let stderr = String::from_utf8_lossy(&output.stderr);
    #[allow(unused_variables)]
    let stdout = String::from_utf8_lossy(&output.stdout);
    #[allow(unused_variables)]
    let combined = format!("{stdout}{stderr}");

    #[cfg(target_os = "macos")]
    {
        // On macOS, it should fail with helpful message
        assert!(!output.status.success(), "sync-hosts should fail on macOS");
        assert!(
            combined.contains("manual setup") || combined.contains("Linux-only"),
            "Should explain manual setup needed. Got: {combined}"
        );
        assert!(
            combined.contains("dnsmasq") || combined.contains("/etc/hosts"),
            "Should suggest alternatives. Got: {combined}"
        );
    }
}

/// Test that `locald doctor` runs on all platforms.
#[test]
fn test_doctor_runs_on_all_platforms() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .arg("doctor")
        .output()
        .expect("failed to run locald doctor");

    // Doctor should always succeed (exit 0) or return 1 for issues
    // It should NOT crash
    assert!(output.status.code().is_some(), "doctor should exit cleanly");

    #[allow(unused_variables)]
    let stdout = String::from_utf8_lossy(&output.stdout);

    #[cfg(target_os = "macos")]
    {
        // On macOS, doctor should mention macOS-specific info
        assert!(
            stdout.contains("macOS") || stdout.contains("exec services"),
            "Doctor should mention macOS compatibility. Got: {stdout}"
        );
    }
}

/// Test that `locald doctor --json` produces valid JSON on all platforms.
#[test]
fn test_doctor_json_valid() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .args(["doctor", "--json"])
        .output()
        .expect("failed to run locald doctor --json");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should be valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(
        parsed.is_ok(),
        "doctor --json should produce valid JSON. Got: {stdout}"
    );

    let json = parsed.unwrap();
    assert!(
        json.get("problems").is_some(),
        "JSON should have 'problems' key"
    );
    assert!(
        json.get("strategy").is_some(),
        "JSON should have 'strategy' key"
    );
}

/// Test that hints module provides platform-appropriate advice.
#[test]
fn test_hints_platform_aware() {
    // This test just verifies the hints compile correctly
    // The actual hint functions are tested implicitly through CLI behavior

    let output = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .arg("--help")
        .output()
        .expect("failed to run locald --help");

    assert!(output.status.success(), "--help should succeed");
}
