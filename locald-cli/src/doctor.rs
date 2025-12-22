use crate::style;
use anyhow::Result;
use crossterm::style::Stylize;
use crossterm::tty::IsTty;
use locald_utils::privileged::{AcquireConfig, CleanupMode, DoctorReport, Severity, Status};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

struct DoctorCompactTheme;

impl cliclack::Theme for DoctorCompactTheme {
    fn format_log(&self, text: &str, symbol: &str) -> String {
        // Default cliclack log formatting intentionally adds an extra trailing
        // spacer line after each log message. For `locald doctor`, that creates
        // too much vertical padding, especially for grouped/nested content.
        self.format_log_with_spacing(text, symbol, false)
    }
}

struct DoctorThemeGuard;

impl DoctorThemeGuard {
    fn install() -> Self {
        cliclack::set_theme(DoctorCompactTheme);
        Self
    }
}

impl Drop for DoctorThemeGuard {
    fn drop(&mut self) {
        cliclack::reset_theme();
    }
}

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

fn render_command(cmd: &str) -> Cow<'_, str> {
    let cmd = cmd.strip_prefix("sudo locald ").unwrap_or(cmd);

    if let Some(rest) = cmd.strip_prefix("admin ") {
        return Cow::Owned(format!("locald admin {rest}"));
    }

    Cow::Borrowed(cmd)
}

fn summarize_resolves(summaries: &[String]) -> String {
    const MAX: usize = 3;

    if summaries.len() <= MAX {
        return summaries.join("; ");
    }

    let head = summaries[..MAX].join("; ");
    format!("{head}; and {} more", summaries.len() - MAX)
}

fn normalize_commands(commands: &[String]) -> Vec<String> {
    commands
        .iter()
        .map(|cmd| render_command(cmd).into_owned())
        .collect()
}

fn tty_wrap_width() -> usize {
    // `cliclack` renders with its own prefix + guide-lines. Keep some headroom so
    // long details wrap before the terminal hard-wraps mid-word.
    let (_rows, cols) = console::Term::stdout().size();
    let cols = cols as usize;
    // Empirically, cliclack's tree prefixes consume more columns than expected,
    // and terminal hard-wrap mid-word looks terrible. Prefer being conservative.
    cols.saturating_sub(24).max(60)
}

fn wrap_for_tty(text: &str, width: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }

    textwrap::wrap(text, textwrap::Options::new(width).break_words(true))
        .into_iter()
        .map(Cow::into_owned)
        .collect()
}

fn tty_remark_wrapped(text: impl AsRef<str>) {
    let width = tty_wrap_width();

    let raw = text.as_ref();
    // Prefer splitting on clause boundaries first, then wrapping each clause.
    // This keeps important tokens (e.g. "unix:///...") intact and avoids
    // terminal hard-wrap in the middle of words.
    let clauses: Vec<&str> = raw.split(';').collect();
    if clauses.len() <= 1 {
        for line in wrap_for_tty(raw, width) {
            let _ = cliclack::log::remark(line);
        }
        return;
    }

    for clause in clauses {
        let clause = clause.trim();
        if clause.is_empty() {
            continue;
        }
        for line in wrap_for_tty(clause, width) {
            let _ = cliclack::log::remark(line);
        }
    }
}

fn render_human(report: &DoctorReport, verbose: bool) {
    if std::io::stdout().is_tty() {
        render_human_cliclack(report, verbose);
    } else {
        render_human_plain(report, verbose);
    }
}

