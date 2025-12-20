use crate::style;
use anyhow::Result;
use crossterm::style::Stylize;
use locald_utils::privileged::{AcquireConfig, CleanupMode, DoctorReport, Severity, Status};

pub fn run(json: bool, verbose: bool) -> Result<i32> {
    const SHIM_BYTES: &[u8] = include_bytes!(env!("LOCALD_EMBEDDED_SHIM_PATH"));
    let expected_version = option_env!("LOCALD_EXPECTED_SHIM_VERSION");

    let report = locald_utils::privileged::collect_report(AcquireConfig {
        verbose,
        expected_shim_version: expected_version,
        expected_shim_bytes: Some(SHIM_BYTES),
    })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_human(&report, verbose);
    }

    Ok(i32::from(report.has_critical_failures()))
}

fn render_human(report: &DoctorReport, verbose: bool) {
    println!(
        "{} Strategy: {} ({})",
        style::PACKAGE,
        match report.strategy.cgroup_root {
            locald_utils::privileged::CgroupStrategyKind::Systemd => "systemd",
            locald_utils::privileged::CgroupStrategyKind::Direct => "direct",
        }
        .bold(),
        report.strategy.why
    );

    println!(
        "{} Cleanup mode: {}",
        style::PACKAGE,
        match report.mode {
            CleanupMode::Enabled => "enabled".green(),
            CleanupMode::Degraded => "degraded".yellow(),
        }
    );

    render_optional_integrations();

    if report.problems.is_empty() {
        println!("{} All critical checks passed.", style::CHECK);
    } else {
        println!("{} Problems:", style::WARN);
        for p in &report.problems {
            let sev = match p.severity {
                Severity::Critical => "critical".red(),
                Severity::Warning => "warning".yellow(),
                Severity::Info => "info".cyan(),
            };

            let status = match p.status {
                Status::Pass => "PASS".green(),
                Status::Fail => "FAIL".red(),
                Status::Skip => "SKIP".yellow(),
            };

            println!(
                "- [{status}] {sev}: {} ({})",
                p.summary,
                p.id.as_str().dim()
            );
            if let Some(details) = &p.details {
                println!("  {details}");
            }

            if verbose && !p.evidence.is_empty() {
                for e in &p.evidence {
                    println!("  {}: {}", e.key.as_str().dim(), e.value);
                }
            }

            if !p.remediation.is_empty() {
                println!("  Fix:");
                for cmd in &p.remediation {
                    println!("    - {cmd}");
                }
            }
        }

        if !report.fixes.is_empty() {
            println!();
            println!("{} Suggested next steps:", style::PACKAGE);
            for fix in &report.fixes {
                println!("- {}", fix.summary);
                for cmd in &fix.commands {
                    println!("  - {cmd}");
                }
            }
        }
    }
}

fn render_optional_integrations() {
    println!("{} Optional integrations:", style::PACKAGE);

    #[cfg(unix)]
    {
        use std::env;
        use std::os::unix::net::UnixStream;
        use std::path::Path;

        let docker_host = env::var("DOCKER_HOST").ok();
        let docker_host_display = docker_host
            .as_deref()
            .unwrap_or("unix:///var/run/docker.sock");
        let mut docker_available = false;
        let mut docker_probe_supported = true;
        let mut docker_sock_display: Option<String> = None;
        let mut docker_unavailable_details: Option<String> = None;

        if let Some(docker_host) = docker_host.as_deref() {
            if !docker_host.starts_with("unix://") {
                docker_probe_supported = false;
                docker_unavailable_details = Some(
                    "unsupported DOCKER_HOST scheme; only unix:// sockets are supported"
                        .to_string(),
                );
            }
        }

        if docker_probe_supported {
            let docker_sock_path = docker_host
                .as_deref()
                .and_then(|h| h.strip_prefix("unix://"))
                .filter(|p| !p.is_empty())
                .unwrap_or("/var/run/docker.sock");

            let docker_sock = Path::new(docker_sock_path);
            docker_sock_display = Some(docker_sock.display().to_string());

            if docker_sock.exists() {
                match UnixStream::connect(docker_sock) {
                    Ok(_) => docker_available = true,
                    Err(e) => {
                        docker_unavailable_details =
                            Some(format!("{}; {e}", docker_sock.display()));
                    }
                }
            } else {
                docker_unavailable_details =
                    Some(format!("{}: socket not found", docker_sock.display()));
            }
        }

        if docker_available {
            println!(
                "- Docker: {} ({docker_host_display}; {}; Docker-based services enabled)",
                "available".green(),
                docker_sock_display.as_deref().unwrap_or("unknown")
            );
        } else if docker_probe_supported {
            println!(
                "- Docker: {} ({docker_host_display}; {}; Docker-based services will be unavailable)",
                "unavailable".yellow(),
                docker_unavailable_details
                    .as_deref()
                    .unwrap_or("unavailable")
            );
        } else {
            println!(
                "- Docker: {} ({docker_host_display}; {}; Docker-based services will be unavailable)",
                "unavailable".yellow(),
                docker_unavailable_details
                    .as_deref()
                    .unwrap_or("unsupported")
            );
        }

        if docker_available {
            println!(
                "- Buildpacks (CNB): {} (Docker available; buildpack-based builds enabled)",
                "available".green()
            );
        } else {
            println!(
                "- Buildpacks (CNB): {} (requires Docker; buildpack-based builds will be unavailable)",
                "unavailable".yellow()
            );
        }
    }

    #[cfg(not(unix))]
    {
        println!(
            "- Docker: {} (unsupported platform; Docker-based services status unknown)",
            "unknown".yellow()
        );
    }
}
