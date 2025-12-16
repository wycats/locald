use anyhow::{Result, anyhow};
use std::fs;
use std::path::{Component, Path, PathBuf};
use tar::Entry;
use tracing::{debug, info};

/// Safely unpacks a hard link entry from a tar archive.
///
/// OCI images often use absolute paths for hard links (e.g., `/bin/sh` -> `/bin/bash`).
/// Standard `tar` extraction treats these as absolute paths on the host, which is wrong/dangerous.
/// This function resolves the link target relative to `target_dir`.
///
/// # Errors
///
/// Returns an error if the link target cannot be resolved, if the destination path cannot be prepared,
/// or if the hard link creation fails.
pub fn unpack_hard_link<R: std::io::Read>(entry: &Entry<R>, target_dir: &Path) -> Result<()> {
    let link_target = resolve_link_target(entry, target_dir)?;
    let link_destination = resolve_entry_path(entry, target_dir)?;

    prepare_destination(&link_destination)?;
    create_hard_link(&link_target, &link_destination)
}

fn resolve_link_target<R: std::io::Read>(entry: &Entry<R>, root: &Path) -> Result<PathBuf> {
    let link_name = entry
        .link_name()?
        .ok_or_else(|| anyhow!("Hard link has no link name"))?;
    resolve_relative_to_root(root, &link_name.to_string_lossy())
}

fn resolve_entry_path<R: std::io::Read>(entry: &Entry<R>, root: &Path) -> Result<PathBuf> {
    let path = entry.path()?;
    resolve_relative_to_root(root, &path.to_string_lossy())
}

fn resolve_relative_to_root(root: &Path, path_str: &str) -> Result<PathBuf> {
    let relative_path = if path_str.starts_with('/') {
        path_str.trim_start_matches('/')
    } else {
        path_str
    };
    safe_join(root, relative_path)
}

#[allow(clippy::collapsible_if)]
fn prepare_destination(path: &Path) -> Result<()> {
    if path.exists() {
        // We ignore the error here because if it fails, the subsequent create_dir_all or hard_link will likely fail too
        // and give a better error message, or it might be a race condition where it was removed.
        // But we log it for debugging.
        if let Err(e) = fs::remove_file(path) {
            debug!(
                "Failed to remove existing file at {}: {}",
                path.display(),
                e
            );
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn create_hard_link(target: &Path, destination: &Path) -> Result<()> {
    debug!(
        "Hard linking {} -> {}",
        destination.display(),
        target.display()
    );

    if let Err(e) = fs::hard_link(target, destination) {
        // If the source doesn't exist yet (forward link), this will fail.
        // Standard tar handles this by delaying links, but for OCI layers,
        // the ordering usually ensures existence or we might need a second pass.
        // For now, we log and continue, but strictly this is an error.
        info!(
            "Failed to hard link {} to {}: {}",
            destination.display(),
            target.display(),
            e
        );
        return Err(e.into());
    }
    Ok(())
}

/// Joins a path to a root and ensures the result is inside the root.
/// Prevents directory traversal attacks (e.g. "../../../etc/passwd").
///
/// # Errors
///
/// Returns an error if the path attempts to traverse above the root directory
/// or if it contains unsupported components (like Windows prefixes).
pub fn safe_join(root: &Path, path: &str) -> Result<PathBuf> {
    let mut result = root.to_path_buf();

    for component in Path::new(path).components() {
        match component {
            Component::Normal(p) => result.push(p),
            Component::RootDir | Component::CurDir => {
                // If we encounter a root dir component in the middle, we reset to root?
                // Or we treat it as an error?
                // For OCI, we treated leading / as relative to root before calling this.
                // So we shouldn't see RootDir here usually.
                // But if we do, we should probably just ignore it or treat as relative.
            }
            Component::ParentDir => {
                if !result.pop() {
                    // If we can't pop, we are at the root.
                    // Attempting to go above root is a violation.
                    return Err(anyhow!("Path traversal detected: {path}"));
                }
                // Ensure we didn't pop above the root
                if !result.starts_with(root) {
                    return Err(anyhow!("Path traversal detected: {path}"));
                }
            }
            Component::Prefix(_) => {
                return Err(anyhow!("Windows prefixes not supported: {path}"));
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_join() {
        let root = PathBuf::from("/tmp/root");

        assert_eq!(safe_join(&root, "foo/bar").ok(), Some(root.join("foo/bar")));

        assert_eq!(safe_join(&root, "foo/../bar").ok(), Some(root.join("bar")));

        assert!(safe_join(&root, "../foo").is_err());
        assert!(safe_join(&root, "foo/../../bar").is_err());
    }
}
