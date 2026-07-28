//! Stable Git worktree domain-slug hints.

use crate::domain::sanitize_dns_label;

/// Extract the last segment of a branch name (after the last `/`).
///
/// `feature/checkout-flow` → `checkout-flow`
/// `hotfix` → `hotfix`
pub fn branch_last_segment(branch: &str) -> &str {
    branch.rsplit('/').next().unwrap_or(branch)
}

/// Convert a mutable task, branch, or path label into an optional DNS-label
/// allocation hint. Labels containing no ASCII letters or digits are skipped
/// so allocation can try the next source.
#[must_use]
pub fn sanitize_slug_hint(value: &str) -> Option<String> {
    value
        .chars()
        .any(|character| character.is_ascii_alphanumeric())
        .then(|| sanitize_dns_label(value, "worktree"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_hints_are_sanitized_to_one_dns_label() {
        assert_eq!(
            sanitize_slug_hint("Feature/JIRA-123_foo"),
            Some("feature-jira-123-foo".to_owned())
        );
        assert_eq!(sanitize_slug_hint("---"), None);
        let long = "a".repeat(100);
        assert_eq!(sanitize_slug_hint(&long).expect("valid hint").len(), 63);
    }

    #[test]
    fn branch_hint_uses_the_final_segment() {
        assert_eq!(
            branch_last_segment("feature/checkout-flow"),
            "checkout-flow"
        );
        assert_eq!(branch_last_segment("hotfix"), "hotfix");
        assert_eq!(branch_last_segment("a/b/c"), "c");
    }
}
