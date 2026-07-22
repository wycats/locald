//! Fail-closed macOS installation, readiness, and repair transaction.

use anyhow::{Context, Result};
use locald_helper_protocol::{
    AUTHORITY_MAX_BYTES, AUTHORITY_PATH, HELPER_PATH, HELPER_PLIST_PATH, HelperAuthority,
    code_signing, load_authority,
};
use locald_utils::privileged::{
    CgroupStrategyKind, CleanupMode, DoctorReport, EvidenceItem, FixAdvice, FixKey, Problem,
    Severity, Status, StrategyReport,
};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const AGENT_LABEL: &str = "com.locald.agent";
const AGENT_BYTES: &[u8] = include_bytes!(env!("LOCALD_EMBEDDED_AGENT_PATH"));
const HELPER_BYTES: &[u8] = include_bytes!(env!("LOCALD_EMBEDDED_HELPER_PATH"));
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
struct SetupOwner {
    uid: u32,
    gid: u32,
    home: PathBuf,
}

#[derive(Debug, Clone)]
struct InstallationPaths {
    certs: PathBuf,
    agent: PathBuf,
    launch_agent: PathBuf,
    daemon: PathBuf,
}

impl InstallationPaths {
    fn for_owner(owner: &SetupOwner, daemon: PathBuf) -> Self {
        let data = owner
            .home
            .join("Library")
            .join("Application Support")
            .join("locald");
        Self {
            certs: data.join("certs"),
            agent: data.join("locald-agent"),
            launch_agent: owner
                .home
                .join("Library/LaunchAgents/com.locald.agent.plist"),
            daemon,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportCaller {
    User,
    SetupRoot,
}

trait SetupPlatform {
    fn install_system_trust(&self, certificate: &Path) -> Result<()>;
    fn retire_legacy_host_aliases(&self) -> Result<()>;
    fn restart_launch_agent(&self, owner: &SetupOwner, plist: &Path) -> Result<()>;
    fn install_helper(&self, bytes: &[u8], authority: &HelperAuthority) -> Result<()>;
    fn probe_helper(&self) -> Result<()>;
}

struct SystemPlatform;

impl SetupPlatform for SystemPlatform {
    fn install_system_trust(&self, certificate: &Path) -> Result<()> {
        crate::trust::install_ca_macos(certificate)
    }

    fn retire_legacy_host_aliases(&self) -> Result<()> {
        let hosts = locald_core::HostsFileSection::new();
        let path = Path::new("/etc/hosts");
        let current = std::fs::read_to_string(path).context("could not read /etc/hosts")?;
        let updated = retire_legacy_hosts_content(&hosts, &current);
        if updated != current {
            let metadata = std::fs::symlink_metadata(path)
                .context("could not inspect /etc/hosts before synchronization")?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                anyhow::bail!("/etc/hosts is not a regular file");
            }
            atomic_install_file(
                path,
                updated.as_bytes(),
                metadata.mode() & 0o7777,
                metadata.uid(),
                metadata.gid(),
            )
            .context("could not atomically synchronize /etc/hosts")?;
        }
        Ok(())
    }

    fn restart_launch_agent(&self, owner: &SetupOwner, plist: &Path) -> Result<()> {
        let service = format!("gui/{}/{AGENT_LABEL}", owner.uid);
        let output = std::process::Command::new("launchctl")
            .args(["bootout", &service])
            .output()
            .context("failed to run launchctl bootout for menu bar agent")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !launchctl_service_absent(output.status.code(), &stderr) {
                anyhow::bail!("launchctl bootout {service} failed: {}", stderr.trim());
            }
        }

        let status = std::process::Command::new("launchctl")
            .args(["enable", &service])
            .status()
            .context("failed to run launchctl enable for menu bar agent")?;
        if !status.success() {
            anyhow::bail!("launchctl enable {service} failed with status {status}");
        }

        let status = std::process::Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{}", owner.uid)])
            .arg(plist)
            .status()
            .context("failed to run launchctl bootstrap for menu bar agent")?;
        if !status.success() {
            anyhow::bail!(
                "launchctl bootstrap gui/{} failed with status {status}",
                owner.uid
            );
        }
        Ok(())
    }

    fn install_helper(&self, bytes: &[u8], authority: &HelperAuthority) -> Result<()> {
        crate::macos_helper::install(bytes, authority)
    }

    fn probe_helper(&self) -> Result<()> {
        crate::macos_helper::probe()
    }
}

fn launchctl_service_absent(exit_code: Option<i32>, stderr: &str) -> bool {
    exit_code == Some(113)
        || stderr.contains("No such process")
        || stderr.contains("Could not find service")
        || stderr.contains("Could not find specified service")
}

fn retire_legacy_hosts_content(hosts: &locald_core::HostsFileSection, current: &str) -> String {
    hosts.remove_domains_from_content(current, locald_core::LEGACY_MACOS_HOST_ALIASES)
}

/// Resolve and cross-check the non-root console user that owns this setup.
fn resolve_setup_owner() -> Result<SetupOwner> {
    let uid = parse_sudo_id("SUDO_UID")?;
    let gid = parse_sudo_id("SUDO_GID")?;
    let name = std::env::var("SUDO_USER").context(
        "SUDO_USER is missing; run `sudo locald admin setup` from the active user's session",
    )?;
    if uid == 0 || gid == 0 || name == "root" {
        anyhow::bail!(
            "direct root setup has no non-root owner; run `sudo locald admin setup` from the active user's session"
        );
    }

    let by_name = nix::unistd::User::from_name(&name)
        .context("could not resolve SUDO_USER")?
        .with_context(|| format!("SUDO_USER {name:?} does not name an account"))?;
    let by_uid = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .context("could not resolve SUDO_UID")?
        .with_context(|| format!("SUDO_UID {uid} does not name an account"))?;
    let console_uid = std::fs::metadata("/dev/console")
        .context("could not inspect /dev/console")?
        .uid();
    validate_setup_owner_identity(
        uid,
        gid,
        &name,
        by_name.uid.as_raw(),
        by_name.gid.as_raw(),
        &by_uid.name,
        by_uid.gid.as_raw(),
        console_uid,
    )?;
    Ok(SetupOwner {
        uid,
        gid,
        home: by_name.dir,
    })
}

#[allow(clippy::similar_names, clippy::too_many_arguments)]
fn validate_setup_owner_identity(
    uid: u32,
    gid: u32,
    name: &str,
    name_uid: u32,
    name_gid: u32,
    uid_name: &str,
    uid_gid: u32,
    console_uid: u32,
) -> Result<()> {
    if name_uid != uid || name_gid != gid || uid_name != name || uid_gid != gid {
        anyhow::bail!(
            "SUDO_USER, SUDO_UID, and SUDO_GID do not describe the same account; run setup from a coherent sudo session"
        );
    }
    if console_uid != uid {
        anyhow::bail!(
            "setup owner UID {uid} does not own /dev/console (owner UID {console_uid}); run setup from the intended user's active console session"
        );
    }
    Ok(())
}

fn parse_sudo_id(name: &str) -> Result<u32> {
    std::env::var(name)
        .with_context(|| {
            format!(
                "{name} is missing; run `sudo locald admin setup` from the active user's session"
            )
        })?
        .parse()
        .with_context(|| format!("{name} is not a valid numeric ID"))
}

/// Run the complete idempotent macOS repair transaction.
pub fn run_setup() -> Result<()> {
    if !nix::unistd::geteuid().is_root() {
        use std::os::unix::process::CommandExt;
        let executable = std::env::current_exe().context("could not resolve locald executable")?;
        let error = std::process::Command::new("sudo")
            .arg("--")
            .arg(executable)
            .args(["admin", "setup"])
            .exec();
        anyhow::bail!("failed to exec sudo for admin setup: {error}");
    }

    let owner = resolve_setup_owner()?;
    let daemon = std::env::current_exe()
        .context("could not resolve locald executable")?
        .canonicalize()
        .context("could not canonicalize locald executable")?;
    let paths = InstallationPaths::for_owner(&owner, daemon);
    let authority = crate::macos_helper::authority_for_current_executable(owner.uid)?;

    cliclack::intro("locald admin setup (macOS)")?;
    run_setup_with(
        &owner,
        &paths,
        AGENT_BYTES,
        HELPER_BYTES,
        &authority,
        &SystemPlatform,
    )?;

    let report = collect_report_for(ReportCaller::SetupRoot, false)?;
    if report.has_critical_failures() {
        anyhow::bail!(
            "macOS setup completed its repair transaction, but readiness checks still fail; run `locald doctor --json` for details"
        );
    }
    cliclack::outro("Setup complete")?;
    println!("Next: run `locald up`.");
    Ok(())
}

fn run_setup_with(
    owner: &SetupOwner,
    paths: &InstallationPaths,
    agent_bytes: &[u8],
    helper_bytes: &[u8],
    authority: &HelperAuthority,
    platform: &impl SetupPlatform,
) -> Result<()> {
    let ca = locald_utils::cert::repair_root_ca_in_dir(&paths.certs, owner.uid, owner.gid)
        .context("could not establish valid Root CA material")?;
    platform
        .install_system_trust(&ca.paths.cert_path)
        .context("could not install Root CA into system trust")?;
    platform
        .retire_legacy_host_aliases()
        .context("could not retire locald's legacy hosts-file aliases")?;

    ensure_directory(
        paths.agent.parent().context("agent path has no parent")?,
        owner.uid,
        owner.gid,
        0o700,
    )
    .context("could not repair the locald application-support directory")?;
    atomic_install_file(&paths.agent, agent_bytes, 0o755, owner.uid, owner.gid)
        .context("could not install the embedded menu bar agent")?;
    ensure_directory(
        paths
            .launch_agent
            .parent()
            .context("LaunchAgent path has no parent")?,
        owner.uid,
        owner.gid,
        0o755,
    )
    .context("could not repair the user's LaunchAgents directory")?;
    let plist = render_launch_agent_plist(&paths.agent, &paths.daemon);
    atomic_install_file(
        &paths.launch_agent,
        plist.as_bytes(),
        0o644,
        owner.uid,
        owner.gid,
    )
    .context("could not install the locald LaunchAgent plist")?;
    platform
        .restart_launch_agent(owner, &paths.launch_agent)
        .context("could not restart the locald menu bar agent")?;

    platform
        .install_helper(helper_bytes, authority)
        .map_err(|error| {
            anyhow::anyhow!("could not install the privileged helper transaction: {error:#}")
        })?;
    platform
        .probe_helper()
        .context("privileged helper postflight probe failed")?;
    Ok(())
}

/// Collect the canonical structured macOS installation-readiness report.
pub fn collect_report(verbose: bool) -> Result<DoctorReport> {
    if std::env::var_os("LOCALD_SANDBOX_ACTIVE").is_some()
        || crate::global_config::load().server.is_sandbox()
    {
        return Ok(sandbox_report());
    }
    collect_report_for(ReportCaller::User, verbose)
}

fn collect_report_for(caller: ReportCaller, _verbose: bool) -> Result<DoctorReport> {
    let current_executable = std::env::current_exe()
        .context("could not resolve current locald executable")?
        .canonicalize()
        .context("could not canonicalize current locald executable")?;

    let authority_result = (caller == ReportCaller::SetupRoot)
        .then(|| load_authority(Path::new(AUTHORITY_PATH)).map_err(anyhow::Error::from));
    let owner_result = match caller {
        ReportCaller::User => console_owner(),
        ReportCaller::SetupRoot => authority_result.as_ref().map_or_else(
            || {
                Err(anyhow::anyhow!(
                    "helper authority was not loaded for setup postflight"
                ))
            },
            |result| {
                result.as_ref().map_or_else(
                    |error| Err(anyhow::anyhow!("helper authority unavailable: {error}")),
                    setup_owner_from_authority,
                )
            },
        ),
    };
    let paths = owner_result
        .as_ref()
        .map(|owner| InstallationPaths::for_owner(owner, current_executable.clone()));
    let mut problems = Vec::new();

    push_check(
        &mut problems,
        "macos.console_user",
        "Console-user identity is coherent",
        validate_report_caller(
            caller,
            authority_result
                .as_ref()
                .and_then(|result| result.as_ref().ok()),
            owner_result.as_ref().ok(),
        ),
    );

    match paths.as_ref() {
        Ok(paths) => {
            push_check(
                &mut problems,
                "macos.ca.material",
                "Root CA certificate and key are valid and matched",
                locald_utils::cert::validate_root_ca_material_in_dir(&paths.certs).map(|_| ()),
            );
            push_check(
                &mut problems,
                "macos.ca.permissions",
                "Root CA ownership and permissions are exact",
                owner_result.as_ref().map_or_else(
                    |error| Err(anyhow::anyhow!("setup owner unavailable: {error}")),
                    |owner| {
                        locald_utils::cert::validate_root_ca_permissions_in_dir(
                            &paths.certs,
                            owner.uid,
                            owner.gid,
                        )
                    },
                ),
            );
            push_check(
                &mut problems,
                "macos.ca.trust",
                "Root CA is trusted by the macOS system trust store",
                if locald_utils::cert::is_ca_path_trusted(&paths.certs.join("rootCA.pem")) {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("Root CA is not trusted by the system"))
                },
            );
            push_check(
                &mut problems,
                "macos.agent.binary",
                "Embedded menu bar agent is installed with exact integrity",
                validate_file(
                    &paths.agent,
                    AGENT_BYTES,
                    owner_result
                        .as_ref()
                        .ok()
                        .map(|owner| (owner.uid, owner.gid)),
                    0o755,
                ),
            );
            push_check(
                &mut problems,
                "macos.agent.launch_agent",
                "LaunchAgent targets the installed agent and current locald executable",
                validate_launch_agent(
                    &paths.launch_agent,
                    paths,
                    owner_result
                        .as_ref()
                        .ok()
                        .map(|owner| (owner.uid, owner.gid)),
                ),
            );
        }
        Err(error) => {
            for (id, summary) in [
                (
                    "macos.ca.material",
                    "Root CA certificate and key are valid and matched",
                ),
                (
                    "macos.ca.permissions",
                    "Root CA ownership and permissions are exact",
                ),
                (
                    "macos.ca.trust",
                    "Root CA is trusted by the macOS system trust store",
                ),
                (
                    "macos.agent.binary",
                    "Embedded menu bar agent is installed with exact integrity",
                ),
                (
                    "macos.agent.launch_agent",
                    "LaunchAgent targets the installed agent and current locald executable",
                ),
            ] {
                push_check(
                    &mut problems,
                    id,
                    summary,
                    Err(anyhow::anyhow!(error.to_string())),
                );
            }
        }
    }

