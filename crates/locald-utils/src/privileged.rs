//! Privileged capability acquisition and host readiness reporting.
//!
//! This module provides a shared, structured report used by `locald doctor` and by
//! privileged call sites to prevent readiness drift.

#![allow(missing_docs)]

use crate::cert;
#[cfg(target_os = "linux")]
use crate::cgroup::{CgroupRootStrategy, cgroup_fs_root, is_root_ready};
use crate::shim;
use anyhow::{Context, Result};
#[cfg(target_os = "linux")]
use nix::unistd::User;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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

    // HTTPS readiness (Root CA presence)
    //
    // The daemon can run HTTP without certificates, but HTTPS requires a local Root CA.
    // On a fresh machine this is commonly missing until `locald admin setup`.
    let certs_dir = (|| -> Result<PathBuf> {
        // If running under sudo, check the invoking user's home, not root's.
        if let Ok(sudo_user) = std::env::var("SUDO_USER")
            && let Ok(Some(user)) = User::from_name(&sudo_user)
        {
            return Ok(user.dir.join(".locald").join("certs"));
        }
        cert::get_certs_dir()
    })();

    match certs_dir {
        Ok(dir) => {
            let ca_cert_path = dir.join("rootCA.pem");
            let ca_key_path = dir.join("rootCA-key.pem");
            let cert_exists = ca_cert_path.exists();
            let key_exists = ca_key_path.exists();

            if !cert_exists || !key_exists {
                problems.push(Problem {
                    id: "https.root_ca".to_string(),
                    severity: Severity::Warning,
                    status: Status::Fail,
                    summary: "HTTPS Root CA is not configured (HTTPS may be disabled or untrusted)"
                        .to_string(),
                    details: Some(
                        "Run `locald admin setup` to generate/install the locald Root CA and configure HTTPS trust."
                            .to_string(),
                    ),
                    remediation: vec![
                        "locald trust".to_string(),
                        "sudo locald admin setup".to_string(),
                    ],
                    evidence: if config.verbose {
                        vec![
                            EvidenceItem {
                                key: "certs.dir".to_string(),
                                value: dir.display().to_string(),
                            },
                            EvidenceItem {
                                key: "certs.rootCA.pem".to_string(),
                                value: if cert_exists { "present" } else { "missing" }
                                    .to_string(),
                            },
                            EvidenceItem {
                                key: "certs.rootCA-key.pem".to_string(),
                                value: if key_exists { "present" } else { "missing" }
                                    .to_string(),
                            },
                        ]
                    } else {
                        vec![]
                    },
                    fix: None,
                });
            }
        }
        Err(e) => {
            if config.verbose {
                problems.push(Problem {
                    id: "https.root_ca".to_string(),
                    severity: Severity::Info,
                    status: Status::Skip,
                    summary: "Skipped HTTPS Root CA check".to_string(),
                    details: Some(e.to_string()),
                    remediation: vec![],
                    evidence: vec![],
                    fix: None,
                });
            }
        }
    }

    // Optional integrations
    //
    // These checks must never block privileged acquisition. They exist to surface
    // integration availability (warn vs critical) in a way that's consistent with
    // the documented integration matrix.
    check_docker_integration(&config, &mut problems);
    check_kvm_integration(&config, &mut problems);

    let fixes = consolidate_fixes(&problems);

    Ok(DoctorReport {
        strategy: strategy_report,
        mode,
        problems,
        fixes,
    })
}

/// Collect a host readiness report (non-Linux stub).
///
/// On non-Linux platforms, locald has limited functionality.
/// This stub returns a report indicating the platform is not fully supported.
#[cfg(not(target_os = "linux"))]
pub fn collect_report(_config: AcquireConfig<'_>) -> Result<DoctorReport> {
    Ok(DoctorReport {
        strategy: StrategyReport {
            cgroup_root: CgroupStrategyKind::Direct,
            why: "non-Linux platform (cgroups not available)".to_string(),
        },
        mode: CleanupMode::Degraded,
        problems: vec![Problem {
            id: "platform.unsupported".to_string(),
            severity: Severity::Warning,
            status: Status::Skip,
            summary: "locald privileged features require Linux".to_string(),
            details: Some(
                "cgroups, shim, and privileged process management are Linux-only".to_string(),
            ),
            remediation: vec![],
            evidence: vec![],
            fix: None,
        }],
        fixes: vec![],
    })
}

#[cfg(target_os = "linux")]
fn check_docker_integration(config: &AcquireConfig<'_>, problems: &mut Vec<Problem>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        fn is_socket(path: &Path) -> bool {
            std::fs::metadata(path)
                .ok()
                .is_some_and(|m| m.file_type().is_socket())
        }

        let mut checked: Vec<String> = Vec::new();

        let docker_host = std::env::var("DOCKER_HOST").ok();
        if let Some(host) = &docker_host {
            // If DOCKER_HOST points at a unix socket, prefer checking that specific path.
            if let Some(rest) = host.strip_prefix("unix://") {
                if !rest.is_empty() {
                    let p = PathBuf::from(rest);
                    checked.push(p.display().to_string());
                    if is_socket(&p) {
                        return;
                    }
                }
            } else {
                // Non-unix DOCKER_HOST (tcp/ssh). We can't reliably probe connectivity here
                // without introducing network calls; treat as configured.
                return;
            }
        }

        // Common defaults.
        let mut candidates: Vec<PathBuf> = vec![
            PathBuf::from("/var/run/docker.sock"),
            PathBuf::from("/run/docker.sock"),
        ];

        if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
            let xdg = PathBuf::from(xdg);
            candidates.push(xdg.join("docker.sock"));
            candidates.push(xdg.join("podman/podman.sock"));
        }

        if let Ok(home) = std::env::var("HOME") {
            candidates.push(PathBuf::from(home).join(".docker/run/docker.sock"));
        }

        for c in candidates {
            checked.push(c.display().to_string());
            if is_socket(&c) {
                return;
            }
        }

        problems.push(Problem {
            id: "integration.docker".to_string(),
            severity: Severity::Warning,
            status: Status::Fail,
            summary: "Docker API socket not found (legacy Docker-based services will be unavailable)"
                .to_string(),
            details: Some(
                "locald can run OCI containers without Docker, but legacy Docker integration requires a reachable Docker-compatible API socket."
                    .to_string(),
            ),
            remediation: vec![
                "Start Docker (or a Docker-compatible daemon)".to_string(),
                "If needed, set DOCKER_HOST=unix:///path/to/docker.sock".to_string(),
            ],
            evidence: if config.verbose {
                let mut ev = Vec::new();
                if let Some(h) = docker_host {
                    ev.push(EvidenceItem {
                        key: "docker.host".to_string(),
                        value: h,
                    });
                }
                ev.push(EvidenceItem {
                    key: "docker.checked_sockets".to_string(),
                    value: checked.join(", "),
                });
                ev
            } else {
                vec![]
            },
            fix: None,
        });
    }

    #[cfg(not(unix))]
    {
        let _ = (config, problems);
    }
}

