//! Phase 113: Integration Boundaries Audit.

use std::process::Command;

#[test]
fn phase113_doctor_mentions_docker_when_unavailable() {
    // This is intentionally RED to start: we want to codify the desired UX for
    // optional integrations before implementing any behavior changes.
    //
    // Goal: `locald doctor` should clearly surface Docker availability and any
    // limitations when Docker is not available.
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    // `doctor` may return a non-zero exit code when critical host setup is missing.
    // For integration boundaries, we still want Docker availability to be surfaced.
    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Docker") || stdout.contains("docker"),
        "Expected doctor output to mention Docker, but got:\n{stdout}"
    );
}
