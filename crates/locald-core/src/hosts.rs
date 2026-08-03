#![allow(clippy::collapsible_if)]
use std::io;
use std::path::PathBuf;
use tokio::fs;

/// Historical macOS platform aliases retained for explicit migrations.
///
/// Canonical platform domains use `.localhost`. Daemon synchronization omits
/// these spellings when they are platform fallbacks, while preserving an
/// identically named service-owned claim.
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

    pub fn update_content(
        &self,
        current_content: &str,
        domains: &[String],
    ) -> Result<String, locald_hosts::HostsContentError> {
        locald_hosts::update_hosts_content(current_content, domains)
    }

    /// Rebuild locald's generated section after removing the named domains
    /// from its IPv4 loopback mappings.
    pub fn remove_domains_from_content(
        &self,
        current_content: &str,
        domains: &[&str],
    ) -> Result<String, locald_hosts::HostsContentError> {
        self.remove_matching_domains_from_content(current_content, |domain| {
            domains.contains(&domain)
        })
    }

    /// Rebuild locald's generated section after removing IPv4 loopback
    /// mappings that macOS resolves natively.
    ///
    /// Historical `.local` spellings are deliberately retained here because
    /// the hosts file does not reveal whether a name is a stale platform alias
    /// or an active service-owned claim. The daemon's complete domain index is
    /// the authority for retiring those names.
    pub fn remove_native_macos_domains_from_content(
        &self,
        current_content: &str,
    ) -> Result<String, locald_hosts::HostsContentError> {
        self.remove_matching_domains_from_content(current_content, |domain| {
            let canonical = domain.to_ascii_lowercase();
            canonical == "localhost" || canonical.ends_with(".localhost")
        })
    }

    fn remove_matching_domains_from_content(
        &self,
        current_content: &str,
        mut should_remove: impl FnMut(&str) -> bool,
    ) -> Result<String, locald_hosts::HostsContentError> {
        let managed = locald_hosts::managed_host_set(current_content)
            .map_err(locald_hosts::HostsContentError::MalformedSection)?;
        let retained = managed
            .iter()
            .filter(|domain| !should_remove(domain))
            .map(str::to_owned)
            .collect::<Vec<_>>();

        if retained.len() == managed.len() {
            Ok(current_content.to_owned())
        } else {
            self.update_content(current_content, &retained)
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

        let new_content = hosts.update_content(content, &domains).unwrap();

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

        let new_content = hosts.update_content(content, &domains).unwrap();

        assert!(new_content.contains("127.0.0.1 new.local"));
        assert!(!new_content.contains("127.0.0.1 old.local"));
        assert_eq!(new_content.matches("# BEGIN locald").count(), 1);
    }

    #[test]
    fn empty_domains_remove_an_existing_section_without_creating_one() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 old.local\n# END locald\n";

        let updated = hosts.update_content(content, &[]).unwrap();
        assert_eq!(updated, "127.0.0.1 localhost\n");
        assert_eq!(hosts.update_content(&updated, &[]).unwrap(), updated);
    }

    #[test]
    fn removing_legacy_domains_preserves_active_custom_mappings() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 locald.local custom.example.test\n# END locald\n";

        let updated = hosts
            .remove_domains_from_content(content, LEGACY_MACOS_HOST_ALIASES)
            .unwrap();
        assert!(!updated.contains("locald.local"));
        assert!(updated.contains("127.0.0.1 custom.example.test"));
        assert!(updated.contains("# BEGIN locald"));
    }

    #[test]
    fn removing_the_only_legacy_domains_retires_the_generated_section() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 locald.local\n# END locald\n";

        let updated = hosts
            .remove_domains_from_content(content, LEGACY_MACOS_HOST_ALIASES)
            .unwrap();
        assert_eq!(updated, "127.0.0.1 localhost\n");
    }

    #[test]
    fn removing_native_macos_domains_preserves_custom_mappings() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 locald.local app.localhost frame.turn.v0.localhost custom.example.test\n# END locald\n";

        let updated = hosts
            .remove_native_macos_domains_from_content(content)
            .unwrap();
        assert!(updated.contains("127.0.0.1 locald.local"));
        assert!(!updated.contains("app.localhost"));
        assert!(!updated.contains("frame.turn.v0.localhost"));
        assert!(updated.contains("127.0.0.1 custom.example.test"));
        assert!(updated.contains("# BEGIN locald"));
    }

    #[test]
    fn removing_only_native_macos_domains_retires_the_generated_section() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 app.localhost frame.turn.v0.localhost\n# END locald\n";

        let updated = hosts
            .remove_native_macos_domains_from_content(content)
            .unwrap();
        assert_eq!(updated, "127.0.0.1 localhost\n");
    }

    #[test]
    fn malformed_managed_section_is_reported_instead_of_rewritten() {
        let hosts = HostsFileSection::with_path(PathBuf::from("/tmp/hosts"));
        let malformed = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 app.localhost\n";

        assert!(hosts.update_content(malformed, &[]).is_err());
        assert!(
            hosts
                .remove_native_macos_domains_from_content(malformed)
                .is_err()
        );
    }
}