fn render_human_plain(report: &DoctorReport, verbose: bool) {
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

    let optional_rollup = render_optional_integrations();

    if report.problems.is_empty() {
        println!("{} All critical checks passed (core).", style::CHECK);
        if !optional_rollup.unavailable.is_empty() {
            println!("{} Optional missing (non-blocking):", style::WARN);
            for item in &optional_rollup.unavailable {
                println!("  - {item}");
            }
        }
        if !optional_rollup.unknown.is_empty() {
            println!("{} Optional unknown (non-blocking):", style::WARN);
            for item in &optional_rollup.unknown {
                println!("  - {item}");
            }
        }
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
        }

        let mut covered_command_lists: BTreeSet<Vec<String>> = BTreeSet::new();
        for fix in &report.fixes {
            let cmds = normalize_commands(&fix.commands);
            if !cmds.is_empty() {
                covered_command_lists.insert(cmds);
            }
        }

        let mut extra_command_lists: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
        for p in &report.problems {
            if p.status != Status::Fail {
                continue;
            }
            if p.remediation.is_empty() {
                continue;
            }

            let cmds = normalize_commands(&p.remediation);
            if cmds.is_empty() {
                continue;
            }

            if covered_command_lists.contains(&cmds) {
                continue;
            }

            extra_command_lists
                .entry(cmds)
                .or_default()
                .push(p.summary.clone());
        }

        if !report.fixes.is_empty() || !extra_command_lists.is_empty() {
            println!();
            println!("{} Suggested next steps:", style::PACKAGE);

            let fix_group_count = report.fixes.len() + extra_command_lists.len();

            for fix in &report.fixes {
                if fix_group_count == 1 {
                    println!("- {}", fix.summary.as_str().bold());
                } else {
                    println!("- {}", fix.summary);
                }
                let mut saw_admin_setup = false;

                for cmd in normalize_commands(&fix.commands) {
                    if cmd == "locald admin setup" {
                        saw_admin_setup = true;
                    }
                    println!("  - {}", cmd.as_str().bold());
                }

                if saw_admin_setup {
                    println!("  - Next: run locald up.");
                }
            }

            for (cmds, summaries) in extra_command_lists {
                println!("- Run:");
                for cmd in cmds {
                    println!("  - {}", cmd.as_str().bold());
                }

                if !summaries.is_empty() {
                    println!("  - Resolves: {}", summarize_resolves(&summaries));
                }
            }

            if !optional_rollup.unavailable.is_empty() {
                println!("{} Optional missing (non-blocking):", style::WARN);
                for item in &optional_rollup.unavailable {
                    println!("  - {item}");
                }
            }
            if !optional_rollup.unknown.is_empty() {
                println!("{} Optional unknown (non-blocking):", style::WARN);
                for item in &optional_rollup.unknown {
                    println!("  - {item}");
                }
            }
        }
    }
}

