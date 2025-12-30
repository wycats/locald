use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    #[serde(default)]
    pub projects: HashMap<PathBuf, ProjectEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "SystemTime::now")]
    pub last_seen: SystemTime,
}

impl Registry {
    /// Load the registry from disk.
    pub async fn load() -> anyhow::Result<Self> {
        Self::load_from_path(Self::path()).await
    }

    /// Load the registry from a specific path.
    ///
    /// # Example
    /// ```rust
    /// use locald_core::registry::Registry;
    /// use std::path::PathBuf;
    ///
    /// # async fn run() {
    /// let registry = Registry::load_from_path(PathBuf::from("/tmp/registry.json")).await.unwrap();
    /// # }
    /// ```
    pub async fn load_from_path(path: PathBuf) -> anyhow::Result<Self> {
        if path.exists() {
            let content = tokio::fs::read_to_string(&path).await?;
            if content.trim().is_empty() {
                return Ok(Self::default());
            }
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    /// Save the registry to disk.
    pub async fn save(&self) -> anyhow::Result<()> {
        Self::save_to_path(self, Self::path()).await
    }

    /// Save the registry to a specific path.
    pub async fn save_to_path(&self, path: PathBuf) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    /// Get the path to the registry file.
    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("com", "locald", "locald").map_or_else(
            || PathBuf::from("locald-registry.json"),
            |dirs| dirs.data_local_dir().join("registry.json"),
        )
    }

    /// Register a project in the registry.
    ///
    /// Updates the last seen timestamp and name if provided.
    pub fn register_project(&mut self, path: &Path, name: Option<String>) {
        let path = Self::canonicalize_path(path);

        let entry = self
            .projects
            .entry(path.clone())
            .or_insert_with(|| ProjectEntry {
                path: path.clone(),
                name: name.clone(),
                pinned: false,
                last_seen: SystemTime::now(),
            });
        entry.last_seen = SystemTime::now();
        if name.is_some() {
            entry.name = name;
        }
    }

    /// Get a project entry by path.
    pub fn get_project(&self, path: &Path) -> Option<&ProjectEntry> {
        let path = Self::canonicalize_path(path);
        self.projects.get(&path)
    }

    /// Pin a project in the registry.
    ///
    /// Pinned projects are not removed during cleanup.
    pub fn pin_project(&mut self, path: &Path) -> bool {
        let path = Self::canonicalize_path(path);
        if let Some(entry) = self.projects.get_mut(&path) {
            entry.pinned = true;
            true
        } else {
            false
        }
    }

    /// Unpin a project from the registry.
    pub fn unpin_project(&mut self, path: &Path) -> bool {
        let path = Self::canonicalize_path(path);
        if let Some(entry) = self.projects.get_mut(&path) {
            entry.pinned = false;
            true
        } else {
            false
        }
    }

    /// Remove projects that no longer exist on disk.
    ///
    /// Returns the number of removed projects.
    pub fn prune_missing_projects(&mut self) -> usize {
        let before = self.projects.len();
        self.projects.retain(|path, _| path.exists());
        before - self.projects.len()
    }

    fn canonicalize_path(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_registry_persistence() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("registry.json");

        // 1. Create and populate registry
        let mut registry = Registry::default();
        let project_path = dir.path().join("my-project");
        std::fs::create_dir(&project_path).unwrap();

        registry.register_project(&project_path, Some("test-project".to_string()));
        registry.save_to_path(file_path.clone()).await.unwrap();

        // 2. Load from disk
        let loaded = Registry::load_from_path(file_path).await.unwrap();

        // 3. Verify
        let entry = loaded
            .get_project(&project_path)
            .expect("Project should exist");
        assert_eq!(entry.name.as_deref(), Some("test-project"));
        assert!(!entry.pinned);
    }

    #[tokio::test]
    async fn test_prune_missing() {
        let dir = tempdir().unwrap();
        let mut registry = Registry::default();

        let existing = dir.path().join("exists");
        let missing = dir.path().join("missing");

        std::fs::create_dir(&existing).unwrap();

        registry.register_project(&existing, None);
        registry.register_project(&missing, None);

        assert_eq!(registry.projects.len(), 2);

        let removed = registry.prune_missing_projects();
        assert_eq!(removed, 1);
        assert_eq!(registry.projects.len(), 1);
        assert!(registry.get_project(&existing).is_some());
    }
}