#[cfg(target_os = "linux")]
fn check_kvm_integration(config: &AcquireConfig<'_>, problems: &mut Vec<Problem>) {
    // Keep this behind verbose to avoid adding noisy, optional platform details to the
    // default doctor output.
    if !config.verbose {
        return;
    }

    #[cfg(unix)]
    {
        let kvm = Path::new("/dev/kvm");
        match std::fs::OpenOptions::new().read(true).write(true).open(kvm) {
            Ok(_f) => {
                // available
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                problems.push(Problem {
                    id: "integration.kvm".to_string(),
                    severity: Severity::Info,
                    status: Status::Skip,
                    summary: "KVM is not available (/dev/kvm missing); VMM features disabled"
                        .to_string(),
                    details: Some(
                        "This is expected on many machines and CI runners. locald does not require KVM unless you opt into VMM-based workflows."
                            .to_string(),
                    ),
                    remediation: vec![
                        "Enable hardware virtualization in BIOS/UEFI".to_string(),
                        "Install/load KVM modules and ensure /dev/kvm exists".to_string(),
                    ],
                    evidence: vec![],
                    fix: None,
                });
            }
            Err(e) => {
                problems.push(Problem {
                    id: "integration.kvm".to_string(),
                    severity: Severity::Warning,
                    status: Status::Fail,
                    summary: "KVM is present but not usable; VMM features may fail".to_string(),
                    details: Some(e.to_string()),
                    remediation: vec![
                        "Ensure your user can access /dev/kvm (often: add to the kvm group, then log out/in)"
                            .to_string(),
                    ],
                    evidence: vec![],
                    fix: None,
                });
            }
        }
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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
    use std::sync::{Mutex, OnceLock};

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

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[allow(unsafe_code)]
    fn set_env<K: AsRef<std::ffi::OsStr>, V: AsRef<std::ffi::OsStr>>(key: K, value: V) {
        // `std::env::set_var` is `unsafe` on some toolchains/editions.
        // These tests serialize env access via `env_lock()`.
        unsafe {
            std::env::set_var(key, value);
        }
    }

    #[allow(unsafe_code)]
    fn remove_env<K: AsRef<std::ffi::OsStr>>(key: K) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    fn restore_env(key: &str, prev: Option<String>) {
        match prev {
            Some(v) => set_env(key, v),
            None => remove_env(key),
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

    #[test]
    fn docker_integration_tcp_host_is_treated_as_configured() {
        let _guard = env_lock().lock().unwrap();

        let prev = std::env::var("DOCKER_HOST").ok();
        set_env("DOCKER_HOST", "tcp://127.0.0.1:2375");

        let mut problems = Vec::new();
        check_docker_integration(
            &AcquireConfig {
                verbose: true,
                ..AcquireConfig::daemon_default()
            },
            &mut problems,
        );
        assert!(problems.is_empty());

        restore_env("DOCKER_HOST", prev);
    }

    #[cfg(unix)]
    #[test]
    fn docker_integration_unix_socket_present_no_problem() {
        use std::os::unix::net::UnixListener;

        let _guard = env_lock().lock().unwrap();

        let prev_docker_host = std::env::var("DOCKER_HOST").ok();
        let prev_runtime = std::env::var("XDG_RUNTIME_DIR").ok();
        let prev_home = std::env::var("HOME").ok();

        let tmp = std::env::temp_dir().join(format!("locald-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");

        // Reduce the chance of accidentally finding a real daemon socket via env paths.
        set_env("XDG_RUNTIME_DIR", &tmp);
        set_env("HOME", &tmp);

        let sock_path = tmp.join("docker.sock");
        let _listener = UnixListener::bind(&sock_path).expect("bind unix socket");

        set_env(
            "DOCKER_HOST",
            format!("unix://{}", sock_path.to_string_lossy()),
        );

        let mut problems = Vec::new();
        check_docker_integration(
            &AcquireConfig {
                verbose: true,
                ..AcquireConfig::daemon_default()
            },
            &mut problems,
        );
        assert!(problems.is_empty());

        restore_env("DOCKER_HOST", prev_docker_host);
        restore_env("XDG_RUNTIME_DIR", prev_runtime);
        restore_env("HOME", prev_home);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn kvm_integration_is_quiet_when_not_verbose() {
        let mut problems = Vec::new();
        check_kvm_integration(&AcquireConfig::daemon_default(), &mut problems);
        assert!(problems.is_empty());
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
