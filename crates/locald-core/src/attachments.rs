use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub enum AttachmentSource {
    Editor { name: String, id: String },
    CLI { pid: u32 },
    Pin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Attachment {
    pub project_path: PathBuf,
    pub source: AttachmentSource,
    pub created_at: SystemTime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub enum ProjectFilter {
    Active,
    Pinned,
    Recent,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ProjectStatusInfo {
    pub project_path: PathBuf,
    pub project_name: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub is_running: bool,
    #[serde(default)]
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ProjectListEntry {
    pub project_path: PathBuf,
    pub project_name: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub is_running: bool,
    pub section: ProjectSection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub enum ProjectSection {
    Active,
    AlwaysOn,
    Recent,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AttachmentStoreData {
    #[serde(default)]
    attachments: HashMap<PathBuf, Vec<Attachment>>,
    #[serde(default)]
    manually_stopped: HashSet<PathBuf>,
}

#[derive(Debug, Default)]
pub struct AttachmentStore {
    path: PathBuf,
    attachments: HashMap<PathBuf, Vec<Attachment>>,
    manually_stopped: HashSet<PathBuf>,
}

impl AttachmentStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            attachments: HashMap::new(),
            manually_stopped: HashSet::new(),
        }
    }

    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("com", "locald", "locald").map_or_else(
            || PathBuf::from("locald-attachments.json"),
            |dirs| dirs.data_local_dir().join("attachments.json"),
        )
    }

    #[allow(clippy::disallowed_methods)]
    pub async fn load(&mut self) -> Result<()> {
        if self.path.exists() {
            let content = tokio::fs::read_to_string(&self.path).await?;
            if content.trim().is_empty() {
                self.attachments.clear();
                self.manually_stopped.clear();
                return Ok(());
            }
            let data: AttachmentStoreData = serde_json::from_str(&content)?;
            self.attachments = data.attachments;
            self.manually_stopped = data.manually_stopped;
        }
        Ok(())
    }

    #[allow(clippy::disallowed_methods)]
    pub async fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let data = AttachmentStoreData {
            attachments: self.attachments.clone(),
            manually_stopped: self.manually_stopped.clone(),
        };
        let content = serde_json::to_string_pretty(&data)?;
        tokio::fs::write(&self.path, content).await?;
        Ok(())
    }

    pub fn attach(&mut self, mut attachment: Attachment) -> bool {
        let path = Self::canonicalize_path(&attachment.project_path);
        attachment.project_path.clone_from(&path);

        let entry = self.attachments.entry(path).or_default();
        let is_first = entry.is_empty();

        entry.retain(|existing| !Self::matches_source(&attachment.source, &existing.source));
        entry.push(attachment);

        is_first
    }

    pub fn detach(&mut self, project_path: &Path, source: &AttachmentSource) -> bool {
        let path = Self::canonicalize_path(project_path);
        let Some(entry) = self.attachments.get_mut(&path) else {
            return false;
        };

        let before = entry.len();
        entry.retain(|existing| !Self::matches_source(source, &existing.source));

        if entry.is_empty() {
            self.attachments.remove(&path);
            return before > 0;
        }

        false
    }

    pub fn detach_all_non_pin(&mut self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        let Some(entry) = self.attachments.get_mut(&path) else {
            return false;
        };

        let before = entry.len();
        entry.retain(|attachment| matches!(attachment.source, AttachmentSource::Pin));

        if entry.is_empty() {
            self.attachments.remove(&path);
            return before > 0;
        }

        false
    }

    pub fn mark_stopped(&mut self, project_path: &Path) {
        let path = Self::canonicalize_path(project_path);
        self.manually_stopped.insert(path);
    }

    pub fn clear_stopped(&mut self, project_path: &Path) {
        let path = Self::canonicalize_path(project_path);
        self.manually_stopped.remove(&path);
    }

    pub fn is_stopped(&self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        self.manually_stopped.contains(&path)
    }

    pub fn attachments_for(&self, project_path: &Path) -> Vec<&Attachment> {
        let path = Self::canonicalize_path(project_path);
        self.attachments
            .get(&path)
            .map_or_else(Vec::new, |attachments| attachments.iter().collect())
    }

    pub fn all_projects(&self) -> Vec<PathBuf> {
        self.attachments.keys().cloned().collect()
    }

    pub fn reap_stale_pids(&mut self) -> Vec<PathBuf> {
        let mut emptied = Vec::new();

        let mut to_remove = Vec::new();
        for (path, attachments) in &mut self.attachments {
            attachments.retain(|attachment| match attachment.source {
                AttachmentSource::CLI { pid } => Self::pid_alive(pid),
                _ => true,
            });

            if attachments.is_empty() {
                to_remove.push(path.clone());
            }
        }

        for path in to_remove {
            self.attachments.remove(&path);
            emptied.push(path);
        }

        emptied
    }

    pub fn section_for(&self, project_path: &Path) -> ProjectSection {
        let path = Self::canonicalize_path(project_path);
        let Some(attachments) = self.attachments.get(&path) else {
            return ProjectSection::Recent;
        };

        if attachments
            .iter()
            .any(|attachment| matches!(attachment.source, AttachmentSource::Pin))
        {
            return ProjectSection::AlwaysOn;
        }

        if attachments.is_empty() {
            ProjectSection::Recent
        } else {
            ProjectSection::Active
        }
    }

    fn canonicalize_path(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn matches_source(needle: &AttachmentSource, existing: &AttachmentSource) -> bool {
        match (needle, existing) {
            (
                AttachmentSource::Editor { name, id },
                AttachmentSource::Editor {
                    name: existing_name,
                    id: existing_id,
                },
            ) => {
                if name.is_empty() {
                    id == existing_id
                } else {
                    name == existing_name && id == existing_id
                }
            }
            _ => needle == existing,
        }
    }

    #[allow(unsafe_code)]
    fn pid_alive(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };

        let result = unsafe { libc::kill(pid, 0) };
        if result == 0 {
            return true;
        }

        let err = std::io::Error::last_os_error();
        err.raw_os_error() != Some(libc::ESRCH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn attach_and_detach_updates_counts() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        let first = store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::Pin,
            created_at: SystemTime::now(),
        });
        assert!(first);
        assert_eq!(store.attachments_for(&project).len(), 1);

        let second = store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::CLI { pid: 1234 },
            created_at: SystemTime::now(),
        });
        assert!(!second);
        assert_eq!(store.attachments_for(&project).len(), 2);

        let last_removed = store.detach(&project, &AttachmentSource::Pin);
        assert!(!last_removed);
        assert_eq!(store.attachments_for(&project).len(), 1);

        let last_removed = store.detach(&project, &AttachmentSource::CLI { pid: 1234 });
        assert!(last_removed);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn section_for_respects_pin_and_activity() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        assert_eq!(store.section_for(&project), ProjectSection::Recent);

        store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::CLI { pid: 42 },
            created_at: SystemTime::now(),
        });
        assert_eq!(store.section_for(&project), ProjectSection::Active);

        store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::Pin,
            created_at: SystemTime::now(),
        });
        assert_eq!(store.section_for(&project), ProjectSection::AlwaysOn);
    }

    #[test]
    fn detach_all_non_pin_keeps_pin() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::Pin,
            created_at: SystemTime::now(),
        });
        store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::CLI { pid: 42 },
            created_at: SystemTime::now(),
        });

        let last_removed = store.detach_all_non_pin(&project);
        assert!(!last_removed);

        let remaining = store.attachments_for(&project);
        assert_eq!(remaining.len(), 1);
        assert!(matches!(remaining[0].source, AttachmentSource::Pin));
    }

    #[test]
    fn reap_stale_pids_prunes_dead_entries() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let alive_pid = child.id();

        store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::CLI { pid: alive_pid },
            created_at: SystemTime::now(),
        });
        store.attach(Attachment {
            project_path: project.clone(),
            source: AttachmentSource::CLI { pid: u32::MAX },
            created_at: SystemTime::now(),
        });

        let removed = store.reap_stale_pids();
        assert!(removed.is_empty());
        assert_eq!(store.attachments_for(&project).len(), 1);

        let _ = child.kill();
        let _ = child.wait();

        let removed = store.reap_stale_pids();
        let canonical = std::fs::canonicalize(&project).unwrap_or(project.clone());
        assert_eq!(removed, vec![canonical]);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[tokio::test]
    async fn manually_stopped_persists() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");

        let mut store = AttachmentStore::new(store_path.clone());
        store.mark_stopped(&project);
        store.save().await.unwrap();

        let mut loaded = AttachmentStore::new(store_path.clone());
        loaded.load().await.unwrap();
        assert!(loaded.is_stopped(&project));

        loaded.clear_stopped(&project);
        loaded.save().await.unwrap();

        let mut reloaded = AttachmentStore::new(store_path);
        reloaded.load().await.unwrap();
        assert!(!reloaded.is_stopped(&project));
    }

    #[test]
    fn serialization_round_trip() {
        let dir = tempdir().unwrap();
        let attachment = Attachment {
            project_path: dir.path().join("project"),
            source: AttachmentSource::Editor {
                name: "vscode".to_string(),
                id: "abc".to_string(),
            },
            created_at: SystemTime::now(),
        };

        let json = serde_json::to_string(&attachment).unwrap();
        let decoded: Attachment = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.project_path, attachment.project_path);
        assert_eq!(decoded.source, attachment.source);
    }
}
