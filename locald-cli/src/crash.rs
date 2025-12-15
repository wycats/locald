use anyhow::Result;
use crossterm::style::Stylize;
use std::fmt::Write;
use std::path::PathBuf;

use crate::hints;

fn shim_setup_advice(err: &anyhow::Error) -> Option<String> {
    // Heuristic matching: these errors commonly come from `locald_utils::shim`.
    // We want to print a clear stderr hint even when we also write a crash log.
    const NEEDS_SETUP_MARKERS: [&str; 3] = [
        "locald-shim is not installed or not setuid root",
        "Privileged locald-shim not found",
        "Privileged locald-shim is not configured",
    ];

    let matches_marker = err.chain().any(|cause| {
        let msg = cause.to_string();
        NEEDS_SETUP_MARKERS
            .iter()
            .any(|marker| msg.contains(marker))
    });

    if !matches_marker {
        return None;
    }

    let cmd = hints::admin_setup_command_for_current_exe();
    Some(format!(
        "locald-shim needs to be installed and setuid root. Run: `{}`",
        cmd
    ))
}

pub fn handle_crash(err: anyhow::Error) {
    // 1. Capture context
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let crash_filename = format!("crash-{}.log", timestamp);

    // Determine crash dir
    let crash_dir = get_crash_dir();
    let crash_path = crash_dir.join(&crash_filename);

    // 2. Prepare crash report
    let mut report = String::new();
    let _ = writeln!(report, "Timestamp: {}", chrono::Local::now().to_rfc3339());
    let _ = writeln!(report, "Version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(report, "Args: {:?}", std::env::args().collect::<Vec<_>>());
    let _ = writeln!(report, "Environment:");
    for (key, value) in std::env::vars() {
        if key.starts_with("LOCALD_") || key == "PATH" || key == "SHELL" || key == "TERM" {
            let _ = writeln!(report, "  {}={}", key, value);
        }
    }
    let _ = writeln!(report, "\nError:");
    let _ = writeln!(report, "{:?}", err); // Debug format includes stack trace if RUST_BACKTRACE is set

    // 3. Write to file
    if let Err(e) = write_crash_file(&crash_path, &report) {
        // Fallback: Nuclear Option
        eprintln!("{}", "✖ An unexpected error occurred.".red().bold());
        eprintln!("{}", "✖ Failed to write crash log.".red());
        eprintln!("Details:\n{}", report);
        eprintln!("Original Error writing log: {}", e);
    } else {
        // Respectful Notification
        eprintln!("{}", "✖ An unexpected error occurred.".red().bold());
        if let Some(advice) = shim_setup_advice(&err) {
            eprintln!("  {}", advice.yellow());
        }
        eprintln!(
            "  Details written to: {}",
            crash_path.display().to_string().bold()
        );
    }

    std::process::exit(1);
}

fn get_crash_dir() -> PathBuf {
    // Try project state dir if locald.toml exists
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("locald.toml").exists() {
            return locald_utils::project::get_state_dir(&cwd).join("crashes");
        }
    }

    // Fallback to global
    if let Some(base) = directories::BaseDirs::new() {
        return base.data_local_dir().join("locald").join("crashes");
    }

    // Fallback to tmp
    PathBuf::from("/tmp/locald-crashes")
}

fn write_crash_file(path: &std::path::Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}
