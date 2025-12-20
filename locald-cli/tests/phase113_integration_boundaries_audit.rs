//! Phase 113: Integration Boundaries Audit (RED tests).

use assert_cmd::prelude::*;
use std::process::Command;
use predicates::prelude::*;
use predicates::str::contains;

#[test]
fn phase113_doctor_mentions_docker_when_unavailable() {
    // This is intentionally RED to start: we want to codify the desired UX for
    // optional integrations before implementing any behavior changes.
    //
    // Goal: `locald doctor` should clearly surface Docker availability and any
    // limitations when Docker is not available.
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    cmd.arg("doctor");

    cmd.assert().success().stdout(contains("Docker").or(contains("docker")));
}