    let helper_binary = validate_file(Path::new(HELPER_PATH), HELPER_BYTES, Some((0, 0)), 0o755);
    let helper_plist = validate_helper_plist(Path::new(HELPER_PLIST_PATH));
    let helper_authority = validate_helper_authority(
        caller,
        authority_result
            .as_ref()
            .map(|result| result.as_ref().map_err(std::string::ToString::to_string)),
        &current_executable,
    );
    let helper_static_ready =
        helper_binary.is_ok() && helper_plist.is_ok() && helper_authority.is_ok();

    push_check(
        &mut problems,
        "macos.helper.binary",
        "Privileged helper binary has exact integrity",
        helper_binary,
    );
    push_check(
        &mut problems,
        "macos.helper.plist",
        "Privileged helper LaunchDaemon is installed securely",
        helper_plist,
    );
    push_check(
        &mut problems,
        "macos.helper.authority",
        "Helper authority is installed with secure metadata",
        helper_authority,
    );
    push_check(
        &mut problems,
        "macos.helper.probe",
        "Privileged helper is reachable and authorizes this locald executable",
        if helper_static_ready {
            crate::macos_helper::probe()
        } else {
            Err(anyhow::anyhow!(
                "helper probe skipped until binary, plist, and authority checks pass"
            ))
        },
    );

