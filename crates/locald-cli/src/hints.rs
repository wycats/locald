use locald_core::ipc::DaemonIdentity;
use std::path::{Path, PathBuf};

fn find_in_path(program: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;

    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

pub fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    paths_refer_to_same_file_result(a, b).unwrap_or(false)
}

fn paths_refer_to_same_file_result(a: &Path, b: &Path) -> Option<bool> {
    Some(a.canonicalize().ok()? == b.canonicalize().ok()?)
}

pub fn admin_setup_command_for_current_exe() -> String {
    let Ok(current_exe) = std::env::current_exe() else {
        return "locald admin setup".to_string();
    };

    if let Some(locald_on_path) = find_in_path("locald") {
        if paths_refer_to_same_file(&locald_on_path, &current_exe) {
            return "locald admin setup".to_string();
        }
    }

    format!("{} admin setup", current_exe.display())
}

pub fn daemon_identity_mismatch_warning(
    cli_version: &str,
    cli_executable: &Path,
    daemon: &DaemonIdentity,
) -> Option<String> {
    let version_mismatch = daemon.version != cli_version;
    let executable_mismatch = paths_refer_to_same_file_result(&daemon.executable, cli_executable)
        .map(|same| !same)
        .unwrap_or(false);

    if !version_mismatch && !executable_mismatch {
        return None;
    }

    let mut reasons = Vec::new();
    if version_mismatch {
        reasons.push("version mismatch");
    }
    if executable_mismatch {
        reasons.push("executable mismatch");
    }

    Some(format!(
        "WARNING: locald CLI and daemon identity differ ({reasons}).\n  CLI: version {cli_version}, executable {cli_executable}\n  Daemon: version {daemon_version}, executable {daemon_executable}, pid {daemon_pid}\nRun `locald debug identity` for details; run `locald server restart` to restart the daemon from this CLI binary.",
        reasons = reasons.join(", "),
        cli_executable = cli_executable.display(),
        daemon_version = daemon.version,
        daemon_executable = daemon.executable.display(),
        daemon_pid = daemon.pid,
    ))
}

#[cfg(test)]
mod tests {
    use super::{daemon_identity_mismatch_warning, paths_refer_to_same_file};
    use locald_core::ipc::DaemonIdentity;
    use std::path::{Path, PathBuf};

    fn daemon_identity(version: &str, executable: impl Into<PathBuf>) -> DaemonIdentity {
        DaemonIdentity {
            version: version.to_string(),
            pid: 1234,
            executable: executable.into(),
        }
    }

    fn fixture_path(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn paths_refer_to_same_file_matches_existing_file() {
        let cargo_toml = fixture_path("Cargo.toml");

        assert!(paths_refer_to_same_file(&cargo_toml, &cargo_toml));
    }

    #[test]
    fn paths_refer_to_same_file_returns_false_for_missing_file() {
        let missing = fixture_path("missing-locald-file");
        let cargo_toml = fixture_path("Cargo.toml");

        assert!(!paths_refer_to_same_file(&missing, &cargo_toml));
    }

    #[test]
    fn daemon_identity_mismatch_warning_does_not_report_executable_when_uncomparable() {
        let cli_executable = fixture_path("Cargo.toml");
        let daemon = daemon_identity("1.2.3", fixture_path("missing-locald-file"));

        assert!(daemon_identity_mismatch_warning("1.2.3", &cli_executable, &daemon).is_none());
    }

    #[test]
    fn daemon_identity_mismatch_warning_returns_none_for_matching_identity() {
        let cli_executable = fixture_path("Cargo.toml");
        let daemon = daemon_identity("1.2.3", cli_executable.clone());

        assert!(daemon_identity_mismatch_warning("1.2.3", &cli_executable, &daemon).is_none());
    }

    #[test]
    fn daemon_identity_mismatch_warning_reports_version_mismatch() {
        let cli_executable = fixture_path("Cargo.toml");
        let daemon = daemon_identity("1.2.2", cli_executable.clone());

        let warning = daemon_identity_mismatch_warning("1.2.3", &cli_executable, &daemon)
            .expect("version mismatch should warn");

        assert!(warning.contains("version mismatch"));
        assert!(warning.contains("CLI: version 1.2.3"));
        assert!(warning.contains("Daemon: version 1.2.2"));
        assert!(warning.contains("pid 1234"));
        assert!(warning.contains("locald debug identity"));
        assert!(warning.contains("locald server restart"));
    }

    #[test]
    fn daemon_identity_mismatch_warning_preserves_the_complete_identity_message() {
        let cli_executable = fixture_path("Cargo.toml");
        let daemon = daemon_identity("1.2.2", cli_executable.clone());

        let warning = daemon_identity_mismatch_warning("1.2.3", &cli_executable, &daemon)
            .expect("version mismatch should warn");

        assert_eq!(
            warning,
            format!(
                "WARNING: locald CLI and daemon identity differ (version mismatch).\n  CLI: version 1.2.3, executable {}\n  Daemon: version 1.2.2, executable {}, pid 1234\nRun `locald debug identity` for details; run `locald server restart` to restart the daemon from this CLI binary.",
                cli_executable.display(),
                daemon.executable.display(),
            )
        );
    }

    #[test]
    fn daemon_identity_mismatch_warning_reports_executable_mismatch() {
        let cli_executable = fixture_path("Cargo.toml");
        let daemon = daemon_identity("1.2.3", fixture_path("README.md"));

        let warning = daemon_identity_mismatch_warning("1.2.3", &cli_executable, &daemon)
            .expect("executable mismatch should warn");

        assert!(warning.contains("executable mismatch"));
        assert!(warning.contains(&cli_executable.display().to_string()));
        assert!(warning.contains(&daemon.executable.display().to_string()));
    }

    #[test]
    fn daemon_identity_mismatch_warning_reports_both_identities_for_combined_mismatch() {
        let cli_executable = fixture_path("Cargo.toml");
        let daemon = daemon_identity("1.2.2", fixture_path("README.md"));

        let warning = daemon_identity_mismatch_warning("1.2.3", &cli_executable, &daemon)
            .expect("combined mismatch should warn");

        assert!(warning.contains("version mismatch"));
        assert!(warning.contains("executable mismatch"));
        assert!(warning.contains("CLI: version 1.2.3"));
        assert!(warning.contains(&cli_executable.display().to_string()));
        assert!(warning.contains("Daemon: version 1.2.2"));
        assert!(warning.contains(&daemon.executable.display().to_string()));
        assert!(warning.contains("pid 1234"));
    }
}
