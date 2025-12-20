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
        use std::os::unix::net::UnixStream;
        use std::path::Path;

        let docker_sock = Path::new("/var/run/docker.sock");
        let docker_sock_display = docker_sock.display();
        if !docker_sock.exists() {
            println!(
                "- Docker: {} ({docker_sock_display}: socket not found; Docker-based services will be unavailable)",
                "unavailable".yellow()
            );
            return;
        }

        match UnixStream::connect(docker_sock) {
            Ok(_) => println!(
                "- Docker: {} ({docker_sock_display}; Docker-based services enabled)",
                "available".green()
            ),
            Err(e) => println!(
                "- Docker: {} ({docker_sock_display}; {e}; Docker-based services will be unavailable)",
                "unavailable".yellow()
            ),
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
