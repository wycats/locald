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

fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Returns the appropriate admin setup command for the current executable.
///
/// On macOS, this returns a message indicating the command is not available
/// since `locald admin setup` is a Linux-only feature.
pub fn admin_setup_command_for_current_exe() -> String {
    // On macOS, admin setup is not available
    #[cfg(target_os = "macos")]
    {
        return "locald admin setup (Linux only - not needed on macOS)".to_string();
    }

    #[cfg(not(target_os = "macos"))]
    {
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
}

/// Check if the current platform supports privileged operations via shim.
/// This is Linux-only since macOS doesn't use the shim for privileged ops.
///
/// Reserved for future use in shim-related error messages.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub const fn platform_supports_shim() -> bool {
    true
}

/// Get platform-appropriate advice for privileged port access.
///
/// Reserved for future use in port binding error messages.
#[allow(dead_code)]
pub const fn privileged_port_advice() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "On macOS, binding to ports below 1024 requires root.\n\
         Consider using high ports (e.g., 8080, 8443) instead.\n\
         The locald proxy will route traffic to your service regardless of the port."
    }

    #[cfg(not(target_os = "macos"))]
    {
        "On Linux, run `locald admin setup` to enable privileged port binding\n\
         via the setuid helper, or use high ports (e.g., 8080, 8443)."
    }
}
