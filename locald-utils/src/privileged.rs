//! Privileged capability acquisition and host readiness reporting.
//!
//! This module provides a shared, structured report used by `locald doctor` and by
//! privileged call sites to prevent readiness drift.

#![allow(missing_docs)]

use crate::cgroup::{CgroupRootStrategy, cgroup_fs_root, is_root_ready};
use crate::shim;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CgroupStrategyKind {
    Systemd,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyReport {
    pub cgroup_root: CgroupStrategyKind,
    pub why: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupMode {
    Enabled,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixKey {
    /// Canonical remediation when the shim/cgroup root needs installation or repair.
    RunAdminSetup,

    /// Host policy blocks setuid helper execution (nosuid, SELinux/AppArmor, container constraints).
    HostPolicyBlocksPrivilegedHelper,

    /// Environment cannot support required privileged operations (e.g. unprivileged container).
    UnsupportedEnvironment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixAdvice {
    pub key: FixKey,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub id: String,
    pub severity: Severity,
    pub status: Status,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remediation: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub strategy: StrategyReport,
    pub mode: CleanupMode,
    pub problems: Vec<Problem>,
    pub fixes: Vec<FixAdvice>,
}

impl DoctorReport {
    #[must_use]
    pub fn has_critical_failures(&self) -> bool {
        self.problems
            .iter()
            .any(|p| p.status == Status::Fail && p.severity == Severity::Critical)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AcquireConfig<'a> {
    pub verbose: bool,
    pub expected_shim_version: Option<&'a str>,
    pub expected_shim_bytes: Option<&'a [u8]>,
}

impl AcquireConfig<'_> {
    #[must_use]
    pub const fn daemon_default() -> Self {
        Self {
            verbose: false,
            expected_shim_version: None,
            expected_shim_bytes: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("host is not ready for privileged operations")]
pub struct NotReady {
    pub report: DoctorReport,
}

#[derive(Debug)]
pub struct Privileged {
    shim: ShimHandle,
    report: DoctorReport,
}

#[derive(Debug)]
struct ShimHandle {
    path: PathBuf,
}

impl Privileged {
    /// Acquire a capability for privileged effects.
    ///
    /// # Errors
    ///
    /// Returns `NotReady` when the host is not ready for privileged operations.
    pub fn acquire(config: AcquireConfig<'_>) -> std::result::Result<Self, NotReady> {
        let report = collect_report(config).unwrap_or_else(|e| DoctorReport {
            strategy: StrategyReport {
                cgroup_root: CgroupStrategyKind::Direct,
                why: format!("failed to collect report: {e}"),
            },
            mode: CleanupMode::Degraded,
            problems: vec![Problem {
                id: "doctor.collect".to_string(),
                severity: Severity::Critical,
                status: Status::Fail,
                summary: "failed to collect host readiness report".to_string(),
                details: Some(e.to_string()),
                remediation: vec![],
                evidence: vec![],
                fix: None,
            }],
            fixes: vec![],
        });

        if report.has_critical_failures() {
            return Err(NotReady { report });
        }

        // If we got here, we must have a privileged shim candidate.
        // Pick the same shim path used during report collection.
        let shim_path = report
            .problems
            .iter()
            .find_map(|p| {
                p.evidence
                    .iter()
                    .find(|e| e.key == "shim.path")
                    .map(|e| PathBuf::from(&e.value))
            })
            .or_else(|| shim::find().ok().flatten())
            .unwrap_or_else(|| PathBuf::from("locald-shim"));

        Ok(Self {
            shim: ShimHandle { path: shim_path },
            report,
        })
    }

    #[must_use]
    pub const fn report(&self) -> &DoctorReport {
        &self.report
    }

    pub fn tokio_command(&self) -> tokio::process::Command {
        shim::tokio_command(&self.shim.path)
    }

    pub fn command(&self) -> std::process::Command {
        #[allow(clippy::disallowed_methods)]
        let mut cmd = std::process::Command::new(&self.shim.path);
        cmd.env_remove("LD_LIBRARY_PATH");
        cmd
    }
}

/// Collect a host readiness report.
///
/// # Errors
///
/// Returns an error if required host probes fail unexpectedly.
pub fn collect_report(config: AcquireConfig<'_>) -> Result<DoctorReport> {
    let mut problems: Vec<Problem> = Vec::new();

    let (strategy, strategy_why) = detect_strategy();
    let strategy_report = StrategyReport {
        cgroup_root: match strategy {
            CgroupRootStrategy::Systemd => CgroupStrategyKind::Systemd,
            CgroupRootStrategy::Direct => CgroupStrategyKind::Direct,
        },
        why: strategy_why,
    };

    // Cgroup readiness
    let cgroup_root = cgroup_fs_root();
    let cgroup_v2_available = cgroup_root.join("cgroup.controllers").exists();

    if !cgroup_v2_available {
        problems.push(Problem {
            id: "cgroup.v2".to_string(),
            severity: Severity::Critical,
            status: Status::Fail,
            summary: "cgroup v2 does not appear to be available".to_string(),
            details: Some("missing /sys/fs/cgroup/cgroup.controllers".to_string()),
            remediation: vec!["sudo locald admin setup".to_string()],
            evidence: if config.verbose {
                vec![EvidenceItem {
                    key: "cgroup.root".to_string(),
                    value: cgroup_root.display().to_string(),
                }]
            } else {
                vec![]
            },
            fix: Some(FixKey::RunAdminSetup),
        });
    }

    let root_ready = is_root_ready(strategy);
    if cgroup_v2_available && !root_ready {
        let expected = match strategy {
            CgroupRootStrategy::Systemd => "/sys/fs/cgroup/locald.slice",
            CgroupRootStrategy::Direct => "/sys/fs/cgroup/locald",
        };

        problems.push(Problem {
            id: "cgroup.root_ready".to_string(),
            severity: Severity::Critical,
            status: Status::Fail,
            summary: "locald cgroup root is not established".to_string(),
            details: Some(format!("missing expected root: {expected}")),
            remediation: vec!["sudo locald admin setup".to_string()],
            evidence: if config.verbose {
                vec![EvidenceItem {
                    key: "cgroup.expected_root".to_string(),
                    value: expected.to_string(),
                }]
            } else {
                vec![]
            },
            fix: Some(FixKey::RunAdminSetup),
        });
    }

    let mode = if cgroup_v2_available && root_ready {
        CleanupMode::Enabled
    } else {
        CleanupMode::Degraded
    };

    // Shim readiness
    let shim_path = shim::find()?;
    match shim_path {
        None => {
            problems.push(Problem {
                id: "shim.present".to_string(),
                severity: Severity::Critical,
                status: Status::Fail,
                summary: "privileged locald-shim not found".to_string(),
                details: Some("expected locald-shim next to the locald executable".to_string()),
                remediation: vec!["sudo locald admin setup".to_string()],
                evidence: vec![],
                fix: Some(FixKey::RunAdminSetup),
            });
        }
        Some(path) => {
            let privileged = shim::is_privileged(&path).unwrap_or(false);
            let mut evidence = vec![EvidenceItem {
                key: "shim.path".to_string(),
                value: path.display().to_string(),
            }];

            if config.verbose
                && let Ok(meta) = std::fs::metadata(&path)
            {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    evidence.push(EvidenceItem {
                        key: "shim.uid".to_string(),
                        value: meta.uid().to_string(),
                    });
                    evidence.push(EvidenceItem {
                        key: "shim.mode".to_string(),
                        value: format!("{:o}", meta.mode()),
                    });
                }

                #[cfg(not(unix))]
                {
                    let _ = meta;
                }
            }

            if privileged {
                // Integrity/version checks are optional based on config.
                if let Some(expected_bytes) = config.expected_shim_bytes {
                    match shim::verify_integrity(&path, expected_bytes) {
                        Ok(true) => {
                            // pass
                        }
                        Ok(false) => {
                            problems.push(Problem {
                                id: "shim.integrity".to_string(),
                                severity: Severity::Critical,
                                status: Status::Fail,
                                summary: "locald-shim does not match this locald build".to_string(),
                                details: Some(
                                    "installed shim differs from embedded shim".to_string(),
                                ),
                                remediation: vec!["sudo locald admin setup".to_string()],
                                evidence: evidence.clone(),
                                fix: Some(FixKey::RunAdminSetup),
                            });
                        }
                        Err(e) => {
                            problems.push(Problem {
                                id: "shim.integrity".to_string(),
                                severity: Severity::Critical,
                                status: Status::Fail,
                                summary: "failed to verify locald-shim".to_string(),
                                details: Some(e.to_string()),
                                remediation: vec!["sudo locald admin setup".to_string()],
                                evidence: evidence.clone(),
                                fix: Some(FixKey::RunAdminSetup),
                            });
                        }
                    }
                } else if let Some(expected_version) = config.expected_shim_version {
                    let detected = shim_version(&path).ok();
                    if config.verbose {
                        if let Some(v) = &detected {
                            evidence.push(EvidenceItem {
                                key: "shim.version".to_string(),
                                value: v.clone(),
                            });
                        }
                        evidence.push(EvidenceItem {
                            key: "shim.expected_version".to_string(),
                            value: expected_version.to_string(),
                        });
                    }

                    if detected.as_deref() != Some(expected_version) {
                        problems.push(Problem {
                            id: "shim.version".to_string(),
                            severity: Severity::Critical,
                            status: Status::Fail,
                            summary: "locald-shim version does not match this locald build"
                                .to_string(),
                            details: Some(format!(
                                "expected {expected_version}, got {}",
                                detected.as_deref().unwrap_or("<unknown>")
                            )),
                            remediation: vec!["sudo locald admin setup".to_string()],
                            evidence: evidence.clone(),
                            fix: Some(FixKey::RunAdminSetup),
                        });
                    }
                }

                // Usability smoke test (non-destructive).
                match shim_self_check(&path) {
                    Ok(()) => {
                        // pass
                    }
                    Err(e) => {
                        let mut ev = evidence;
                        if config.verbose
                            && let Ok(uid) = std::env::var("SUDO_UID")
                        {
                            ev.push(EvidenceItem {
                                key: "sudo.uid".to_string(),
                                value: uid,
                            });
                        }

                        problems.push(Problem {
                            id: "shim.usability".to_string(),
                            severity: Severity::Critical,
                            status: Status::Fail,
                            summary: "locald-shim failed its usability self-check".to_string(),
                            details: Some(e.to_string()),
                            remediation: vec![],
                            evidence: ev,
                            fix: Some(FixKey::HostPolicyBlocksPrivilegedHelper),
                        });
                    }
                }
            } else {
                problems.push(Problem {
                    id: "shim.permissions".to_string(),
                    severity: Severity::Critical,
                    status: Status::Fail,
                    summary: "locald-shim is present but not configured for privileged use"
                        .to_string(),
                    details: Some("expected root ownership + setuid bit".to_string()),
                    remediation: vec!["sudo locald admin setup".to_string()],
                    evidence: evidence.clone(),
                    fix: Some(FixKey::RunAdminSetup),
                });
            }
        }
    }

    let fixes = consolidate_fixes(&problems);

    Ok(DoctorReport {
        strategy: strategy_report,
        mode,
        problems,
        fixes,
    })
}

fn consolidate_fixes(problems: &[Problem]) -> Vec<FixAdvice> {
    use std::collections::BTreeSet;

    let mut keys = BTreeSet::new();
    for p in problems {
        if p.status == Status::Fail
            && let Some(k) = p.fix
        {
            keys.insert(k);
        }
    }

    // Priority ordering: canonical fix first.
    let ordered = [
        FixKey::RunAdminSetup,
        FixKey::HostPolicyBlocksPrivilegedHelper,
        FixKey::UnsupportedEnvironment,
    ];

    let mut out = Vec::new();
    for key in ordered {
        if !keys.contains(&key) {
            continue;
        }

        match key {
            FixKey::RunAdminSetup => out.push(FixAdvice {
                key,
                summary: "Install/repair the privileged helper and host setup".to_string(),
                commands: vec!["sudo locald admin setup".to_string()],
            }),
            FixKey::HostPolicyBlocksPrivilegedHelper => out.push(FixAdvice {
                key,
                summary: "Host policy prevents the setuid helper from working".to_string(),
                commands: vec![],
            }),
            FixKey::UnsupportedEnvironment => out.push(FixAdvice {
                key,
                summary: "This environment cannot support required privileged operations"
                    .to_string(),
                commands: vec![],
            }),
        }
    }

    out
}

fn detect_strategy() -> (CgroupRootStrategy, String) {
    let mut comm = String::new();
    if let Ok(mut file) = std::fs::File::open("/proc/1/comm") {
        let _read_result = file.read_to_string(&mut comm);
    }
    let comm = comm.trim().to_string();

    if comm == "systemd" {
        (CgroupRootStrategy::Systemd, "PID 1 is systemd".to_string())
    } else {
        (
            CgroupRootStrategy::Direct,
            format!("PID 1 is not systemd (comm={comm})"),
        )
    }
}

fn shim_version(shim_path: &Path) -> Result<String> {
    #[allow(clippy::disallowed_methods)]
    let output = std::process::Command::new(shim_path)
        .arg("--shim-version")
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .with_context(|| format!("failed to execute {} --shim-version", shim_path.display()))?;

    if !output.status.success() {
        anyhow::bail!("shim --shim-version failed: {}", output.status);
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(version)
}

fn shim_self_check(shim_path: &Path) -> Result<()> {
    #[allow(clippy::disallowed_methods)]
    let status = std::process::Command::new(shim_path)
        .arg("admin")
        .arg("self-check")
        .env_remove("LD_LIBRARY_PATH")
        .status()
        .with_context(|| format!("failed to execute {} admin self-check", shim_path.display()))?;

    if !status.success() {
        anyhow::bail!("shim self-check failed with status: {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn failing_problem(id: &str, severity: Severity, fix: Option<FixKey>) -> Problem {
        Problem {
            id: id.to_string(),
            severity,
            status: Status::Fail,
            summary: "x".to_string(),
            details: None,
            remediation: vec![],
            evidence: vec![],
            fix,
        }
    }

    #[test]
    fn report_detects_critical_failures() {
        let report = DoctorReport {
            strategy: StrategyReport {
                cgroup_root: CgroupStrategyKind::Direct,
                why: "x".to_string(),
            },
            mode: CleanupMode::Degraded,
            problems: vec![failing_problem(
                "shim.present",
                Severity::Critical,
                Some(FixKey::RunAdminSetup),
            )],
            fixes: vec![],
        };

        assert!(report.has_critical_failures());
    }

    #[test]
    fn consolidate_fixes_dedupes_and_orders() {
        let problems = vec![
            failing_problem(
                "shim.permissions",
                Severity::Critical,
                Some(FixKey::RunAdminSetup),
            ),
            failing_problem(
                "cgroup.root_ready",
                Severity::Critical,
                Some(FixKey::RunAdminSetup),
            ),
            failing_problem(
                "shim.usability",
                Severity::Critical,
                Some(FixKey::HostPolicyBlocksPrivilegedHelper),
            ),
        ];

        let fixes = consolidate_fixes(&problems);
        assert_eq!(fixes.len(), 2);
        assert_eq!(fixes[0].key, FixKey::RunAdminSetup);
        assert_eq!(fixes[1].key, FixKey::HostPolicyBlocksPrivilegedHelper);
    }

    #[test]
    fn json_roundtrip_preserves_fields() {
        let report = DoctorReport {
            strategy: StrategyReport {
                cgroup_root: CgroupStrategyKind::Direct,
                why: "PID 1 is not systemd".to_string(),
            },
            mode: CleanupMode::Degraded,
            problems: vec![Problem {
                id: "shim.present".to_string(),
                severity: Severity::Critical,
                status: Status::Fail,
                summary: "privileged locald-shim not found".to_string(),
                details: None,
                remediation: vec!["sudo locald admin setup".to_string()],
                evidence: vec![],
                fix: Some(FixKey::RunAdminSetup),
            }],
            fixes: vec![FixAdvice {
                key: FixKey::RunAdminSetup,
                summary: "Install/repair the privileged helper and host setup".to_string(),
                commands: vec!["sudo locald admin setup".to_string()],
            }],
        };

        let json = serde_json::to_value(&report);
        assert!(json.is_ok());
        let Ok(json) = json else { return };
        // Minimal schema assertions (avoid brittle snapshots).
        assert!(json.get("strategy").is_some());
        assert!(json.get("mode").is_some());
        assert!(json.get("problems").is_some());
        assert!(json.get("fixes").is_some());

        let roundtrip = serde_json::from_value::<DoctorReport>(json);
        assert!(roundtrip.is_ok());
        let Ok(roundtrip) = roundtrip else { return };
        assert_eq!(roundtrip.strategy.cgroup_root, report.strategy.cgroup_root);
        assert_eq!(roundtrip.mode, report.mode);
        assert_eq!(roundtrip.problems.len(), 1);
    }

    fn arb_fix_key() -> impl Strategy<Value = FixKey> {
        prop_oneof![
            Just(FixKey::RunAdminSetup),
            Just(FixKey::HostPolicyBlocksPrivilegedHelper),
            Just(FixKey::UnsupportedEnvironment),
        ]
    }

    fn arb_severity() -> impl Strategy<Value = Severity> {
        prop_oneof![
            Just(Severity::Critical),
            Just(Severity::Warning),
            Just(Severity::Info)
        ]
    }

    fn arb_status() -> impl Strategy<Value = Status> {
        prop_oneof![Just(Status::Pass), Just(Status::Fail), Just(Status::Skip)]
    }

    fn arb_small_string() -> impl Strategy<Value = String> {
        // Keep inputs small and printable to reduce shrink noise.
        proptest::collection::vec(proptest::char::range(' ', '~'), 0..40)
            .prop_map(|chars| chars.into_iter().collect())
    }

    fn arb_evidence_item() -> impl Strategy<Value = EvidenceItem> {
        (arb_small_string(), arb_small_string())
            .prop_map(|(key, value)| EvidenceItem { key, value })
    }

    fn arb_problem() -> impl Strategy<Value = Problem> {
        (
            arb_small_string(),
            arb_severity(),
            arb_status(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            proptest::collection::vec(arb_small_string(), 0..5),
            proptest::collection::vec(arb_evidence_item(), 0..5),
            proptest::option::of(arb_fix_key()),
        )
            .prop_map(
                |(id, severity, status, summary, details, remediation, evidence, fix)| Problem {
                    id,
                    severity,
                    status,
                    summary,
                    details,
                    remediation,
                    evidence,
                    fix,
                },
            )
    }

    fn arb_strategy_report() -> impl Strategy<Value = StrategyReport> {
        (
            prop_oneof![
                Just(CgroupStrategyKind::Systemd),
                Just(CgroupStrategyKind::Direct)
            ],
            arb_small_string(),
        )
            .prop_map(|(cgroup_root, why)| StrategyReport { cgroup_root, why })
    }

    fn arb_cleanup_mode() -> impl Strategy<Value = CleanupMode> {
        prop_oneof![Just(CleanupMode::Enabled), Just(CleanupMode::Degraded)]
    }

    fn arb_fix_advice() -> impl Strategy<Value = FixAdvice> {
        (
            arb_fix_key(),
            arb_small_string(),
            proptest::collection::vec(arb_small_string(), 0..4),
        )
            .prop_map(|(key, summary, commands)| FixAdvice {
                key,
                summary,
                commands,
            })
    }

    fn arb_report() -> impl Strategy<Value = DoctorReport> {
        (
            arb_strategy_report(),
            arb_cleanup_mode(),
            proptest::collection::vec(arb_problem(), 0..80),
            proptest::collection::vec(arb_fix_advice(), 0..10),
        )
            .prop_map(|(strategy, mode, problems, fixes)| DoctorReport {
                strategy,
                mode,
                problems,
                fixes,
            })
    }

    proptest! {
        #[test]
        fn consolidate_fixes_is_deduped_and_prioritized(keys in proptest::collection::vec(arb_fix_key(), 0..30)) {
            let problems: Vec<Problem> = keys.into_iter().enumerate().map(|(i, k)| Problem {
                id: format!("p{i}"),
                severity: Severity::Critical,
                status: Status::Fail,
                summary: "x".to_string(),
                details: None,
                remediation: vec![],
                evidence: vec![],
                fix: Some(k),
            }).collect();

            let fixes = consolidate_fixes(&problems);

            // No duplicates.
            let mut seen = std::collections::BTreeSet::new();
            for f in &fixes {
                prop_assert!(seen.insert(f.key));
            }

            // Sorted by our explicit priority ordering.
            let priority = |k: FixKey| match k {
                FixKey::RunAdminSetup => 0u8,
                FixKey::HostPolicyBlocksPrivilegedHelper => 1u8,
                FixKey::UnsupportedEnvironment => 2u8,
            };
            for w in fixes.windows(2) {
                prop_assert!(priority(w[0].key) <= priority(w[1].key));
            }
        }

        #[test]
        fn has_critical_failures_matches_predicate(
            triples in proptest::collection::vec((arb_severity(), arb_status(), proptest::option::of(arb_fix_key())), 0..200)
        ) {
            let problems: Vec<Problem> = triples
                .into_iter()
                .enumerate()
                .map(|(i, (severity, status, fix))| Problem {
                    id: format!("p{i}"),
                    severity,
                    status,
                    summary: "x".to_string(),
                    details: None,
                    remediation: vec![],
                    evidence: vec![],
                    fix,
                })
                .collect();

            let expected = problems
                .iter()
                .any(|p| p.severity == Severity::Critical && p.status == Status::Fail);

            let report = DoctorReport {
                strategy: StrategyReport {
                    cgroup_root: CgroupStrategyKind::Direct,
                    why: "x".to_string(),
                },
                mode: CleanupMode::Degraded,
                problems,
                fixes: vec![],
            };

            prop_assert_eq!(report.has_critical_failures(), expected);
        }

        #[test]
        fn consolidate_fixes_never_invents_advice(
            triples in proptest::collection::vec((arb_status(), proptest::option::of(arb_fix_key())), 0..200)
        ) {
            let problems: Vec<Problem> = triples
                .into_iter()
                .enumerate()
                .map(|(i, (status, fix))| Problem {
                    id: format!("p{i}"),
                    severity: Severity::Critical,
                    status,
                    summary: "x".to_string(),
                    details: None,
                    remediation: vec![],
                    evidence: vec![],
                    fix,
                })
                .collect();

            let failing_fix_keys: std::collections::BTreeSet<FixKey> = problems
                .iter()
                .filter(|p| p.status == Status::Fail)
                .filter_map(|p| p.fix)
                .collect();

            let fixes = consolidate_fixes(&problems);
            for fix in fixes {
                prop_assert!(failing_fix_keys.contains(&fix.key));
            }
        }

        #[test]
        fn consolidate_fixes_is_complete_over_failing_fix_keys(
            triples in proptest::collection::vec((arb_status(), proptest::option::of(arb_fix_key())), 0..200)
        ) {
            let problems: Vec<Problem> = triples
                .into_iter()
                .enumerate()
                .map(|(i, (status, fix))| Problem {
                    id: format!("p{i}"),
                    severity: Severity::Critical,
                    status,
                    summary: "x".to_string(),
                    details: None,
                    remediation: vec![],
                    evidence: vec![],
                    fix,
                })
                .collect();

            let failing_fix_keys: std::collections::BTreeSet<FixKey> = problems
                .iter()
                .filter(|p| p.status == Status::Fail)
                .filter_map(|p| p.fix)
                .collect();

            let fixes = consolidate_fixes(&problems);
            let fix_keys: std::collections::BTreeSet<FixKey> = fixes.iter().map(|f| f.key).collect();

            // Precedence-not-subsumption semantics: consolidation is lossless over failing fix keys.
            prop_assert_eq!(fix_keys, failing_fix_keys);
        }

        #[test]
        fn consolidate_fixes_is_monotone_under_added_failures(
            base in proptest::collection::vec((arb_status(), proptest::option::of(arb_fix_key())), 0..200),
            extra in proptest::collection::vec((arb_status(), proptest::option::of(arb_fix_key())), 0..200)
        ) {
            let base_problems: Vec<Problem> = base
                .into_iter()
                .enumerate()
                .map(|(i, (status, fix))| Problem {
                    id: format!("base{i}"),
                    severity: Severity::Critical,
                    status,
                    summary: "x".to_string(),
                    details: None,
                    remediation: vec![],
                    evidence: vec![],
                    fix,
                })
                .collect();

            let mut combined = base_problems.clone();
            combined.extend(
                extra
                    .into_iter()
                    .enumerate()
                    .map(|(i, (status, fix))| Problem {
                        id: format!("extra{i}"),
                        severity: Severity::Critical,
                        status,
                        summary: "x".to_string(),
                        details: None,
                        remediation: vec![],
                        evidence: vec![],
                        fix,
                    }),
            );

            let base_fix_keys: std::collections::BTreeSet<FixKey> =
                consolidate_fixes(&base_problems).iter().map(|f| f.key).collect();
            let combined_fix_keys: std::collections::BTreeSet<FixKey> =
                consolidate_fixes(&combined).iter().map(|f| f.key).collect();

            // Adding more failing problems must not cause previously-required fix keys to disappear.
            prop_assert!(base_fix_keys.is_subset(&combined_fix_keys));
        }

        #[test]
        fn json_roundtrip_preserves_report_meaning(report in arb_report()) {
            let before_has_critical_failures = report.has_critical_failures();
            let before_failing_problem_fix_keys: std::collections::BTreeSet<FixKey> = report
                .problems
                .iter()
                .filter(|p| p.status == Status::Fail)
                .filter_map(|p| p.fix)
                .collect();
            let before_fix_advice_keys: std::collections::BTreeSet<FixKey> =
                report.fixes.iter().map(|f| f.key).collect();

            let json = serde_json::to_value(&report)
                .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("serialize: {e}")))?;
            let roundtrip: DoctorReport = serde_json::from_value(json)
                .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("deserialize: {e}")))?;

            prop_assert_eq!(roundtrip.has_critical_failures(), before_has_critical_failures);

            let after_failing_problem_fix_keys: std::collections::BTreeSet<FixKey> = roundtrip
                .problems
                .iter()
                .filter(|p| p.status == Status::Fail)
                .filter_map(|p| p.fix)
                .collect();
            let after_fix_advice_keys: std::collections::BTreeSet<FixKey> =
                roundtrip.fixes.iter().map(|f| f.key).collect();

            prop_assert_eq!(after_failing_problem_fix_keys, before_failing_problem_fix_keys);
            prop_assert_eq!(after_fix_advice_keys, before_fix_advice_keys);
        }

        #[test]
        fn run_admin_setup_fix_commands_are_canonical_and_dominant(
            triples in proptest::collection::vec((arb_status(), proptest::option::of(arb_fix_key())), 0..200)
        ) {
            let problems: Vec<Problem> = triples
                .into_iter()
                .enumerate()
                .map(|(i, (status, fix))| Problem {
                    id: format!("p{i}"),
                    severity: Severity::Critical,
                    status,
                    summary: "x".to_string(),
                    details: None,
                    remediation: vec![],
                    evidence: vec![],
                    fix,
                })
                .collect();

            let has_failing_admin_setup = problems
                .iter()
                .any(|p| p.status == Status::Fail && p.fix == Some(FixKey::RunAdminSetup));

            let fixes = consolidate_fixes(&problems);

            if has_failing_admin_setup {
                prop_assert!(!fixes.is_empty());
                prop_assert_eq!(fixes[0].key, FixKey::RunAdminSetup);

                let Some(admin) = fixes.iter().find(|f| f.key == FixKey::RunAdminSetup) else {
                    return Err(proptest::test_runner::TestCaseError::fail(
                        "RunAdminSetup fix must be present when key is present",
                    ));
                };

                prop_assert_eq!(admin.commands.len(), 1);
                prop_assert_eq!(admin.commands[0].as_str(), "sudo locald admin setup");
            } else {
                prop_assert!(fixes.iter().all(|f| f.key != FixKey::RunAdminSetup));
            }
        }
    }
}
