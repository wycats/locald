//! Trusted ambient workspace resolution for agent adapters.

#![allow(clippy::redundant_pub_crate)] // Shared with the sibling IPC module.

use anyhow::{Context, Result};
use ignore::{DirEntry, WalkBuilder};
use locald_core::AgentWorkspaceContext;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use thiserror::Error;

const CONFIG_FILES: [&str; 2] = ["locald.toml", "Procfile"];
const EXCLUDED_DIRECTORIES: [&str; 6] =
    [".git", "node_modules", "target", "dist", "build", ".next"];
const MAX_DISCOVERY_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentWorkspaceResolutionKind {
    Ambiguous,
    ConflictingSources,
    MissingProject,
    InspectionFailure,
}

/// A private ambient-resolution diagnostic with model-safe recovery guidance.
#[derive(Debug, Error)]
#[error("{details}")]
pub(crate) struct AgentWorkspaceResolutionError {
    kind: AgentWorkspaceResolutionKind,
    details: String,
}

impl AgentWorkspaceResolutionError {
    fn new(kind: AgentWorkspaceResolutionKind, details: impl Into<String>) -> Self {
        Self {
            kind,
            details: details.into(),
        }
    }

    /// Return actionable guidance without exposing private workspace paths.
    pub(crate) const fn safe_message(&self) -> &'static str {
        match self.kind {
            AgentWorkspaceResolutionKind::Ambiguous => {
                "ambient workspace contains multiple locald projects; narrow the task workspace to one project"
            }
            AgentWorkspaceResolutionKind::ConflictingSources => {
                "trusted workspace sources identify different locald projects; reopen the task with one consistent workspace"
            }
            AgentWorkspaceResolutionKind::MissingProject => {
                "ambient workspace does not contain `locald.toml` or `Procfile`; open the task inside one locald project"
            }
            AgentWorkspaceResolutionKind::InspectionFailure => {
                "locald could not inspect the ambient workspace; verify that it exists and is readable"
            }
        }
    }
}

/// Resolve trusted MCP/Codex context without accepting a model-provided path.
pub(crate) async fn resolve_agent_workspace(context: &AgentWorkspaceContext) -> Result<PathBuf> {
    context.validate().map_err(|error| {
        AgentWorkspaceResolutionError::new(
            AgentWorkspaceResolutionKind::InspectionFailure,
            error.to_string(),
        )
    })?;
    let context = context.clone();
    tokio::task::spawn_blocking(move || resolve_agent_workspace_blocking(&context))
        .await
        .map_err(|error| {
            AgentWorkspaceResolutionError::new(
                AgentWorkspaceResolutionKind::InspectionFailure,
                format!("ambient agent workspace resolution task failed: {error}"),
            )
        })?
}

fn resolve_agent_workspace_blocking(context: &AgentWorkspaceContext) -> Result<PathBuf> {
    let roots = resolve_locator_group("MCP workspace roots", &context.workspace_roots)?;
    let sandbox = context
        .sandbox_cwd
        .as_deref()
        .map(|path| resolve_locator_group("Codex sandbox workspace", &[path.to_path_buf()]))
        .transpose()?
        .flatten();

    if let Some(project_root) = roots {
        if let Some(sandbox_root) = sandbox
            && sandbox_root != project_root
        {
            return Err(AgentWorkspaceResolutionError::new(
                AgentWorkspaceResolutionKind::ConflictingSources,
                format!(
                    "trusted MCP workspace roots resolve to `{}` while Codex sandbox metadata resolves to `{}`; refusing to choose between conflicting worktrees",
                    project_root.display(),
                    sandbox_root.display()
                ),
            )
            .into());
        }
        return Ok(project_root);
    }

    if let Some(project_root) = sandbox {
        return Ok(project_root);
    }

    if let Some(process_cwd) = context.process_cwd.as_deref()
        && let Some(project_root) = resolve_locator_group(
            "agent adapter working directory",
            &[process_cwd.to_path_buf()],
        )?
    {
        return Ok(project_root);
    }

    Err(AgentWorkspaceResolutionError::new(
        AgentWorkspaceResolutionKind::MissingProject,
        "ambient agent context does not resolve to a locald project",
    )
    .into())
}

