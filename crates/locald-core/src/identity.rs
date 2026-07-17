//! Stable identities for Git repositories, worktrees, and locald projects.
//!
//! Paths, branches, and commits are locators or display metadata. The durable
//! identity inputs live in Git's administrative directories so ordinary Git
//! operations and filesystem moves do not change them.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

const MARKER_VERSION: &str = "v1";
const MARKER_DIRECTORY: &str = "locald";
const REPOSITORY_MARKER: &str = "repository-id";
const WORKTREE_MARKER: &str = "worktree-id";
const PROJECT_ID_DOMAIN: &[u8] = b"locald-project-v1\0";
const PROJECT_INSTANCE_ID_DOMAIN: &[u8] = b"locald-instance-v1\0";

/// An invalid textual opaque identity.
#[derive(Debug, Error)]
#[error("invalid locald identity `{value}`: {source}")]
pub struct ParseIdentityError {
    value: String,
    #[source]
    source: uuid::Error,
}

macro_rules! identity_type {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Return the UUID representation used by the durable identity format.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdentityError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value)
                    .map(Self)
                    .map_err(|source| ParseIdentityError {
                        value: value.to_owned(),
                        source,
                    })
            }
        }
    };
}

identity_type!(
    RepositoryId,
    "The stable identity of one physical Git clone."
);
identity_type!(
    WorktreeId,
    "The stable identity of one physical Git worktree."
);
identity_type!(
    ProjectId,
    "The stable identity of one repository-relative locald project."
);
identity_type!(
    ProjectInstanceId,
    "The stable identity of one locald project in one physical worktree."
);

/// The four identity levels associated with a Git-backed locald project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIdentity {
    pub repository_id: RepositoryId,
    pub worktree_id: WorktreeId,
    pub project_id: ProjectId,
    pub project_instance_id: ProjectInstanceId,
}

/// A stable identity together with its current filesystem locators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProjectIdentity {
    pub identity: ProjectIdentity,
    pub common_git_dir: PathBuf,
    pub worktree_git_dir: PathBuf,
    pub worktree_root: PathBuf,
    pub project_root: PathBuf,
    pub repository_relative_project_root: PathBuf,
}

/// A failure to resolve or persist a stable Git-backed project identity.
#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("failed to resolve project root `{path}`: {source}")]
    CanonicalizeProjectRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(
        "`{project_root}` is not inside a Git worktree; non-Git identity is assigned by the project catalog"
    )]
    NotGit { project_root: PathBuf },

    #[error(
        "Git metadata at `{git_locator}` could not be opened for `{project_root}`: {source}. If this worktree was moved, run `git worktree repair` from the repository"
    )]
    BrokenWorktree {
        project_root: PathBuf,
        git_locator: PathBuf,
        #[source]
        source: git2::Error,
    },

    #[error(
        "Git metadata at `{git_locator}` still points to the unavailable worktree `{recorded_worktree_root}` instead of `{project_root}`: {source}. Run `git worktree repair` from the repository"
    )]
    UnrepairedWorktree {
        project_root: PathBuf,
        git_locator: PathBuf,
        recorded_worktree_root: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("bare Git repository `{git_dir}` cannot contain a locald project")]
    BareRepository { git_dir: PathBuf },

    #[error("failed to resolve Git {kind} directory `{path}`: {source}")]
    CanonicalizeGitDirectory {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("project root `{project_root}` is outside the discovered worktree `{worktree_root}`")]
    ProjectOutsideWorktree {
        project_root: PathBuf,
        worktree_root: PathBuf,
    },

    #[error("failed to {operation} identity marker `{path}`: {source}")]
    MarkerIo {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid identity marker `{path}`: {reason}")]
    InvalidMarker { path: PathBuf, reason: String },

    #[error("project root `{project_root}` contains an unsupported path component `{component}`")]
    UnsupportedProjectPath {
        project_root: PathBuf,
        component: String,
    },
}

