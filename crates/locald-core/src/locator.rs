//! Stable normalization for project path locators.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Resolve symlinks through the longest existing ancestor while preserving a
/// missing suffix.
///
/// Project paths remain useful locators after the project itself disappears.
/// Canonicalizing only the complete path loses stable spelling through
/// symlinked ancestors such as macOS `/var` -> `/private/var`, so walk upward
/// until an existing ancestor can be resolved and append the missing suffix.
pub fn normalize_project_locator(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut existing_ancestor = absolute.as_path();

    loop {
        match std::fs::canonicalize(existing_ancestor) {
            Ok(canonical_ancestor) => {
                let suffix = absolute.strip_prefix(existing_ancestor).map_err(|error| {
                    io::Error::other(format!("failed to derive project locator suffix: {error}"))
                })?;
                return Ok(lexically_normalize(&canonical_ancestor.join(suffix)));
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = existing_ancestor.parent() else {
                    return Err(source);
                };
                existing_ancestor = parent;
            }
            Err(source) => return Err(source),
        }
    }
}

pub(crate) fn absolute_project_locator(path: &Path) -> io::Result<PathBuf> {
    std::path::absolute(path)
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parent_components_are_clamped_at_the_filesystem_root() {
        let normalized = lexically_normalize(Path::new("/../foo"));

        assert_eq!(normalized, PathBuf::from("/foo"));
        assert!(normalized.is_absolute());
        assert_eq!(
            lexically_normalize(Path::new("/one/../../../foo")),
            PathBuf::from("/foo")
        );
    }

    #[test]
    fn missing_suffix_is_normalized_through_a_symlinked_ancestor() {
        let dir = tempdir().expect("create locator fixture");
        let real = dir.path().join("real");
        let alias = dir.path().join("alias");
        std::fs::create_dir(&real).expect("create real locator ancestor");
        std::os::unix::fs::symlink(&real, &alias).expect("create locator symlink");

        let normalized = normalize_project_locator(&alias.join("missing/../project"))
            .expect("normalize missing project locator");

        assert_eq!(
            normalized,
            std::fs::canonicalize(real)
                .expect("canonicalize real locator ancestor")
                .join("project")
        );
    }

    #[test]
    fn parent_components_follow_symlink_filesystem_semantics() {
        let dir = tempdir().expect("create locator fixture");
        let base = dir.path().join("base");
        let elsewhere = dir.path().join("elsewhere");
        let nested = elsewhere.join("nested");
        std::fs::create_dir(&base).expect("create lexical parent");
        std::fs::create_dir_all(&nested).expect("create symlink target");
        std::fs::create_dir(elsewhere.join("project")).expect("create resolved project");
        std::os::unix::fs::symlink(&nested, base.join("alias"))
            .expect("create cross-parent symlink");

        assert_eq!(
            normalize_project_locator(&base.join("alias/../project"))
                .expect("normalize existing path through symlink parent"),
            std::fs::canonicalize(elsewhere.join("project"))
                .expect("canonicalize resolved project")
        );
        assert_eq!(
            normalize_project_locator(&base.join("alias/../missing/../future"))
                .expect("normalize missing suffix after symlink parent"),
            std::fs::canonicalize(&elsewhere)
                .expect("canonicalize resolved parent")
                .join("future")
        );
    }
}