fn render_human_cliclack(report: &DoctorReport, verbose: bool) {
    let _theme = DoctorThemeGuard::install();

    let _ = cliclack::log::info(format!(
        "Strategy: {} ({})",
        match report.strategy.cgroup_root {
            locald_utils::privileged::CgroupStrategyKind::Systemd => "systemd",
            locald_utils::privileged::CgroupStrategyKind::Direct => "direct",
        },
        report.strategy.why
    ));

    let _ = cliclack::log::info(format!(
        "Cleanup mode: {}",
        match report.mode {
            CleanupMode::Enabled => "enabled",
            CleanupMode::Degraded => "degraded",
        }
    ));

    let optional_rollup = render_optional_integrations_cliclack(verbose);

    if report.problems.is_empty() {
        let _ = cliclack::log::success("All critical checks passed (core).");

        // Make the last line summarize what won't work, even when core is OK.
        if !optional_rollup.unavailable.is_empty() {
            let _ = cliclack::log::warning("Optional missing (non-blocking):");
            for item in &optional_rollup.unavailable {
                let _ = cliclack::log::remark(item);
            }
        }
        if !optional_rollup.unknown.is_empty() {
            let _ = cliclack::log::info("Optional unknown (non-blocking):");
            for item in &optional_rollup.unknown {
                let _ = cliclack::log::remark(item);
            }
        }
        return;
    }

    let _ = cliclack::log::warning("Problems:");
    for p in &report.problems {
        // Avoid ANSI styling inside wrapped lines: `textwrap` doesn't account for
        // escape sequences when measuring width.
        let sev = match p.severity {
            Severity::Critical => "critical",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };

        let status = match p.status {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
        };

        // Headline (wrapped): keep the summary visible, move IDs/details to nested lines.
        let headline = format!("[{status}] {sev}: {}", p.summary);
        let width = tty_wrap_width();
        let headline_wrapped = wrap_for_tty(&headline, width).join("\n");
        match (p.status, p.severity) {
            (Status::Fail, Severity::Critical) => {
                let _ = cliclack::log::error(headline_wrapped);
            }
            (Status::Fail, Severity::Warning) => {
                let _ = cliclack::log::warning(headline_wrapped);
            }
            _ => {
                let _ = cliclack::log::info(headline_wrapped);
            }
        }

        // Avoid ANSI styling inside wrapped remark lines; it breaks width accounting.
        let _ = cliclack::log::remark(format!("check: {}", p.id));
        if let Some(details) = &p.details {
            tty_remark_wrapped(details);
        }

        if verbose && !p.evidence.is_empty() {
            for e in &p.evidence {
                tty_remark_wrapped(format!("{}: {}", e.key, e.value));
            }
        }
    }

    let mut covered_command_lists: BTreeSet<Vec<String>> = BTreeSet::new();
    for fix in &report.fixes {
        let cmds = normalize_commands(&fix.commands);
        if !cmds.is_empty() {
            covered_command_lists.insert(cmds);
        }
    }

    let mut extra_command_lists: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
    for p in &report.problems {
        if p.status != Status::Fail {
            continue;
        }
        if p.remediation.is_empty() {
            continue;
        }

        let cmds = normalize_commands(&p.remediation);
        if cmds.is_empty() {
            continue;
        }

        if covered_command_lists.contains(&cmds) {
            continue;
        }

        extra_command_lists
            .entry(cmds)
            .or_default()
            .push(p.summary.clone());
    }

    if report.fixes.is_empty() && extra_command_lists.is_empty() {
        return;
    }

    let fix_group_count = report.fixes.len() + extra_command_lists.len();
    if fix_group_count == 1 {
        let _ = cliclack::log::info("Fix:");
    } else {
        let _ = cliclack::log::info("Fixes:");
    }

    for fix in &report.fixes {
        if fix_group_count == 1 {
            let _ = cliclack::log::info(fix.summary.as_str().bold());
        } else {
            let _ = cliclack::log::info(&fix.summary);
        }
        let mut saw_admin_setup = false;

        for cmd in normalize_commands(&fix.commands) {
            if cmd == "locald admin setup" {
                saw_admin_setup = true;
            }
            let _ = cliclack::log::info(cmd.as_str().bold());
        }

        if saw_admin_setup {
            let _ = cliclack::log::info("Next: run locald up.");
        }
    }

    for (cmds, summaries) in extra_command_lists {
        let _ = cliclack::log::info("Run:");
        for cmd in cmds {
            let _ = cliclack::log::info(cmd.as_str().bold());
        }

        if !summaries.is_empty() {
            tty_remark_wrapped(format!("Resolves: {}", summarize_resolves(&summaries)));
        }
    }

    // After the actionable Fix block, summarize any missing optional integrations.
    if !optional_rollup.unavailable.is_empty() {
        let _ = cliclack::log::warning("Optional missing (non-blocking):");
        for item in &optional_rollup.unavailable {
            let _ = cliclack::log::remark(item);
        }
    }
    if !optional_rollup.unknown.is_empty() {
        let _ = cliclack::log::info("Optional unknown (non-blocking):");
        for item in &optional_rollup.unknown {
            let _ = cliclack::log::remark(item);
        }
    }
}

#[derive(Debug, Default)]
struct OptionalIntegrationsRollup {
    unavailable: Vec<String>,
    unknown: Vec<String>,
}