/// Resolve a Git-backed project root into stable repository, worktree, project,
/// and project-instance identities.
///
/// The supplied root may be nested within the worktree. Non-Git projects are
/// deliberately reported as [`IdentityError::NotGit`]; their persistent random
/// identities belong to the versioned project catalog.
pub fn resolve_git_project_identity(
    project_root: &Path,
) -> Result<ResolvedProjectIdentity, IdentityError> {
    let project_root = fs::canonicalize(project_root).map_err(|source| {
        IdentityError::CanonicalizeProjectRoot {
            path: project_root.to_path_buf(),
            source,
        }
    })?;

    let repository =
        if let Some((worktree_candidate, git_locator)) = nearest_git_locator(&project_root) {
            git2::Repository::open(&worktree_candidate).map_err(|source| {
                IdentityError::BrokenWorktree {
                    project_root: project_root.clone(),
                    git_locator,
                    source,
                }
            })?
        } else {
            git2::Repository::discover(&project_root).map_err(|_| IdentityError::NotGit {
                project_root: project_root.clone(),
            })?
        };

    let Some(raw_worktree_root) = repository.workdir() else {
        return Err(IdentityError::BareRepository {
            git_dir: repository.path().to_path_buf(),
        });
    };

    let worktree_root = fs::canonicalize(raw_worktree_root).map_err(|source| {
        if let Some((_, git_locator)) = nearest_git_locator(&project_root) {
            IdentityError::UnrepairedWorktree {
                project_root: project_root.clone(),
                git_locator,
                recorded_worktree_root: raw_worktree_root.to_path_buf(),
                source,
            }
        } else {
            IdentityError::CanonicalizeGitDirectory {
                kind: "worktree root",
                path: raw_worktree_root.to_path_buf(),
                source,
            }
        }
    })?;
    let common_git_dir = canonicalize_git_path("common", repository.commondir())?;
    let worktree_git_dir = canonicalize_git_path("worktree", repository.path())?;
    let repository_relative_project_root = project_root
        .strip_prefix(&worktree_root)
        .map(Path::to_path_buf)
        .map_err(|_| IdentityError::ProjectOutsideWorktree {
            project_root: project_root.clone(),
            worktree_root: worktree_root.clone(),
        })?;

    let repository_id = RepositoryId(read_or_create_marker(
        &common_git_dir
            .join(MARKER_DIRECTORY)
            .join(REPOSITORY_MARKER),
    )?);
    let worktree_id = WorktreeId(read_or_create_marker(
        &worktree_git_dir
            .join(MARKER_DIRECTORY)
            .join(WORKTREE_MARKER),
    )?);
    let project_id = derive_project_id(
        repository_id,
        &repository_relative_project_root,
        &project_root,
    )?;
    let project_instance_id = derive_project_instance_id(worktree_id, project_id);

    Ok(ResolvedProjectIdentity {
        identity: ProjectIdentity {
            repository_id,
            worktree_id,
            project_id,
            project_instance_id,
        },
        common_git_dir,
        worktree_git_dir,
        worktree_root,
        project_root,
        repository_relative_project_root,
    })
}

fn canonicalize_git_path(kind: &'static str, path: &Path) -> Result<PathBuf, IdentityError> {
    fs::canonicalize(path).map_err(|source| IdentityError::CanonicalizeGitDirectory {
        kind,
        path: path.to_path_buf(),
        source,
    })
}

fn nearest_git_locator(project_root: &Path) -> Option<(PathBuf, PathBuf)> {
    project_root.ancestors().find_map(|ancestor| {
        let git_locator = ancestor.join(".git");
        fs::symlink_metadata(&git_locator)
            .is_ok()
            .then(|| (ancestor.to_path_buf(), git_locator))
    })
}

