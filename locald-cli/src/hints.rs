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

pub fn admin_setup_command_for_current_exe() -> String {
    let Ok(current_exe) = std::env::current_exe() else {
        return "sudo locald admin setup".to_string();
    };

    if let Some(locald_on_path) = find_in_path("locald") {
        if paths_refer_to_same_file(&locald_on_path, &current_exe) {
            return "sudo locald admin setup".to_string();
        }
    }

    format!("sudo {} admin setup", current_exe.display())
}