fn render_optional_integrations() -> OptionalIntegrationsRollup {
    let mut rollup = OptionalIntegrationsRollup::default();
    println!("{} Optional integrations:", style::PACKAGE);

    #[cfg(unix)]
    {
        use std::path::Path;

        // Buildpacks (CNB) runs through the privileged shim (OCI), not the Docker daemon.
        let (cnb_available, cnb_checked) = match locald_utils::shim::find() {
            Ok(Some(path)) => match locald_utils::shim::is_privileged(&path) {
                Ok(true) => (
                    true,
                    Some(format!("privileged locald-shim at {}", path.display())),
                ),
                Ok(false) => (
                    false,
                    Some(format!(
                        "locald-shim at {} is not privileged (needs setuid root)",
                        path.display()
                    )),
                ),
                Err(e) => (
                    false,
                    Some(format!(
                        "failed to inspect locald-shim at {}; {e}",
                        path.display()
                    )),
                ),
            },
            Ok(None) => (
                false,
                Some("locald-shim not found next to the locald executable".to_string()),
            ),
            Err(e) => (false, Some(format!("failed to locate locald-shim; {e}"))),
        };

        if cnb_available {
            println!("- Buildpacks (CNB) (optional): {}", "available".green());
            println!("  - Dependency: privileged locald-shim installed");
            println!("  - Impact: buildpack-based builds enabled");
            if let Some(details) = cnb_checked.as_deref() {
                println!("  - Checked: {details}");
            }
        } else {
            rollup.unavailable.push("Buildpacks (CNB)".to_string());
            println!("- Buildpacks (CNB) (optional): {}", "unavailable".yellow());
            println!("  - Dependency: requires privileged locald-shim");
            println!("  - Impact: buildpack-based builds will be unavailable");
            println!("  - If you need this: run locald admin setup");
            if let Some(details) = cnb_checked.as_deref() {
                println!("  - Checked: {details}");
            }
        }

        #[cfg(target_os = "linux")]
        {
            use std::fs::OpenOptions;

            let kvm_path = Path::new("/dev/kvm");
            if kvm_path.exists() {
                match OpenOptions::new().read(true).write(true).open(kvm_path) {
                    Ok(_) => {
                        println!("- Virtualization (KVM) (optional): {}", "available".green());
                        println!("  - Checked: /dev/kvm");
                        println!("  - Impact: VM-based workflows enabled");
                    }
                    Err(e) => {
                        rollup.unavailable.push("Virtualization (KVM)".to_string());
                        println!(
                            "- Virtualization (KVM) (optional): {}",
                            "unavailable".yellow()
                        );
                        println!("  - Checked: /dev/kvm; {e}");
                        println!("  - Impact: VM-based workflows will be unavailable");
                        println!("  - If you need this: enable KVM (/dev/kvm access)");
                    }
                }
            } else {
                rollup.unavailable.push("Virtualization (KVM)".to_string());
                println!(
                    "- Virtualization (KVM) (optional): {}",
                    "unavailable".yellow()
                );
                println!("  - Checked: /dev/kvm: not found");
                println!("  - Impact: VM-based workflows will be unavailable");
                println!("  - If you need this: enable KVM (/dev/kvm access)");
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            rollup.unknown.push("Virtualization".to_string());
            println!("- Virtualization (optional): {}", "unknown".yellow());
            println!("  - Checked: not supported on this platform");
            println!("  - Impact: VM-based workflows status unknown");
        }
    }

    #[cfg(not(unix))]
    {}

    rollup
}

fn render_optional_integrations_cliclack(verbose: bool) -> OptionalIntegrationsRollup {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum IntegrationStatus {
        Available,
        Unavailable,
        Unknown,
    }

    struct IntegrationLine {
        name: String,
        headline: String,
        status: IntegrationStatus,
        details: Vec<String>,
    }

    fn log_integration(line: IntegrationLine) {
        match line.status {
            IntegrationStatus::Available => {
                let _ = cliclack::log::success(line.headline);
            }
            IntegrationStatus::Unavailable => {
                let _ = cliclack::log::warning(line.headline);
            }
            IntegrationStatus::Unknown => {
                let _ = cliclack::log::info(line.headline);
            }
        }

        for detail in line.details {
            tty_remark_wrapped(detail);
        }
    }

    // We print a roll-up summary for this section so "unavailable" doesn't get lost
    // among otherwise-similar lines.
    let mut lines: Vec<IntegrationLine> = Vec::new();
    let mut available_count: usize = 0;
    let mut unavailable_count: usize = 0;

    #[cfg(unix)]
    {
        use std::path::Path;

        // Buildpacks (CNB) runs through the privileged shim (OCI), not the Docker daemon.
        let (cnb_available, cnb_checked) = match locald_utils::shim::find() {
            Ok(Some(path)) => match locald_utils::shim::is_privileged(&path) {
                Ok(true) => (
                    true,
                    Some(format!("privileged locald-shim at {}", path.display())),
                ),
                Ok(false) => (
                    false,
                    Some(format!(
                        "locald-shim at {} is not privileged (needs setuid root)",
                        path.display()
                    )),
                ),
                Err(e) => (
                    false,
                    Some(format!(
                        "failed to inspect locald-shim at {}; {e}",
                        path.display()
                    )),
                ),
            },
            Ok(None) => (
                false,
                Some("locald-shim not found next to the locald executable".to_string()),
            ),
            Err(e) => (false, Some(format!("failed to locate locald-shim; {e}"))),
        };

        if cnb_available {
            available_count += 1;
            lines.push(IntegrationLine {
                name: "Buildpacks (CNB)".to_string(),
                headline: "Buildpacks (CNB) (optional): available".to_string(),
                status: IntegrationStatus::Available,
                details: {
                    let mut details = Vec::new();
                    details.push("Impact: buildpack-based builds enabled".to_string());
                    if verbose {
                        details.push("Dependency: privileged locald-shim installed".to_string());
                        if let Some(details_str) = cnb_checked.as_deref() {
                            details.push(format!("Checked: {details_str}"));
                        }
                    }
                    details
                },
            });
        } else {
            unavailable_count += 1;
            lines.push(IntegrationLine {
                name: "Buildpacks (CNB)".to_string(),
                headline: "Buildpacks (CNB) (optional): unavailable".to_string(),
                status: IntegrationStatus::Unavailable,
                details: {
                    let mut details = Vec::new();
                    details.push("Impact: buildpack-based builds will be unavailable".to_string());
                    details.push("If you need this: run locald admin setup".to_string());
                    if verbose {
                        details.push("Dependency: requires privileged locald-shim".to_string());
                        if let Some(details_str) = cnb_checked.as_deref() {
                            details.push(format!("Checked: {details_str}"));
                        }
                    }
                    details
                },
            });
        }

        #[cfg(target_os = "linux")]
        {
            use std::fs::OpenOptions;

            let kvm_path = Path::new("/dev/kvm");
            if kvm_path.exists() {
                match OpenOptions::new().read(true).write(true).open(kvm_path) {
                    Ok(_) => {
                        available_count += 1;
                        lines.push(IntegrationLine {
                            name: "Virtualization (KVM)".to_string(),
                            headline: "Virtualization (KVM) (optional): available".to_string(),
                            status: IntegrationStatus::Available,
                            details: {
                                let mut details = Vec::new();
                                details.push("Impact: VM-based workflows enabled".to_string());
                                if verbose {
                                    details.push("Checked: /dev/kvm".to_string());
                                }
                                details
                            },
                        });
                    }
                    Err(e) => {
                        unavailable_count += 1;
                        lines.push(IntegrationLine {
                            name: "Virtualization (KVM)".to_string(),
                            headline: "Virtualization (KVM) (optional): unavailable".to_string(),
                            status: IntegrationStatus::Unavailable,
                            details: {
                                let mut details = Vec::new();
                                details.push(
                                    "Impact: VM-based workflows will be unavailable".to_string(),
                                );
                                details.push(
                                    "If you need this: enable KVM (/dev/kvm access)".to_string(),
                                );
                                if verbose {
                                    details.push(format!("Checked: /dev/kvm; {e}"));
                                }
                                details
                            },
                        });
                    }
                }
            } else {
                unavailable_count += 1;
                lines.push(IntegrationLine {
                    name: "Virtualization (KVM)".to_string(),
                    headline: "Virtualization (KVM) (optional): unavailable".to_string(),
                    status: IntegrationStatus::Unavailable,
                    details: {
                        let mut details = Vec::new();
                        details.push("Impact: VM-based workflows will be unavailable".to_string());
                        details.push("If you need this: enable KVM (/dev/kvm access)".to_string());
                        if verbose {
                            details.push("Checked: /dev/kvm: not found".to_string());
                        }
                        details
                    },
                });
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            lines.push(IntegrationLine {
                name: "Virtualization".to_string(),
                headline: "Virtualization (optional): unknown".to_string(),
                status: IntegrationStatus::Unknown,
                details: {
                    let mut details = Vec::new();
                    details.push("Impact: VM-based workflows status unknown".to_string());
                    if verbose {
                        details.push("Checked: not supported on this platform".to_string());
                    }
                    details
                },
            });
        }
    }

    #[cfg(not(unix))]
    {}

    let unknown_count = lines
        .iter()
        .filter(|l| l.status == IntegrationStatus::Unknown)
        .count();

    // Roll-up for the whole section.
    let mut rollup = OptionalIntegrationsRollup::default();
    for l in &lines {
        match l.status {
            IntegrationStatus::Unavailable => rollup.unavailable.push(l.name.clone()),
            IntegrationStatus::Unknown => rollup.unknown.push(l.name.clone()),
            IntegrationStatus::Available => {}
        }
    }

    let heading = if unknown_count == 0 {
        format!(
            "Optional integrations: {available_count} available, {unavailable_count} unavailable"
        )
    } else {
        format!(
            "Optional integrations: {available_count} available, {unavailable_count} unavailable, {unknown_count} unknown"
        )
    };

    if unavailable_count > 0 {
        let _ = cliclack::log::warning(heading);
    } else {
        let _ = cliclack::log::info(heading);
    }

    for line in lines {
        log_integration(line);
    }

    rollup
}