fn derive_project_id(
    repository_id: RepositoryId,
    relative_root: &Path,
    project_root: &Path,
) -> Result<ProjectId, IdentityError> {
    let mut name = PROJECT_ID_DOMAIN.to_vec();

    for component in relative_root.components() {
        let Component::Normal(value) = component else {
            return Err(IdentityError::UnsupportedProjectPath {
                project_root: project_root.to_path_buf(),
                component: format!("{component:?}"),
            });
        };
        // This framing is a persistence contract: each repository-relative
        // component is UTF-8, length-prefixed with an unsigned 64-bit
        // big-endian length, and encoded without a platform path separator.
        // Rejecting non-UTF-8 paths avoids platform-specific derived IDs.
        let Some(value) = value.to_str() else {
            return Err(IdentityError::UnsupportedProjectPath {
                project_root: project_root.to_path_buf(),
                component: "project path components must be UTF-8".to_owned(),
            });
        };
        let bytes = value.as_bytes();
        let length =
            u64::try_from(bytes.len()).map_err(|_| IdentityError::UnsupportedProjectPath {
                project_root: project_root.to_path_buf(),
                component: "path component is too large".to_owned(),
            })?;
        name.extend_from_slice(&length.to_be_bytes());
        name.extend_from_slice(bytes);
    }

    Ok(ProjectId(Uuid::new_v5(&repository_id.0, &name)))
}

fn derive_project_instance_id(worktree_id: WorktreeId, project_id: ProjectId) -> ProjectInstanceId {
    let mut name = Vec::with_capacity(PROJECT_INSTANCE_ID_DOMAIN.len() + 16);
    name.extend_from_slice(PROJECT_INSTANCE_ID_DOMAIN);
    name.extend_from_slice(project_id.0.as_bytes());
    ProjectInstanceId(Uuid::new_v5(&worktree_id.0, &name))
}

// Identity discovery is a synchronous Git/filesystem boundary. Async callers
// must invoke it from their existing blocking discovery path.
#[allow(clippy::disallowed_methods)]
fn read_or_create_marker(path: &Path) -> Result<Uuid, IdentityError> {
    match fs::read(path) {
        Ok(contents) => return parse_marker_bytes(path, contents),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(IdentityError::MarkerIo {
                operation: "read",
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let Some(parent) = path.parent() else {
        return Err(IdentityError::InvalidMarker {
            path: path.to_path_buf(),
            reason: "marker has no parent directory".to_owned(),
        });
    };
    fs::create_dir_all(parent).map_err(|source| IdentityError::MarkerIo {
        operation: "create parent directory for",
        path: path.to_path_buf(),
        source,
    })?;

    let candidate = Uuid::new_v4();
    let temporary_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("identity"),
        Uuid::new_v4()
    ));
    let payload = format!("{MARKER_VERSION} {candidate}\n");

    write_temporary_marker(&temporary_path, payload.as_bytes())?;

    match fs::hard_link(&temporary_path, path) {
        Ok(()) => {
            sync_directory(parent, path)?;
            remove_temporary_marker(&temporary_path, path)?;
            Ok(candidate)
        }
        Err(publish_error) if publish_error.kind() == io::ErrorKind::AlreadyExists => {
            remove_temporary_marker(&temporary_path, path)?;
            read_existing_marker(path, "read concurrently created")
        }
        Err(source) => {
            let cleanup_result = fs::remove_file(&temporary_path);
            if let Err(cleanup_error) = cleanup_result
                && cleanup_error.kind() != io::ErrorKind::NotFound
            {
                return Err(IdentityError::MarkerIo {
                    operation: "clean up temporary marker after publication failure for",
                    path: path.to_path_buf(),
                    source: cleanup_error,
                });
            }
            Err(IdentityError::MarkerIo {
                operation: "publish",
                path: path.to_path_buf(),
                source,
            })
        }
    }
}

fn write_temporary_marker(path: &Path, payload: &[u8]) -> Result<(), IdentityError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| IdentityError::MarkerIo {
            operation: "create temporary",
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(payload)
        .and_then(|()| file.sync_all())
        .map_err(|source| IdentityError::MarkerIo {
            operation: "write and sync temporary",
            path: path.to_path_buf(),
            source,
        })
}

fn remove_temporary_marker(temporary_path: &Path, marker_path: &Path) -> Result<(), IdentityError> {
    fs::remove_file(temporary_path).map_err(|source| IdentityError::MarkerIo {
        operation: "remove temporary file for",
        path: marker_path.to_path_buf(),
        source,
    })
}

fn sync_directory(parent: &Path, marker_path: &Path) -> Result<(), IdentityError> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| IdentityError::MarkerIo {
            operation: "sync parent directory for",
            path: marker_path.to_path_buf(),
            source,
        })
}