    Ok(report_from_problems(problems))
}

fn setup_owner_from_authority(authority: &HelperAuthority) -> Result<SetupOwner> {
    let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(authority.console_user_uid))?
        .with_context(|| {
            format!(
                "configured helper user {} no longer exists",
                authority.console_user_uid
            )
        })?;
    Ok(SetupOwner {
        uid: user.uid.as_raw(),
        gid: user.gid.as_raw(),
        home: user.dir,
    })
}

fn console_owner() -> Result<SetupOwner> {
    let console_uid = std::fs::metadata("/dev/console")
        .context("could not inspect /dev/console")?
        .uid();
    if console_uid == 0 {
        anyhow::bail!("/dev/console is not owned by a non-root user");
    }
    let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(console_uid))?
        .with_context(|| format!("console owner UID {console_uid} does not name an account"))?;
    Ok(SetupOwner {
        uid: user.uid.as_raw(),
        gid: user.gid.as_raw(),
        home: user.dir,
    })
}

fn validate_helper_authority(
    caller: ReportCaller,
    authority: Option<std::result::Result<&HelperAuthority, String>>,
    current_executable: &Path,
) -> Result<()> {
    validate_authority_metadata(Path::new(AUTHORITY_PATH))?;
    if caller == ReportCaller::User {
        return Ok(());
    }
    let authority = authority
        .context("helper authority was not loaded for setup postflight")?
        .map_err(anyhow::Error::msg)?;
    if authority.executable_path != current_executable {
        anyhow::bail!(
            "helper authority targets {}, current executable is {}",
            authority.executable_path.display(),
            current_executable.display()
        );
    }
    if authority.executable_version != env!("LOCALD_BUILD_VERSION") {
        anyhow::bail!(
            "helper authority version {} does not match current version {}",
            authority.executable_version,
            env!("LOCALD_BUILD_VERSION")
        );
    }
    code_signing::path_satisfies_requirement(current_executable, &authority.designated_requirement)
        .context("current executable does not satisfy helper authority")
}

