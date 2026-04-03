//! Git worktree detection and branch-qualified domain resolution.

use std::path::Path;

/// Git context for a project path.
#[derive(Debug, Clone)]
pub struct GitContext {
    /// Whether this path is inside a git worktree (vs the main repo).
    pub is_worktree: bool,
    /// The current branch name (e.g., "main", "feature/checkout").
    pub branch: Option<String>,
    /// Whether this is the default branch.
    pub is_default_branch: bool,
}

/// Detect git context for a project path.
///
/// Returns `None` if the path is not inside a git repository.
pub fn detect(path: &Path) -> Option<GitContext> {
    let repo = git2::Repository::open(path).ok()?;

    let is_worktree = repo.is_worktree();

    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from));

    let is_default_branch = branch
        .as_deref()
        .is_some_and(|b| check_is_default(b, &repo));

    Some(GitContext {
        is_worktree,
        branch,
        is_default_branch,
    })
}

/// Check if a branch name is the repository's default branch.
///
/// Tries `refs/remotes/origin/HEAD` first, falls back to common names.
fn check_is_default(branch: &str, repo: &git2::Repository) -> bool {
    if let Ok(reference) = repo.find_reference("refs/remotes/origin/HEAD")
        && let Ok(resolved) = reference.resolve()
        && let Some(name) = resolved.shorthand()
    {
        let remote_branch = name.rsplit('/').next().unwrap_or(name);
        return branch == remote_branch;
    }

    matches!(branch, "main" | "master")
}

/// Sanitize a branch name for use as a DNS label.
///
/// Lowercase, replace `[^a-z0-9-]` with `-`, collapse consecutive hyphens,
/// trim leading/trailing hyphens, truncate to 63 characters.
pub fn sanitize_branch_for_dns(branch: &str) -> String {
    let mut result = String::with_capacity(branch.len());

    for ch in branch.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            result.push(ch);
        } else {
            result.push('-');
        }
    }

    // Collapse consecutive hyphens.
    while result.contains("--") {
        result = result.replace("--", "-");
    }

    let result = result.trim_matches('-').to_string();

    if result.len() > 63 {
        result[..63].trim_end_matches('-').to_string()
    } else {
        result
    }
}

/// Extract the last segment of a branch name (after the last `/`).
///
/// `feature/checkout-flow` → `checkout-flow`
/// `hotfix` → `hotfix`
pub fn branch_last_segment(branch: &str) -> &str {
    branch.rsplit('/').next().unwrap_or(branch)
}

/// Resolve a worktree domain template.
///
/// Supported variables:
/// - `{{name}}` — project name
/// - `{{branch.last}}` — last segment of branch, DNS-sanitized
/// - `{{branch.hyphenated}}` — full branch with `/` → `-`, DNS-sanitized
/// - `{{project.domain}}` — the resolved project domain
pub fn resolve_domain_template(
    template: &str,
    name: &str,
    branch: &str,
    project_domain: &str,
) -> String {
    let branch_last = sanitize_branch_for_dns(branch_last_segment(branch));
    let branch_hyphenated = sanitize_branch_for_dns(branch);

    template
        .replace("{{name}}", name)
        .replace("{{branch.last}}", &branch_last)
        .replace("{{branch.hyphenated}}", &branch_hyphenated)
        .replace("{{project.domain}}", project_domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_simple() {
        assert_eq!(sanitize_branch_for_dns("main"), "main");
    }

    #[test]
    fn sanitize_slash() {
        assert_eq!(
            sanitize_branch_for_dns("feature/checkout"),
            "feature-checkout"
        );
    }

    #[test]
    fn sanitize_complex() {
        assert_eq!(
            sanitize_branch_for_dns("feature/JIRA-123_foo"),
            "feature-jira-123-foo"
        );
    }

    #[test]
    fn sanitize_leading_trailing() {
        assert_eq!(sanitize_branch_for_dns("-foo-"), "foo");
    }

    #[test]
    fn sanitize_consecutive_hyphens() {
        assert_eq!(sanitize_branch_for_dns("a//b"), "a-b");
    }

    #[test]
    fn sanitize_truncate() {
        let long = "a".repeat(100);
        let result = sanitize_branch_for_dns(&long);
        assert!(result.len() <= 63);
    }

    #[test]
    fn last_segment_with_slash() {
        assert_eq!(
            branch_last_segment("feature/checkout-flow"),
            "checkout-flow"
        );
    }

    #[test]
    fn last_segment_no_slash() {
        assert_eq!(branch_last_segment("hotfix"), "hotfix");
    }

    #[test]
    fn last_segment_nested() {
        assert_eq!(branch_last_segment("a/b/c"), "c");
    }

    #[test]
    fn template_branch_last() {
        assert_eq!(
            resolve_domain_template(
                "{{branch.last}}.{{project.domain}}",
                "myapp",
                "feature/checkout",
                "myapp.localhost"
            ),
            "checkout.myapp.localhost"
        );
    }

    #[test]
    fn template_branch_hyphenated() {
        assert_eq!(
            resolve_domain_template(
                "{{branch.hyphenated}}.{{project.domain}}",
                "myapp",
                "feature/checkout",
                "myapp.localhost"
            ),
            "feature-checkout.myapp.localhost"
        );
    }

    #[test]
    fn template_with_name() {
        assert_eq!(
            resolve_domain_template(
                "{{branch.last}}.{{name}}.localhost",
                "myapp",
                "feature/checkout",
                "myapp.localhost"
            ),
            "checkout.myapp.localhost"
        );
    }
}