fn parse_marker(path: &Path, contents: &str) -> Result<Uuid, IdentityError> {
    let Some(payload) = contents.strip_suffix('\n') else {
        return Err(invalid_marker(path, "marker must end with one newline"));
    };
    if payload.contains('\n') || payload.contains('\r') {
        return Err(invalid_marker(path, "marker must contain exactly one line"));
    }
    let Some(value) = payload.strip_prefix("v1 ") else {
        return Err(invalid_marker(path, "expected `v1 <uuid>`"));
    };
    let uuid = Uuid::parse_str(value)
        .map_err(|source| invalid_marker(path, format!("invalid UUID: {source}")))?;
    if uuid.get_version_num() != 4 {
        return Err(invalid_marker(
            path,
            "persisted marker UUID must be version 4",
        ));
    }
    Ok(uuid)
}

#[allow(clippy::disallowed_methods)]
fn read_existing_marker(path: &Path, operation: &'static str) -> Result<Uuid, IdentityError> {
    let contents = fs::read(path).map_err(|source| IdentityError::MarkerIo {
        operation,
        path: path.to_path_buf(),
        source,
    })?;
    parse_marker_bytes(path, contents)
}

fn parse_marker_bytes(path: &Path, contents: Vec<u8>) -> Result<Uuid, IdentityError> {
    let contents = String::from_utf8(contents)
        .map_err(|source| invalid_marker(path, format!("marker is not UTF-8: {source}")))?;
    parse_marker(path, &contents)
}