fn validate_authority_metadata(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("helper authority is not a regular file");
    }
    let mode = metadata.mode() & 0o7777;
    if metadata.uid() != 0 || metadata.gid() != 0 {
        anyhow::bail!(
            "helper authority must be owned by root:wheel (found {}:{})",
            metadata.uid(),
            metadata.gid()
        );
    }
    if mode != 0o600 {
        anyhow::bail!("helper authority must have mode 0600 (found {mode:04o})");
    }
    if metadata.len() > AUTHORITY_MAX_BYTES {
        anyhow::bail!("helper authority exceeds {AUTHORITY_MAX_BYTES} bytes");
    }
    Ok(())
}

fn validate_report_caller(
    caller: ReportCaller,
    authority: Option<&HelperAuthority>,
    owner: Option<&SetupOwner>,
) -> Result<()> {
    let owner = owner.context("configured helper user is unavailable")?;
    let console_uid = std::fs::metadata("/dev/console")?.uid();
    let configured_uid = authority.map_or(owner.uid, |authority| authority.console_user_uid);
    if console_uid != configured_uid || owner.uid != configured_uid {
        anyhow::bail!(
            "configured UID {}, console UID {}, and account UID {} do not agree",
            configured_uid,
            console_uid,
            owner.uid
        );
    }
    let caller_uid = nix::unistd::getuid().as_raw();
    match caller {
        ReportCaller::User if caller_uid != owner.uid => {
            anyhow::bail!(
                "locald is running as UID {caller_uid}, expected UID {}",
                owner.uid
            )
        }
        ReportCaller::SetupRoot if caller_uid != 0 => {
            anyhow::bail!("setup postflight must run as root")
        }
        ReportCaller::User | ReportCaller::SetupRoot => Ok(()),
    }
}

