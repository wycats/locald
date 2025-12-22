//! Phase 113: Integration Boundaries Audit.

use std::process::Command;

#[test]
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
fn phase113_doctor_recommends_admin_setup_without_sudo_and_suggests_up_next() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.contains("Suggested next steps:") {
        return;
    }

    assert!(
        stdout.contains("locald admin setup"),
        "Expected doctor output to recommend locald admin setup, but got:\n{stdout}"
    );
    assert!(
        !stdout.contains("sudo locald"),
        "Expected doctor output to avoid sudo locald (it can restrict PATH), but got:\n{stdout}"
    );

    assert!(
        !stdout.contains("\n  Fix:\n"),
        "Expected doctor output to avoid per-problem Fix blocks (fixes should be consolidated), but got:\n{stdout}"
    );
    assert!(
        stdout.contains("- Next: run locald up."),
        "Expected doctor output to include Next: run locald up. as a structured list item, but got:\n{stdout}"
    );
}
