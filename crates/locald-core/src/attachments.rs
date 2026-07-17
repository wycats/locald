use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const LEGACY_EDITOR_TTL: Duration = Duration::from_mins(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub enum AttachmentSource {
    Editor {
        name: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
    },
    CLI {
        pid: u32,
    },
    /// Parked quiet-up hold retained for the availability-store migration.
    ///
    /// It is preserved as legacy evidence and does not count as a current live
    /// attachment in this compatibility model.
    Runtime,
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
    /// Full service details (ports, URLs, health, etc.)
    #[serde(default)]
    pub service_details: Vec<crate::ipc::ServiceStatus>,
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

#[derive(Debug, Default, Clone)]
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
        crate::storage::data_dir().join("attachments.json")
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

    pub fn attach(&mut self, mut attachment: Attachment) -> Result<bool> {
        if matches!(attachment.source, AttachmentSource::Runtime) {
            anyhow::bail!(
                "Runtime attachment evidence is loaded from legacy state and cannot be created"
            );
        }
        let path = Self::canonicalize_path(&attachment.project_path);
        attachment.project_path.clone_from(&path);

        let entry = self.attachments.entry(path).or_default();
        let is_first = entry
            .iter()
            .all(|existing| matches!(existing.source, AttachmentSource::Runtime));

        entry.retain(|existing| !Self::matches_source(&attachment.source, &existing.source));
        entry.push(attachment);

        Ok(is_first)
    }

    pub fn detach(&mut self, project_path: &Path, source: &AttachmentSource) -> bool {
        if matches!(source, AttachmentSource::Runtime) {
            return false;
        }
        let path = Self::canonicalize_path(project_path);
        let Some(entry) = self.attachments.get_mut(&path) else {
            return false;
        };

        let live_before = Self::has_live_attachment(entry);
        entry.retain(|existing| !Self::matches_source(source, &existing.source));
        let live_after = Self::has_live_attachment(entry);

        if entry.is_empty() {
            self.attachments.remove(&path);
        }
        live_before && !live_after
    }

    pub fn detach_all_non_pin(&mut self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        let Some(entry) = self.attachments.get_mut(&path) else {
            return false;
        };

        let live_before = Self::has_live_attachment(entry);
        entry.retain(|attachment| {
            matches!(
                attachment.source,
                AttachmentSource::Pin | AttachmentSource::Runtime
            )
        });
        let live_after = Self::has_live_attachment(entry);

        if entry.is_empty() {
            self.attachments.remove(&path);
        }
        live_before && !live_after
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

    /// Remove every compatibility attachment and manual-stop marker owned by a
    /// project that is being explicitly removed.
    pub fn forget_project(&mut self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        let removed_attachments = self.attachments.remove(&path).is_some();
        let removed_stop = self.manually_stopped.remove(&path);
        removed_attachments || removed_stop
    }

    pub fn reap_stale_attachments(&mut self) -> Vec<PathBuf> {
        let mut emptied = Vec::new();
        let now = SystemTime::now();

        let mut to_remove = Vec::new();
        for (path, attachments) in &mut self.attachments {
            let live_before = Self::has_live_attachment(attachments);
            attachments.retain(|attachment| Self::attachment_alive(attachment, now));
            let live_after = Self::has_live_attachment(attachments);

            if attachments.is_empty() {
                to_remove.push(path.clone());
            }
            if live_before && !live_after {
                emptied.push(path.clone());
            }
        }

        for path in to_remove {
            self.attachments.remove(&path);
        }

        emptied
    }

    pub fn reap_stale_attachments_for(&mut self, project_path: &Path) -> bool {
        let path = Self::canonicalize_path(project_path);
        let Some(attachments) = self.attachments.get_mut(&path) else {
            return false;
        };

        let now = SystemTime::now();
        let live_before = Self::has_live_attachment(attachments);
        attachments.retain(|attachment| Self::attachment_alive(attachment, now));
        let live_after = Self::has_live_attachment(attachments);

        if attachments.is_empty() {
            self.attachments.remove(&path);
        }
        live_before && !live_after
    }

    pub fn reap_stale_pids(&mut self) -> Vec<PathBuf> {
        self.reap_stale_attachments()
    }

    pub fn section_for(&self, project_path: &Path) -> ProjectSection {
        let path = Self::canonicalize_path(project_path);
        let Some(attachments) = self.attachments.get(&path) else {
            return ProjectSection::Recent;
        };

        if attachments.is_empty() {
            return ProjectSection::Recent;
        }

        let has_pin = attachments
            .iter()
            .any(|a| matches!(a.source, AttachmentSource::Pin));
        let has_active = attachments
            .iter()
            .any(|a| !matches!(a.source, AttachmentSource::Pin | AttachmentSource::Runtime));

        // Active takes priority: if there are non-Pin attachments, it's Active
        // even if also pinned.
        if has_active {
            ProjectSection::Active
        } else if has_pin {
            ProjectSection::AlwaysOn
        } else {
            ProjectSection::Recent
        }
    }

    fn canonicalize_path(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn matches_source(needle: &AttachmentSource, existing: &AttachmentSource) -> bool {
        match (needle, existing) {
            (
                AttachmentSource::Editor { name, id, .. },
                AttachmentSource::Editor {
                    name: existing_name,
                    id: existing_id,
                    ..
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

    fn has_live_attachment(attachments: &[Attachment]) -> bool {
        attachments
            .iter()
            .any(|attachment| !matches!(attachment.source, AttachmentSource::Runtime))
    }

    fn attachment_alive(attachment: &Attachment, now: SystemTime) -> bool {
        match attachment.source {
            AttachmentSource::CLI { pid } | AttachmentSource::Editor { pid: Some(pid), .. } => {
                Self::pid_alive(pid)
            }
            AttachmentSource::Editor { pid: None, .. } => now
                .duration_since(attachment.created_at)
                .map_or(true, |age| age <= LEGACY_EDITOR_TTL),
            AttachmentSource::Runtime | AttachmentSource::Pin => true,
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

        let first = store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        assert!(first);
        assert_eq!(store.attachments_for(&project).len(), 1);

        let second = store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: 1234 },
                created_at: SystemTime::now(),
            })
            .expect("attach CLI");
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

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: 42 },
                created_at: SystemTime::now(),
            })
            .expect("attach CLI");
        assert_eq!(store.section_for(&project), ProjectSection::Active);

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        // Active takes priority: pinned + CLI attachment = Active.
        assert_eq!(store.section_for(&project), ProjectSection::Active);

        // Remove the CLI attachment — now only Pin remains = AlwaysOn.
        store.detach(&project, &AttachmentSource::CLI { pid: 42 });
        assert_eq!(store.section_for(&project), ProjectSection::AlwaysOn);
    }

    #[test]
    fn detach_all_non_pin_keeps_pin() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            })
            .expect("attach pin");
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: 42 },
                created_at: SystemTime::now(),
            })
            .expect("attach CLI");

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

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: alive_pid },
                created_at: SystemTime::now(),
            })
            .expect("attach live CLI");
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: u32::MAX },
                created_at: SystemTime::now(),
            })
            .expect("attach dead CLI");

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

    #[test]
    fn reap_stale_attachments_prunes_legacy_old_editor() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: None,
                },
                created_at: SystemTime::now() - Duration::from_secs(31 * 60),
            })
            .expect("attach old editor");

        let removed = store.reap_stale_attachments();
        let canonical = std::fs::canonicalize(&project).unwrap_or(project.clone());
        assert_eq!(removed, vec![canonical]);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn reap_stale_attachments_keeps_legacy_fresh_editor() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: None,
                },
                created_at: SystemTime::now(),
            })
            .expect("attach fresh editor");

        let removed = store.reap_stale_attachments();
        assert!(removed.is_empty());
        assert_eq!(store.attachments_for(&project).len(), 1);
    }

    #[test]
    fn reap_stale_attachments_prunes_dead_editor_pid() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: Some(u32::MAX),
                },
                created_at: SystemTime::now(),
            })
            .expect("attach dead editor");

        let removed = store.reap_stale_attachments();
        let canonical = std::fs::canonicalize(&project).unwrap_or(project.clone());
        assert_eq!(removed, vec![canonical]);
        assert!(store.attachments_for(&project).is_empty());
    }

    #[test]
    fn reap_stale_attachments_keeps_pin() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now() - Duration::from_secs(31 * 60),
            })
            .expect("attach pin");

        let removed = store.reap_stale_attachments();
        assert!(removed.is_empty());
        assert_eq!(store.attachments_for(&project).len(), 1);
    }

    #[test]
    fn detach_editor_matching_ignores_pid() {
        let dir = tempdir().unwrap();
        let mut store = AttachmentStore::new(dir.path().join("attachments.json"));
        let project = dir.path().join("project");

        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Editor {
                    name: "vscode".to_string(),
                    id: "abc".to_string(),
                    pid: Some(1234),
                },
                created_at: SystemTime::now(),
            })
            .expect("attach editor");

        let last_removed = store.detach(
            &project,
            &AttachmentSource::Editor {
                name: String::new(),
                id: "abc".to_string(),
                pid: None,
            },
        );

        assert!(last_removed);
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

    #[tokio::test]
    async fn parked_runtime_hold_survives_load_and_save_without_blocking_attach() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let project_text = project.to_string_lossy();
        let legacy = serde_json::json!({
            "attachments": {
                project_text.as_ref(): [{
                    "project_path": project,
                    "source": "Runtime",
                    "created_at": {
                        "secs_since_epoch": 1,
                        "nanos_since_epoch": 0
                    }
                }]
            },
            "manually_stopped": []
        });
        tokio::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy attachment"),
        )
        .await
        .expect("write legacy attachment store");

        let mut store = AttachmentStore::new(store_path.clone());
        store.load().await.expect("load legacy runtime hold");
        assert_eq!(store.section_for(&project), ProjectSection::Recent);
        assert!(matches!(
            store.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));
        assert!(
            store
                .attach(Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::CLI {
                        pid: std::process::id(),
                    },
                    created_at: SystemTime::now(),
                })
                .expect("attach CLI beside parked Runtime evidence")
        );
        store.save().await.expect("save compatible store");

        let mut reloaded = AttachmentStore::new(store_path);
        reloaded.load().await.expect("reload compatible store");
        let sources: Vec<_> = reloaded
            .attachments_for(&project)
            .into_iter()
            .map(|attachment| &attachment.source)
            .collect();
        assert!(
            sources
                .iter()
                .any(|source| matches!(source, AttachmentSource::Runtime))
        );
        assert!(sources.iter().any(|source| matches!(
            source,
            AttachmentSource::CLI { pid } if *pid == std::process::id()
        )));

        assert!(reloaded.detach(
            &project,
            &AttachmentSource::CLI {
                pid: std::process::id(),
            },
        ));
        assert_eq!(reloaded.attachments_for(&project).len(), 1);
        assert!(matches!(
            reloaded.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));

        assert!(
            reloaded
                .attach(Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::CLI {
                        pid: std::process::id(),
                    },
                    created_at: SystemTime::now(),
                })
                .expect("reattach CLI beside parked Runtime evidence")
        );
        assert!(reloaded.detach_all_non_pin(&project));
        assert!(matches!(
            reloaded.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));

        let error = reloaded
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::Runtime,
                created_at: SystemTime::now(),
            })
            .expect_err("live Runtime attachments are migration-owned");
        assert!(error.to_string().contains("cannot be created"));

        assert!(
            reloaded
                .attach(Attachment {
                    project_path: project.clone(),
                    source: AttachmentSource::Pin,
                    created_at: SystemTime::now(),
                })
                .expect("attach pin beside parked Runtime evidence")
        );
        reloaded.mark_stopped(&project);
        assert!(reloaded.forget_project(&project));
        assert!(reloaded.attachments_for(&project).is_empty());
        assert!(!reloaded.is_stopped(&project));
        assert!(!reloaded.all_projects().contains(&project));
    }

    #[tokio::test]
    async fn parked_runtime_evidence_does_not_mask_a_reaped_last_owner() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("attachments.json");
        let project = dir.path().join("project");
        let project_text = project.to_string_lossy();
        let legacy = serde_json::json!({
            "attachments": {
                project_text.as_ref(): [{
                    "project_path": project,
                    "source": "Runtime",
                    "created_at": {
                        "secs_since_epoch": 1,
                        "nanos_since_epoch": 0
                    }
                }]
            },
            "manually_stopped": []
        });
        tokio::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy attachment"),
        )
        .await
        .expect("write legacy attachment store");

        let mut store = AttachmentStore::new(store_path);
        store.load().await.expect("load legacy Runtime evidence");
        store
            .attach(Attachment {
                project_path: project.clone(),
                source: AttachmentSource::CLI { pid: u32::MAX },
                created_at: SystemTime::now(),
            })
            .expect("attach dead CLI");

        let orphaned = store.reap_stale_attachments();

        assert_eq!(orphaned, vec![project.clone()]);
        assert_eq!(store.attachments_for(&project).len(), 1);
        assert!(matches!(
            store.attachments_for(&project)[0].source,
            AttachmentSource::Runtime
        ));
    }

    #[test]
    fn serialization_round_trip() {
        let dir = tempdir().unwrap();
        let attachment = Attachment {
            project_path: dir.path().join("project"),
            source: AttachmentSource::Editor {
                name: "vscode".to_string(),
                id: "abc".to_string(),
                pid: Some(std::process::id()),
            },
            created_at: SystemTime::now(),
        };

        let json = serde_json::to_string(&attachment).unwrap();
        let decoded: Attachment = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.project_path, attachment.project_path);
        assert_eq!(decoded.source, attachment.source);
    }

    #[test]
    fn legacy_editor_json_deserializes_without_pid() {
        let json = r#"
{
    "project_path": "/tmp/project",
    "source": {
        "Editor": {
            "name": "vscode",
            "id": "abc"
        }
    },
    "created_at": {
        "secs_since_epoch": 1,
        "nanos_since_epoch": 0
    }
}
"#;

        let decoded: Attachment = serde_json::from_str(json).unwrap();

        assert_eq!(
            decoded.source,
            AttachmentSource::Editor {
                name: "vscode".to_string(),
                id: "abc".to_string(),
                pid: None,
            }
        );
    }
}