fn validate_file(path: &Path, expected: &[u8], owner: Option<(u32, u32)>, mode: u32) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("{} is not a regular file", path.display());
    }
    let actual_mode = metadata.mode() & 0o7777;
    if actual_mode != mode {
        anyhow::bail!(
            "{} has mode {actual_mode:04o}, expected {mode:04o}",
            path.display()
        );
    }
    if let Some((uid, gid)) = owner
        && (metadata.uid() != uid || metadata.gid() != gid)
    {
        anyhow::bail!(
            "{} is owned by {}:{}, expected {uid}:{gid}",
            path.display(),
            metadata.uid(),
            metadata.gid()
        );
    }
    if std::fs::read(path)? != expected {
        anyhow::bail!("{} does not match the embedded component", path.display());
    }
    Ok(())
}

fn validate_launch_agent(
    path: &Path,
    paths: &InstallationPaths,
    owner: Option<(u32, u32)>,
) -> Result<()> {
    let expected = render_launch_agent_plist(&paths.agent, &paths.daemon);
    validate_file(path, expected.as_bytes(), owner, 0o644)
        .context("LaunchAgent plist does not target the installed components")
}

fn validate_helper_plist(path: &Path) -> Result<()> {
    let expected = crate::macos_helper::render_launch_daemon_plist();
    validate_file(path, expected.as_bytes(), Some((0, 0)), 0o644)
        .context("helper plist does not publish the exact expected service")
}

