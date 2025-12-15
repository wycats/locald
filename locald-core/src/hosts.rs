#![allow(clippy::collapsible_if)]
use std::fmt::Write;
use std::io;
use std::path::PathBuf;
use tokio::fs;

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
        let start_marker = "# BEGIN locald";
        let end_marker = "# END locald";

        let mut new_section = String::new();
        new_section.push_str(start_marker);
        new_section.push('\n');
        for domain in domains {
            let _ = writeln!(new_section, "127.0.0.1 {domain}");
        }
        new_section.push_str(end_marker);

        if let Some(start) = current_content.find(start_marker) {
            if let Some(end_idx) = current_content[start..].find(end_marker) {
                let end = start + end_idx;
                // Replace existing section
                let mut output = String::from(&current_content[..start]);
                output.push_str(&new_section);
                output.push_str(&current_content[end + end_marker.len()..]);
                return output;
            }
        }

        // Append if not found
        let mut output = String::from(current_content);
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&new_section);
        output.push('\n');
        output
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
}
