//! Phase 113: Integration Boundaries Audit.

use std::process::Command;

#[test]
#[cfg(target_os = "linux")]
fn phase113_doctor_mentions_buildpacks_cnb_optional_integration() {
    // Goal: `locald doctor` should surface that Buildpacks/CNB support exists and
    // clearly communicate its dependency on the privileged shim (not Docker).
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Buildpacks") || stdout.contains("CNB"),
        "Expected doctor output to mention Buildpacks/CNB, but got:\n{stdout}"
    );
    assert!(
        stdout.contains("locald-shim")
            || stdout.contains("admin setup")
            || stdout.contains("privileged"),
        "Expected doctor output to explain the privileged shim dependency for Buildpacks/CNB, but got:\n{stdout}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn phase113_doctor_mentions_virtualization_kvm_optional_integration() {
    // Goal: `locald doctor` should surface virtualization availability, since some
    // workflows depend on KVM (/dev/kvm) on Linux.
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Virtualization") || stdout.contains("KVM"),
        "Expected doctor output to mention virtualization/KVM, but got:\n{stdout}"
    );
    assert!(
        stdout.contains("/dev/kvm") || stdout.to_lowercase().contains("kvm"),
        "Expected doctor output to reference /dev/kvm or KVM availability, but got:\n{stdout}"
    );
}

#[test]
fn phase113_doctor_consolidates_privileged_repair_into_admin_setup() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.contains("Suggested next steps:") {
        return;
    }

    let setup_command = if cfg!(target_os = "macos") {
        "sudo locald admin setup"
    } else {
        "locald admin setup"
    };
    assert!(
        stdout.contains(setup_command),
        "Expected doctor output to recommend {setup_command}, but got:\n{stdout}"
    );
    assert!(
        !output.status.success(),
        "Expected doctor to exit nonzero while privileged repair is required, but got:\n{stdout}"
    );

    assert!(
        !stdout.contains("\n  Fix:\n"),
        "Expected doctor output to avoid per-problem Fix blocks (fixes should be consolidated), but got:\n{stdout}"
    );
}
