#![allow(missing_docs)]

use std::process::Command;

#[test]
fn doctor_json_outputs_valid_json() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("locald"))
        .args(["doctor", "--json"])
        .env("LOCALD_SANDBOX_ACTIVE", "1")
        .output()
        .expect("run locald doctor --json");

    assert!(
        output.status.success() || output.status.code() == Some(1),
        "unexpected exit status: {:?}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "expected empty stderr, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("doctor --json should write a valid JSON report to stdout");

    assert!(report.get("strategy").is_some(), "missing strategy");
    assert!(report.get("mode").is_some(), "missing mode");
    assert!(report.get("problems").is_some(), "missing problems");
    assert!(report.get("fixes").is_some(), "missing fixes");

    #[cfg(target_os = "macos")]
    {
        let problems = report["problems"]
            .as_array()
            .expect("macOS doctor problems are an array");
        let ids = problems
            .iter()
            .filter_map(|problem| problem["id"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(ids.contains("macos.console_user"));
        assert!(ids.contains("macos.ca.trust"));
        assert!(ids.contains("macos.agent.launch_agent"));
        assert!(ids.contains("macos.helper.authority"));
        assert!(ids.contains("macos.helper.probe"));
        assert!(problems.iter().all(|problem| problem["status"] == "skip"));
        assert_eq!(report["fixes"], serde_json::json!([]));
    }
}