fn push_check(problems: &mut Vec<Problem>, id: &str, summary: &str, result: Result<()>) {
    match result {
        Ok(()) => problems.push(Problem {
            id: id.to_string(),
            severity: Severity::Critical,
            status: Status::Pass,
            summary: summary.to_string(),
            details: None,
            remediation: vec![],
            evidence: vec![],
            fix: None,
        }),
        Err(error) => {
            let details = format!("{error:#}");
            problems.push(Problem {
                id: id.to_string(),
                severity: Severity::Critical,
                status: Status::Fail,
                summary: summary.to_string(),
                details: Some(details.clone()),
                remediation: vec!["sudo locald admin setup".to_string()],
                evidence: vec![EvidenceItem {
                    key: "error".to_string(),
                    value: details,
                }],
                fix: Some(FixKey::RunAdminSetup),
            });
        }
    }
}

fn report_from_problems(problems: Vec<Problem>) -> DoctorReport {
    let fixes = problems
        .iter()
        .any(|problem| problem.status == Status::Fail)
        .then(|| FixAdvice {
            key: FixKey::RunAdminSetup,
            summary: "Repair the complete privileged macOS installation".to_string(),
            commands: vec!["sudo locald admin setup".to_string()],
        })
        .into_iter()
        .collect();
    DoctorReport {
        strategy: StrategyReport {
            cgroup_root: CgroupStrategyKind::Direct,
            why: "macOS privileged helper and trusted HTTPS".to_string(),
        },
        mode: CleanupMode::Enabled,
        problems,
        fixes,
    }
}

