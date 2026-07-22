#![allow(clippy::collapsible_if)]
use std::io;
use std::path::PathBuf;
use tokio::fs;

/// Historical macOS hosts-file aliases removed during setup and ignored by
/// daemon synchronization. Canonical locald domains use `.localhost`.
pub const LEGACY_MACOS_HOST_ALIASES: &[&str] = &[
    "dev.docs.local",
    "dev.locald.local",
    "docs.local",
    "locald.local",
];

#[derive(Debug)]
pub struct HostsFileSection {
    path: PathBuf,
}

impl Default for HostsFileSection {
    fn default() -> Self {
        Self::new()
    }
}

impl HostsFileSection {
    pub fn new() -> Self {
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
        } else {
            PathBuf::from("/etc/hosts")
        };
        Self { path }
    }

    pub const fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub async fn read(&self) -> io::Result<String> {
        fs::read_to_string(&self.path).await
    }

    pub fn update_content(&self, current_content: &str, domains: &[String]) -> String {
        locald_hosts::update_hosts_content(current_content, domains)
    }

    /// Rebuild locald's generated section after removing the named domains
    /// from its IPv4 loopback mappings.
    pub fn remove_domains_from_content(&self, current_content: &str, domains: &[&str]) -> String {
        self.remove_matching_domains_from_content(current_content, |domain| {
            domains.contains(&domain)
        })
    }

    /// Rebuild locald's generated section after removing IPv4 loopback
    /// mappings that macOS resolves natively or that locald has retired.
    pub fn remove_native_macos_domains_from_content(&self, current_content: &str) -> String {
        self.remove_matching_domains_from_content(current_content, |domain| {
            let canonical = domain.to_ascii_lowercase();
            canonical == "localhost"
                || canonical.ends_with(".localhost")
                || LEGACY_MACOS_HOST_ALIASES.contains(&canonical.as_str())
        })
    }

    fn remove_matching_domains_from_content(
        &self,
        current_content: &str,
        mut should_remove: impl FnMut(&str) -> bool,
    ) -> String {
        let start_marker = "# BEGIN locald";
        let end_marker = "# END locald";
        let Some(start) = current_content.find(start_marker) else {
            return current_content.to_owned();
        };
        let content_start = start + start_marker.len();
        let Some(relative_end) = current_content[content_start..].find(end_marker) else {
            return current_content.to_owned();
        };
        let content_end = content_start + relative_end;

        let mut removed = false;
        let mut retained = Vec::new();
        for line in current_content[content_start..content_end].lines() {
            let mut fields = line.split_whitespace();
            if fields.next() != Some("127.0.0.1") {
                continue;
            }
            for domain in fields {
                if should_remove(domain) {
                    removed = true;
                } else {
                    retained.push(domain.to_owned());
                }
            }
        }

        if removed {
            self.update_content(current_content, &retained)
        } else {
            current_content.to_owned()
        }
    }

    pub async fn write(&self, content: &str) -> io::Result<()> {
        fs::write(&self.path, content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_new_section() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n";
        let domains = vec!["app.local".to_string(), "api.local".to_string()];

        let new_content = hosts.update_content(content, &domains);

        assert!(new_content.contains("# BEGIN locald"));
        assert!(new_content.contains("127.0.0.1 app.local"));
        assert!(new_content.contains("127.0.0.1 api.local"));
        assert!(new_content.contains("# END locald"));
        assert!(new_content.starts_with("127.0.0.1 localhost\n"));
    }

    #[test]
    fn test_replace_existing_section() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 old.local\n# END locald\n";
        let domains = vec!["new.local".to_string()];

        let new_content = hosts.update_content(content, &domains);

        assert!(new_content.contains("127.0.0.1 new.local"));
        assert!(!new_content.contains("127.0.0.1 old.local"));
        assert_eq!(new_content.matches("# BEGIN locald").count(), 1);
    }

    #[test]
    fn empty_domains_remove_an_existing_section_without_creating_one() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 old.local\n# END locald\n";

        let updated = hosts.update_content(content, &[]);
        assert_eq!(updated, "127.0.0.1 localhost\n\n");
        assert_eq!(hosts.update_content(&updated, &[]), updated);
    }

    #[test]
    fn removing_legacy_domains_preserves_active_custom_mappings() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 locald.local custom.example.test\n# END locald\n";

        let updated = hosts.remove_domains_from_content(content, LEGACY_MACOS_HOST_ALIASES);
        assert!(!updated.contains("locald.local"));
        assert!(updated.contains("127.0.0.1 custom.example.test"));
        assert!(updated.contains("# BEGIN locald"));
    }

    #[test]
    fn removing_the_only_legacy_domains_retires_the_generated_section() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 locald.local\n# END locald\n";

        let updated = hosts.remove_domains_from_content(content, LEGACY_MACOS_HOST_ALIASES);
        assert_eq!(updated, "127.0.0.1 localhost\n\n");
    }

    #[test]
    fn removing_native_macos_domains_preserves_custom_mappings() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 locald.local app.localhost frame.turn.v0.localhost custom.example.test\n# END locald\n";

        let updated = hosts.remove_native_macos_domains_from_content(content);
        assert!(!updated.contains("locald.local"));
        assert!(!updated.contains("app.localhost"));
        assert!(!updated.contains("frame.turn.v0.localhost"));
        assert!(updated.contains("127.0.0.1 custom.example.test"));
        assert!(updated.contains("# BEGIN locald"));
    }

    #[test]
    fn removing_only_native_macos_domains_retires_the_generated_section() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 app.localhost frame.turn.v0.localhost\n# END locald\n";

        let updated = hosts.remove_native_macos_domains_from_content(content);
        assert_eq!(updated, "127.0.0.1 localhost\n\n");
    }
}
