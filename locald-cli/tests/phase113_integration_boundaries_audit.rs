//! Phase 113: Integration Boundaries Audit.

use std::process::Command;

#[test]
fn phase113_doctor_mentions_docker_when_unavailable() {
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

#[test]
fn phase113_doctor_explains_docker_unavailability_impact() {
    // Goal: When Docker isn't available, `locald doctor` should explain what
    // that means for locald (i.e. Docker-based services won't work).
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Docker-based services"),
        "Expected doctor output to explain Docker impact, but got:\n{stdout}"
    );
}

#[test]
fn phase113_doctor_shows_docker_socket_path() {
    // Goal: `locald doctor` should show the exact Docker socket path being used.
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("/var/run/docker.sock"),
        "Expected doctor output to mention the Docker socket path, but got:\n{stdout}"
    );
}