fn resolve_locator_group(label: &str, locators: &[PathBuf]) -> Result<Option<PathBuf>> {
    let mut projects = BTreeSet::new();
    for locator in locators {
        let discovered = projects_for_locator(locator).map_err(|error| {
            AgentWorkspaceResolutionError::new(
                AgentWorkspaceResolutionKind::InspectionFailure,
                format!(
                    "failed to inspect {label} locator `{}`: {error:#}",
                    locator.display()
                ),
            )
        })?;
        projects.extend(discovered);
    }
    match projects.len() {
        0 => Ok(None),
        1 => Ok(projects.into_iter().next()),
        _ => {
            let paths = projects
                .iter()
                .map(|path| format!("`{}`", path.display()))
                .collect::<Vec<_>>()
                .join(", ");
            Err(AgentWorkspaceResolutionError::new(
                AgentWorkspaceResolutionKind::Ambiguous,
                format!(
                    "{label} contain multiple locald projects ({paths}); narrow the task workspace to one project"
                ),
            )
            .into())
        }
    }
}

fn projects_for_locator(locator: &Path) -> Result<BTreeSet<PathBuf>> {
    let locator = std::fs::canonicalize(locator)
        .with_context(|| format!("could not canonicalize `{}`", locator.display()))?;
    let start = if locator.is_file() {
        locator
            .parent()
            .context("workspace locator file has no parent directory")?
            .to_path_buf()
    } else {
        locator
    };

    if let Some(project) = nearest_ancestor_project(&start)? {
        return Ok(BTreeSet::from([project]));
    }

    let mut projects = BTreeSet::new();
    let mut builder = WalkBuilder::new(&start);
    builder
        .follow_links(false)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .filter_entry(include_walk_entry);
    for (index, entry) in builder.build().enumerate() {
        anyhow::ensure!(
            index < MAX_DISCOVERY_ENTRIES,
            "ambient workspace discovery exceeded {MAX_DISCOVERY_ENTRIES} filesystem entries under `{}`; narrow the task workspace",
            start.display()
        );
        let entry = entry.with_context(|| {
            format!(
                "failed while discovering nested locald projects under `{}`",
                start.display()
            )
        })?;
        if entry.file_type().is_some_and(|kind| kind.is_file())
            && is_project_config_name(entry.file_name())
        {
            let parent = entry
                .path()
                .parent()
                .context("locald configuration has no parent directory")?;
            projects.insert(std::fs::canonicalize(parent).with_context(|| {
                format!("could not canonicalize project root `{}`", parent.display())
            })?);
        }
    }
    Ok(projects)
}

fn nearest_ancestor_project(start: &Path) -> Result<Option<PathBuf>> {
    for ancestor in start.ancestors() {
        for config_name in CONFIG_FILES {
            let config = ancestor.join(config_name);
            match std::fs::symlink_metadata(&config) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    return std::fs::canonicalize(ancestor).map(Some).with_context(|| {
                        format!(
                            "could not canonicalize project root `{}`",
                            ancestor.display()
                        )
                    });
                }
                Ok(_) => {
                    anyhow::bail!(
                        "locald configuration `{}` is not a regular file",
                        config.display()
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "could not inspect locald configuration `{}`",
                            config.display()
                        )
                    });
                }
            }
        }
    }
    Ok(None)
}

fn is_project_config_name(name: &OsStr) -> bool {
    CONFIG_FILES.iter().any(|config| name == *config)
}

fn include_walk_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    !entry.file_type().is_some_and(|kind| kind.is_dir())
        || !EXCLUDED_DIRECTORIES
            .iter()
            .any(|excluded| entry.file_name() == *excluded)
}

#[cfg(test)]
mod tests {
    use super::{AgentWorkspaceResolutionError, resolve_agent_workspace};
    use locald_core::{
        AGENT_ADAPTER_PROTOCOL_VERSION, AgentConversationKey, AgentWorkspaceContext,
    };
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn context(
        roots: Vec<PathBuf>,
        sandbox_cwd: Option<PathBuf>,
        process_cwd: Option<PathBuf>,
    ) -> AgentWorkspaceContext {
        AgentWorkspaceContext {
            protocol_version: AGENT_ADAPTER_PROTOCOL_VERSION,
            conversation: AgentConversationKey::digest("test conversation")
                .expect("digest conversation"),
            workspace_roots: roots,
            sandbox_cwd,
            process_cwd,
        }
    }

    fn project(parent: &Path, name: &str) -> PathBuf {
        let project = parent.join(name);
        std::fs::create_dir_all(project.join("src")).expect("create project");
        std::fs::write(project.join("locald.toml"), "[project]\nname = \"test\"\n")
            .expect("write config");
        std::fs::canonicalize(project).expect("canonical project")
    }

