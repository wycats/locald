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

#[test]
fn phase113_doctor_respects_docker_host_env() {
    // Goal: If DOCKER_HOST is set to a unix socket, `locald doctor` should
    // report/check that socket (not only /var/run/docker.sock).
    let docker_host = "unix:///tmp/locald-nonexistent-docker.sock";

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.env("DOCKER_HOST", docker_host);
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains(docker_host),
        "Expected doctor output to mention DOCKER_HOST value, but got:\n{stdout}"
    );
    assert!(
        stdout.contains("socket not found"),
        "Expected doctor output to reflect missing DOCKER_HOST socket, but got:\n{stdout}"
    );
}

#[test]
fn phase113_doctor_explains_unsupported_docker_host_schemes() {
    // Goal: If DOCKER_HOST is set to a non-unix scheme (e.g. tcp://), `locald doctor`
    // should clearly explain that this check only supports unix:// sockets, rather than
    // silently probing /var/run/docker.sock.
    let docker_host = "tcp://127.0.0.1:2375";
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");
    cmd.env("DOCKER_HOST", docker_host);

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains(docker_host),
        "Expected doctor output to mention DOCKER_HOST value, but got:\n{stdout}"
    );
    assert!(
        stdout.to_lowercase().contains("unsupported") || stdout.contains("only unix://"),
        "Expected doctor output to explain unsupported DOCKER_HOST scheme, but got:\n{stdout}"
    );
    assert!(
        !stdout.contains("/var/run/docker.sock"),
        "Expected doctor output not to fall back to /var/run/docker.sock when DOCKER_HOST is non-unix, but got:\n{stdout}"
    );
}

#[test]
fn phase113_doctor_mentions_buildpacks_cnb_optional_integration() {
    // Goal: `locald doctor` should surface that Buildpacks/CNB support exists and
    // clearly communicate its dependency on Docker.
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    let output = cmd.output().expect("failed to run locald doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Buildpacks") || stdout.contains("CNB"),
        "Expected doctor output to mention Buildpacks/CNB, but got:\n{stdout}"
    );
    assert!(
        stdout.contains("requires Docker") || stdout.contains("Docker available"),
        "Expected doctor output to explain the Docker dependency for Buildpacks/CNB, but got:\n{stdout}"
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