fn sandbox_report() -> DoctorReport {
    let problems = [
        ("macos.console_user", "Console-user identity"),
        ("macos.ca.material", "Root CA material"),
        ("macos.ca.permissions", "Root CA permissions"),
        ("macos.ca.trust", "System Root CA trust"),
        ("macos.agent.binary", "Menu bar agent integrity"),
        ("macos.agent.launch_agent", "LaunchAgent target"),
        ("macos.helper.binary", "Privileged helper integrity"),
        ("macos.helper.plist", "Privileged helper LaunchDaemon"),
        ("macos.helper.authority", "Privileged helper authority"),
        ("macos.helper.probe", "Privileged helper probe"),
    ]
    .into_iter()
    .map(|(id, summary)| Problem {
        id: id.to_string(),
        severity: Severity::Info,
        status: Status::Skip,
        summary: format!("{summary} is not required in explicit sandbox mode"),
        details: None,
        remediation: vec![],
        evidence: vec![],
        fix: None,
    })
    .collect();
    report_from_problems(problems)
}

pub fn render_launch_agent_plist(agent: &Path, daemon: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>LOCALD_DAEMON_PATH</key>
        <string>{}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
</dict>
</plist>"#,
        escape_xml(&agent.display().to_string()),
        escape_xml(&daemon.display().to_string())
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn ensure_directory(path: &Path, uid: u32, gid: u32, mode: u32) -> Result<()> {
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "installation directory is not a real directory: {}",
            path.display()
        );
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    nix::unistd::chown(
        path,
        Some(nix::unistd::Uid::from_raw(uid)),
        Some(nix::unistd::Gid::from_raw(gid)),
    )?;
    Ok(())
}