fn invalid_marker(path: &Path, reason: impl Into<String>) -> IdentityError {
    IdentityError::InvalidMarker {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::TempDir;

    const GIT_LOCAL_ENV_VARS: &[&str] = &[
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
    ];

    struct GitFixture {
        _temp: TempDir,
        root: PathBuf,
    }

    impl GitFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("create temporary directory");
            let root = temp.path().join("repo");
            fs::create_dir(&root).expect("create repository directory");
            git(&root, &["init", "-b", "main"]);
            git(&root, &["config", "user.name", "locald tests"]);
            git(&root, &["config", "user.email", "locald@example.test"]);
            fs::write(root.join("locald.toml"), "[project]\nname = \"test\"\n")
                .expect("write project config");
            git(&root, &["add", "locald.toml"]);
            git(&root, &["commit", "-m", "initial"]);
            Self { _temp: temp, root }
        }

        fn add_worktree(&self, name: &str) -> PathBuf {
            let path = self._temp.path().join(name);
            git(
                &self.root,
                &["worktree", "add", "-b", name, path_str(&path)],
            );
            path
        }
    }

    fn git(current_dir: &Path, arguments: &[&str]) {
        let mut command = Command::new("git");
        command.args(arguments).current_dir(current_dir);

        // Git exports repository-local environment to hooks. The full test suite
        // runs from the pre-push hook, so fixture commands must not inherit the
        // enclosing locald checkout as their repository.
        for variable in GIT_LOCAL_ENV_VARS {
            command.env_remove(variable);
        }

        let output = command.output().expect("run git");
        assert!(
            output.status.success(),
            "git {} failed:\nstdout: {}\nstderr: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn path_str(path: &Path) -> &str {
        path.to_str().expect("test path is UTF-8")
    }

    #[test]
    fn repeated_discovery_preserves_every_identity() {
        let fixture = GitFixture::new();

        let first = resolve_git_project_identity(&fixture.root).expect("first identity");
        let second = resolve_git_project_identity(&fixture.root).expect("second identity");

        assert_eq!(first.identity, second.identity);
        assert_eq!(first.repository_relative_project_root, Path::new(""));
    }

    #[test]
    fn concurrent_first_discovery_converges_without_partial_markers() {
        let fixture = GitFixture::new();
        let root = Arc::new(fixture.root.clone());
        let barrier = Arc::new(Barrier::new(12));
        let mut handles = Vec::new();

        for _ in 0..12 {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                resolve_git_project_identity(&root).expect("concurrent identity")
            }));
        }

        let identities: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("identity thread").identity)
            .collect();
        assert!(identities.windows(2).all(|pair| pair[0] == pair[1]));

        let resolved = resolve_git_project_identity(&fixture.root).expect("final identity");
        for marker in [
            resolved
                .common_git_dir
                .join(MARKER_DIRECTORY)
                .join(REPOSITORY_MARKER),
            resolved
                .worktree_git_dir
                .join(MARKER_DIRECTORY)
                .join(WORKTREE_MARKER),
        ] {
            let contents = fs::read_to_string(&marker).expect("read marker");
            parse_marker(&marker, &contents).expect("complete marker");
            let parent = marker.parent().expect("marker parent");
            assert!(
                fs::read_dir(parent)
                    .expect("read marker directory")
                    .all(|entry| !entry
                        .expect("directory entry")
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".tmp"))
            );
        }
    }

    #[test]
    fn linked_worktrees_share_repository_and_project_but_not_instance() {
        let fixture = GitFixture::new();
        let linked = fixture.add_worktree("linked");

        let primary = resolve_git_project_identity(&fixture.root).expect("primary identity");
        let linked = resolve_git_project_identity(&linked).expect("linked identity");

        assert_eq!(
            primary.identity.repository_id,
            linked.identity.repository_id
        );
        assert_eq!(primary.identity.project_id, linked.identity.project_id);
        assert_ne!(primary.identity.worktree_id, linked.identity.worktree_id);
        assert_ne!(
            primary.identity.project_instance_id,
            linked.identity.project_instance_id
        );
    }

    #[test]
    fn derivation_format_matches_fixed_compatibility_vectors() {
        let repository_id = RepositoryId(
            Uuid::parse_str("11111111-1111-4111-8111-111111111111")
                .expect("repository vector UUID"),
        );
        let worktree_id = WorktreeId(
            Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("worktree vector UUID"),
        );

        for (relative_root, expected_project, expected_instance) in [
            (
                Path::new(""),
                "3ac8a4a9-14cb-5723-9501-e9574325d94a",
                "9d24a2e2-b011-5bf4-ace9-07053e37ecaf",
            ),
            (
                Path::new("packages/app"),
                "00bb1da8-510e-52d5-8fe3-6f66e96b7b28",
                "889b5a0c-9dc9-5f96-9166-f00e07796a9a",
            ),
        ] {
            let project_id = derive_project_id(repository_id, relative_root, relative_root)
                .expect("derive project vector");
            let instance_id = derive_project_instance_id(worktree_id, project_id);

            assert_eq!(project_id.to_string(), expected_project);
            assert_eq!(instance_id.to_string(), expected_instance);
        }
    }

    #[test]
    fn nested_project_roots_have_distinct_logical_ids() {
        let fixture = GitFixture::new();
        let nested = fixture.root.join("packages/app");
        fs::create_dir_all(&nested).expect("create nested project");
        fs::write(nested.join("locald.toml"), "[project]\nname = \"app\"\n")
            .expect("write nested config");

        let root = resolve_git_project_identity(&fixture.root).expect("root identity");
        let nested = resolve_git_project_identity(&nested).expect("nested identity");

        assert_eq!(root.identity.repository_id, nested.identity.repository_id);
        assert_eq!(root.identity.worktree_id, nested.identity.worktree_id);
        assert_ne!(root.identity.project_id, nested.identity.project_id);
        assert_ne!(
            root.identity.project_instance_id,
            nested.identity.project_instance_id
        );
        assert_eq!(
            nested.repository_relative_project_root,
            Path::new("packages/app")
        );
    }

    #[test]
    fn branch_switch_and_detached_head_preserve_identity() {
        let fixture = GitFixture::new();
        let initial = resolve_git_project_identity(&fixture.root)
            .expect("initial identity")
            .identity;

        git(&fixture.root, &["switch", "-c", "feature"]);
        let branch = resolve_git_project_identity(&fixture.root)
            .expect("branch identity")
            .identity;
        git(&fixture.root, &["switch", "--detach"]);
        let detached = resolve_git_project_identity(&fixture.root)
            .expect("detached identity")
            .identity;

        assert_eq!(initial, branch);
        assert_eq!(initial, detached);
    }

    #[test]
    fn git_worktree_move_preserves_identity() {
        let fixture = GitFixture::new();
        let linked = fixture.add_worktree("movable");
        let initial = resolve_git_project_identity(&linked)
            .expect("initial linked identity")
            .identity;
        let moved = fixture._temp.path().join("moved");

        git(
            &fixture.root,
            &["worktree", "move", path_str(&linked), path_str(&moved)],
        );
        let after_move = resolve_git_project_identity(&moved)
            .expect("moved linked identity")
            .identity;

        assert_eq!(initial, after_move);
    }

    #[test]
    fn moving_whole_clone_preserves_identity() {
        let fixture = GitFixture::new();
        let initial = resolve_git_project_identity(&fixture.root)
            .expect("initial identity")
            .identity;
        let moved = fixture._temp.path().join("moved-repo");
        fs::rename(&fixture.root, &moved).expect("move repository");

        let after_move = resolve_git_project_identity(&moved)
            .expect("moved identity")
            .identity;

        assert_eq!(initial, after_move);
    }

    #[test]
    fn recreating_linked_worktree_creates_a_new_instance() {
        let fixture = GitFixture::new();
        let linked = fixture.add_worktree("recreated");
        let initial = resolve_git_project_identity(&linked)
            .expect("initial linked identity")
            .identity;

        git(
            &fixture.root,
            &["worktree", "remove", "--force", path_str(&linked)],
        );
        git(
            &fixture.root,
            &["worktree", "add", path_str(&linked), "recreated"],
        );
        let recreated = resolve_git_project_identity(&linked)
            .expect("recreated linked identity")
            .identity;

        assert_eq!(initial.repository_id, recreated.repository_id);
        assert_eq!(initial.project_id, recreated.project_id);
        assert_ne!(initial.worktree_id, recreated.worktree_id);
        assert_ne!(initial.project_instance_id, recreated.project_instance_id);
    }

    #[test]
    fn malformed_marker_fails_closed_without_replacement() {
        let fixture = GitFixture::new();
        let resolved = resolve_git_project_identity(&fixture.root).expect("initial identity");
        let marker = resolved
            .common_git_dir
            .join(MARKER_DIRECTORY)
            .join(REPOSITORY_MARKER);
        let malformed = "v1 definitely-not-a-uuid\n";
        fs::write(&marker, malformed).expect("corrupt marker");

        let error = resolve_git_project_identity(&fixture.root).expect_err("invalid marker");

        assert!(matches!(error, IdentityError::InvalidMarker { .. }));
        assert_eq!(
            fs::read_to_string(marker).expect("read corruption"),
            malformed
        );
    }

    #[test]
    fn manually_moved_linked_worktree_recommends_repair() {
        let fixture = GitFixture::new();
        let linked = fixture.add_worktree("broken");
        let moved = fixture._temp.path().join("broken-moved");
        fs::rename(&linked, &moved).expect("move without Git");

        let error = resolve_git_project_identity(&moved).expect_err("broken worktree");
        let message = error.to_string();

        assert!(matches!(error, IdentityError::UnrepairedWorktree { .. }));
        assert!(message.contains("git worktree repair"));
    }

    #[test]
    fn broken_nearer_git_locator_never_binds_an_enclosing_repository() {
        let fixture = GitFixture::new();
        let nested = fixture.root.join("nested");
        fs::create_dir_all(nested.join(".git")).expect("create broken nested Git locator");

        let error = resolve_git_project_identity(&nested).expect_err("broken nested repository");

        assert!(matches!(error, IdentityError::BrokenWorktree { .. }));
        assert!(!fixture.root.join(".git/locald").exists());
    }

    #[test]
    fn non_git_projects_are_deferred_without_writing_state() {
        let temp = tempfile::tempdir().expect("create non-Git directory");

        let error = resolve_git_project_identity(temp.path()).expect_err("non-Git outcome");

        assert!(matches!(error, IdentityError::NotGit { .. }));
        assert!(!temp.path().join(MARKER_DIRECTORY).exists());
    }
}