    fn procfile_project(parent: &Path, name: &str) -> PathBuf {
        let project = parent.join(name);
        std::fs::create_dir_all(project.join("src")).expect("create Procfile project");
        std::fs::write(project.join("Procfile"), "web: npm start\n").expect("write Procfile");
        std::fs::canonicalize(project).expect("canonical Procfile project")
    }

    #[tokio::test]
    async fn workspace_root_resolves_one_nested_project() {
        let directory = TempDir::new().expect("create temporary workspace");
        let expected = project(directory.path(), "packages/app");
        let resolved =
            resolve_agent_workspace(&context(vec![directory.path().to_path_buf()], None, None))
                .await
                .expect("resolve nested project");
        assert_eq!(resolved, expected);
    }

    #[tokio::test]
    async fn workspace_discovery_does_not_hide_projects_behind_ignore_patterns() {
        let directory = TempDir::new().expect("create temporary workspace");
        std::fs::write(directory.path().join(".gitignore"), "packages/\n")
            .expect("write workspace ignore file");
        let expected = project(directory.path(), "packages/app");

        let resolved =
            resolve_agent_workspace(&context(vec![directory.path().to_path_buf()], None, None))
                .await
                .expect("resolve ignored nested project");

        assert_eq!(resolved, expected);
    }

    #[tokio::test]
    async fn workspace_root_resolves_one_nested_procfile_project() {
        let directory = TempDir::new().expect("create temporary workspace");
        let expected = procfile_project(directory.path(), "packages/procfile-app");

        let resolved =
            resolve_agent_workspace(&context(vec![directory.path().to_path_buf()], None, None))
                .await
                .expect("resolve nested Procfile project");

        assert_eq!(resolved, expected);
    }

    #[tokio::test]
    async fn locator_inside_project_resolves_nearest_ancestor() {
        let directory = TempDir::new().expect("create temporary workspace");
        let expected = project(directory.path(), "app");
        let resolved = resolve_agent_workspace(&context(vec![expected.join("src")], None, None))
            .await
            .expect("resolve ancestor project");
        assert_eq!(resolved, expected);
    }

    #[tokio::test]
    async fn multiple_projects_in_one_workspace_are_rejected() {
        let directory = TempDir::new().expect("create temporary workspace");
        project(directory.path(), "one");
        project(directory.path(), "two");
        let error =
            resolve_agent_workspace(&context(vec![directory.path().to_path_buf()], None, None))
                .await
                .expect_err("ambiguous workspace must fail");
        assert!(error.to_string().contains("multiple locald projects"));
        let safe = error
            .downcast_ref::<AgentWorkspaceResolutionError>()
            .expect("typed workspace-resolution error")
            .safe_message();
        assert!(safe.contains("narrow the task workspace"));
        assert!(!safe.contains(directory.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn conflicting_roots_and_sandbox_metadata_are_rejected() {
        let directory = TempDir::new().expect("create temporary workspace");
        let first = project(directory.path(), "one");
        let second = project(directory.path(), "two");
        let error = resolve_agent_workspace(&context(vec![first], Some(second), None))
            .await
            .expect_err("conflicting trusted sources must fail");
        assert!(error.to_string().contains("conflicting worktrees"));
    }

    #[tokio::test]
    async fn process_cwd_is_used_only_as_fallback() {
        let directory = TempDir::new().expect("create temporary workspace");
        let expected = project(directory.path(), "fallback");
        let resolved = resolve_agent_workspace(&context(Vec::new(), None, Some(expected.clone())))
            .await
            .expect("resolve fallback");
        assert_eq!(resolved, expected);
    }

    #[tokio::test]
    async fn missing_project_guidance_names_both_supported_configurations_without_paths() {
        let directory = TempDir::new().expect("create empty workspace");
        let error =
            resolve_agent_workspace(&context(vec![directory.path().to_path_buf()], None, None))
                .await
                .expect_err("empty workspace must fail");
        let safe = error
            .downcast_ref::<AgentWorkspaceResolutionError>()
            .expect("typed workspace-resolution error")
            .safe_message();

        assert!(safe.contains("`locald.toml`"));
        assert!(safe.contains("`Procfile`"));
        assert!(!safe.contains(directory.path().to_string_lossy().as_ref()));
    }
}
