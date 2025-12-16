use directories::ProjectDirs;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Returns the state directory for a project.
///
/// This directory is used to store build artifacts, logs, and other ephemeral state.
/// It is located in `XDG_DATA_HOME/locald/projects/<name>-<hash>/.locald`.
///
/// The `.locald` suffix is required by `locald-shim` for security reasons.
///
/// # Example
/// ```
/// use locald_utils::project::get_state_dir;
/// use std::path::Path;
///
/// let path = Path::new("/tmp/my-project");
/// let state_dir = get_state_dir(path);
/// // state_dir is something like ~/.local/share/locald/projects/my-project-a1b2c3d4/.locald
/// ```
pub fn get_state_dir(project_path: &Path) -> PathBuf {
    let abs_path =
        std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());

    // Calculate hash of the absolute path
    let mut hasher = Sha256::new();
    hasher.update(abs_path.to_string_lossy().as_bytes());
    let result = hasher.finalize();
    let hash_str = hex::encode(result);

    // Use first 8 chars of hash for brevity
    let short_hash = &hash_str[..8];

    let project_name = abs_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let dir_name = format!("{project_name}-{short_hash}");

    ProjectDirs::from("com", "locald", "locald").map_or_else(
        || project_path.join(".locald"),
        |dirs| {
            dirs.data_local_dir()
                .join("projects")
                .join(dir_name)
                .join(".locald")
        },
    )
}
