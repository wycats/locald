use anyhow::{Context, Result};
use locald_core::state::ServerState;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};
use uuid::Uuid;

#[derive(Debug, Error)]
pub(crate) enum StateError {
    #[error("runtime state `{path}` was published and its parent-directory sync failed: {reason}")]
    PublishedNotDurable { path: PathBuf, reason: String },
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateSaveFault {
    TemporaryFileSync,
    ParentDirectorySync,
}

#[derive(Debug)]
pub struct StateManager {
    state_path: PathBuf,
    write_lock: Mutex<()>,
    #[cfg(test)]
    next_save_fault: Mutex<Option<StateSaveFault>>,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        let state_path = locald_core::storage::data_dir().join("state.json");

        info!("State file configured at: {:?}", state_path);

        Ok(Self {
            state_path,
            write_lock: Mutex::new(()),
            #[cfg(test)]
            next_save_fault: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn with_path(state_path: PathBuf) -> Self {
        Self {
            state_path,
            write_lock: Mutex::new(()),
            #[cfg(test)]
            next_save_fault: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.state_path
    }

    #[cfg(test)]
    pub(crate) async fn inject_save_fault(&self, fault: StateSaveFault) {
        *self.next_save_fault.lock().await = Some(fault);
    }

    #[cfg(test)]
    async fn take_save_fault(&self, fault: StateSaveFault) -> bool {
        let mut next = self.next_save_fault.lock().await;
        if *next == Some(fault) {
            *next = None;
            true
        } else {
            false
        }
    }

    async fn ensure_dir(&self) -> Result<()> {
        if let Some(parent) = self.state_path.parent()
            && !parent.exists()
        {
            debug!("Creating state directory: {:?}", parent);
            fs::create_dir_all(parent)
                .await
                .context("Failed to create state directory")?;
        }
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn load(&self) -> Result<ServerState> {
        self.ensure_dir().await?;

        debug!("Loading state from {:?}", self.state_path);
        let content = match fs::read_to_string(&self.state_path).await {
            Ok(content) => content,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(&self.state_path).await {
                    Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => {
                        debug!("No state file found, returning default state");
                        return Ok(ServerState::default());
                    }
                    Ok(_) => return Err(source).context("Failed to read state file"),
                    Err(metadata_error) => {
                        return Err(metadata_error).context("Failed to inspect state file");
                    }
                }
            }
            Err(source) => return Err(source).context("Failed to read state file"),
        };

        let state: ServerState =
            serde_json::from_str(&content).context("Failed to parse state file")?;

        info!("Loaded state with {} services", state.services.len());
        Ok(state)
    }

    #[instrument(skip(self, state))]
    pub async fn save(&self, state: &ServerState) -> Result<()> {
        self.ensure_dir().await?;
        let _guard = self.write_lock.lock().await;

        debug!("Saving state with {} services", state.services.len());
        let temporary = self.write_temporary(state).await?;

        if let Err(source) = fs::rename(&temporary, &self.state_path).await {
            let source = cleanup_reason(source, &temporary).await;
            return Err(source).with_context(|| {
                format!(
                    "Failed to atomically replace state file {}",
                    self.state_path.display()
                )
            });
        }

        let parent_sync = async {
            #[cfg(test)]
            if self
                .take_save_fault(StateSaveFault::ParentDirectorySync)
                .await
            {
                anyhow::bail!("injected parent-directory sync failure");
            }
            sync_parent(&self.state_path).await
        }
        .await;
        if let Err(error) = parent_sync {
            return Err(StateError::PublishedNotDurable {
                path: self.state_path.clone(),
                reason: error.to_string(),
            }
            .into());
        }

        debug!("State saved successfully");
        Ok(())
    }

    async fn write_temporary(&self, state: &ServerState) -> Result<PathBuf> {
        let parent = self
            .state_path
            .parent()
            .with_context(|| format!("State path {} has no parent", self.state_path.display()))?;
        let file_name = self
            .state_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state.json");
        let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let mut content = serde_json::to_vec_pretty(state).context("Failed to serialize state")?;
        content.push(b'\n');

        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await
            .with_context(|| {
                format!(
                    "Failed to create temporary state file {}",
                    temporary.display()
                )
            })?;
        let write_result = async {
            output.write_all(&content).await?;
            #[cfg(test)]
            if self
                .take_save_fault(StateSaveFault::TemporaryFileSync)
                .await
            {
                return Err(io::Error::other("injected temporary-file sync failure"));
            }
            output.sync_all().await
        }
        .await;
        drop(output);
        if let Err(source) = write_result {
            let source = cleanup_reason(source, &temporary).await;
            return Err(source).with_context(|| {
                format!(
                    "Failed to write and sync temporary state file {}",
                    temporary.display()
                )
            });
        }

        Ok(temporary)
    }
}

async fn cleanup_reason(source: io::Error, temporary: &Path) -> io::Error {
    match fs::remove_file(temporary).await {
        Ok(()) => source,
        Err(cleanup_error) => io::Error::new(
            source.kind(),
            format!("{source}; temporary cleanup also failed: {cleanup_error}"),
        ),
    }
}

async fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("State path {} has no parent", path.display()))?;
    let directory = fs::File::open(parent).await.with_context(|| {
        format!(
            "Failed to open state directory {} for sync",
            parent.display()
        )
    })?;
    directory
        .sync_all()
        .await
        .with_context(|| format!("Failed to sync state directory {}", parent.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn temporary_files(path: &Path) -> Vec<PathBuf> {
        let parent = path.parent().expect("runtime state parent");
        std::fs::read_dir(parent)
            .expect("read runtime state directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(".state.json.")
                            && Path::new(name)
                                .extension()
                                .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
                    })
            })
            .collect()
    }

    #[tokio::test]
    async fn temporary_file_sync_failure_does_not_publish_runtime_state() {
        let directory = tempdir().expect("create temporary directory");
        let path = directory.path().join("state.json");
        let manager = StateManager::with_path(path.clone());
        manager
            .inject_save_fault(StateSaveFault::TemporaryFileSync)
            .await;

        let error = manager
            .save(&ServerState::default())
            .await
            .expect_err("temporary-file sync failure must fail before publication");

        assert!(error.to_string().contains("Failed to write and sync"));
        assert!(!path.exists());
        assert!(temporary_files(&path).is_empty());
    }

    #[tokio::test]
    async fn parent_sync_failure_reports_published_but_uncertain_runtime_state() {
        let directory = tempdir().expect("create temporary directory");
        let path = directory.path().join("state.json");
        let manager = StateManager::with_path(path.clone());
        manager
            .inject_save_fault(StateSaveFault::ParentDirectorySync)
            .await;

        let error = manager
            .save(&ServerState::default())
            .await
            .expect_err("parent-directory sync failure must not claim durability");

        assert!(matches!(
            error.downcast_ref::<StateError>(),
            Some(StateError::PublishedNotDurable { .. })
        ));
        let published = manager
            .load()
            .await
            .expect("atomically published runtime state remains visible");
        assert!(published.services.is_empty());
        assert!(temporary_files(&path).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_runtime_state_entry_is_not_treated_as_absent() {
        let directory = tempdir().expect("create temporary directory");
        let path = directory.path().join("state.json");
        std::os::unix::fs::symlink(directory.path().join("missing-state"), &path)
            .expect("create dangling runtime state symlink");
        let manager = StateManager::with_path(path.clone());

        manager
            .load()
            .await
            .expect_err("dangling runtime state must block loading");
        assert!(
            fs::symlink_metadata(&path)
                .await
                .expect("inspect preserved runtime state")
                .file_type()
                .is_symlink()
        );
    }
}