fn atomic_install_file(path: &Path, bytes: &[u8], mode: u32, uid: u32, gid: u32) -> Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        anyhow::bail!(
            "installation path is not a regular file: {}",
            path.display()
        );
    }
    let parent = path.parent().context("installation path has no parent")?;
    let name = path
        .file_name()
        .context("installation path has no file name")?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.tmp.{}.{}",
        name.to_string_lossy(),
        std::process::id(),
        sequence
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        nix::unistd::fchown(
            &file,
            Some(nix::unistd::Uid::from_raw(uid)),
            Some(nix::unistd::Gid::from_raw(gid)),
        )?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingPlatform {
        calls: Mutex<Vec<&'static str>>,
    }

    impl SetupPlatform for RecordingPlatform {
        fn install_system_trust(&self, _certificate: &Path) -> Result<()> {
            self.calls.lock().unwrap().push("trust");
            Ok(())
        }

        fn retire_legacy_host_aliases(&self) -> Result<()> {
            self.calls.lock().unwrap().push("hosts-cleanup");
            Ok(())
        }

        fn restart_launch_agent(&self, _owner: &SetupOwner, _plist: &Path) -> Result<()> {
            self.calls.lock().unwrap().push("agent");
            Ok(())
        }

        fn install_helper(&self, _bytes: &[u8], _authority: &HelperAuthority) -> Result<()> {
            self.calls.lock().unwrap().push("helper");
            Ok(())
        }

        fn probe_helper(&self) -> Result<()> {
            self.calls.lock().unwrap().push("probe");
            Ok(())
        }
    }

    fn fixture() -> (
        tempfile::TempDir,
        SetupOwner,
        InstallationPaths,
        HelperAuthority,
    ) {
        let root = tempfile::tempdir().unwrap();
        let owner = SetupOwner {
            uid: nix::unistd::getuid().as_raw(),
            gid: nix::unistd::getgid().as_raw(),
            home: root.path().to_path_buf(),
        };
        let daemon = root.path().join("bin/locald");
        std::fs::create_dir_all(daemon.parent().unwrap()).unwrap();
        std::fs::write(&daemon, b"locald").unwrap();
        let paths = InstallationPaths::for_owner(&owner, daemon.clone());
        let authority = HelperAuthority::new(
            owner.uid,
            "identifier test".to_string(),
            daemon,
            "test".to_string(),
        )
        .unwrap();
        (root, owner, paths, authority)
    }

    #[test]
    fn repair_transaction_is_atomic_idempotent_and_replaces_stale_components() {
        let (_root, owner, paths, authority) = fixture();
        let platform = RecordingPlatform::default();

        run_setup_with(
            &owner,
            &paths,
            b"agent-v1",
            b"helper-v1",
            &authority,
            &platform,
        )
        .unwrap();
        std::fs::write(&paths.agent, b"stale").unwrap();
        run_setup_with(
            &owner,
            &paths,
            b"agent-v1",
            b"helper-v1",
            &authority,
            &platform,
        )
        .unwrap();

        assert_eq!(std::fs::read(&paths.agent).unwrap(), b"agent-v1");
        assert_eq!(
            std::fs::metadata(&paths.agent).unwrap().mode() & 0o7777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(&paths.launch_agent).unwrap().mode() & 0o7777,
            0o644
        );
        assert_eq!(
            platform.calls.lock().unwrap().as_slice(),
            [
                "trust",
                "hosts-cleanup",
                "agent",
                "helper",
                "probe",
                "trust",
                "hosts-cleanup",
                "agent",
                "helper",
                "probe"
            ]
        );
    }

    #[test]
    fn readiness_report_has_stable_ids_and_one_setup_fix() {
        let mut problems = Vec::new();
        push_check(
            &mut problems,
            "macos.helper.probe",
            "helper probe",
            Err(anyhow::anyhow!("unavailable")),
        );
        push_check(
            &mut problems,
            "macos.ca.trust",
            "CA trust",
            Err(anyhow::anyhow!("untrusted")),
        );
        let report = report_from_problems(problems);

        assert!(report.has_critical_failures());
        assert_eq!(report.problems[0].id, "macos.helper.probe");
        assert_eq!(report.problems[1].id, "macos.ca.trust");
        assert_eq!(report.fixes.len(), 1);
        assert_eq!(report.fixes[0].commands, ["sudo locald admin setup"]);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["problems"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn setup_owner_requires_coherent_account_and_console_identity() {
        validate_setup_owner_identity(501, 20, "owner", 501, 20, "owner", 20, 501)
            .expect("coherent setup owner");
        assert!(
            validate_setup_owner_identity(501, 20, "owner", 502, 20, "owner", 20, 501)
                .unwrap_err()
                .to_string()
                .contains("do not describe the same account")
        );
        assert!(
            validate_setup_owner_identity(501, 20, "owner", 501, 20, "owner", 20, 502)
                .unwrap_err()
                .to_string()
                .contains("does not own /dev/console")
        );
    }

    #[test]
    fn launch_agent_absence_is_a_clean_first_install() {
        assert!(launchctl_service_absent(
            Some(113),
            "Boot-out failed: 113: Could not find specified service"
        ));
        assert!(launchctl_service_absent(
            Some(1),
            "Could not find specified service"
        ));
        assert!(!launchctl_service_absent(Some(1), "permission denied"));
    }

    #[test]
    fn setup_retires_only_legacy_hosts_aliases() {
        let hosts = locald_core::HostsFileSection::with_path(PathBuf::from("/etc/hosts"));
        let current = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 locald.local\n127.0.0.1 workbench.example.test\n# END locald\n";

        let updated = retire_legacy_hosts_content(&hosts, current);
        assert!(!updated.contains("locald.local"));
        assert!(updated.contains("127.0.0.1 workbench.example.test"));
    }

    #[test]
    fn sandbox_report_keeps_stable_checks_but_exempts_privileged_components() {
        let report = sandbox_report();
        assert!(!report.has_critical_failures());
        assert!(report.fixes.is_empty());
        assert_eq!(report.problems.len(), 10);
        assert!(
            report
                .problems
                .iter()
                .all(|problem| problem.status == Status::Skip)
        );
    }
}
