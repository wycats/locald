use anyhow::{Context, Result};
use directories::ProjectDirs;
use locald_core::state::ServerState;
use std::path::PathBuf;
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};

#[derive(Debug)]
pub struct StateManager {
    state_path: PathBuf,
    write_lock: Mutex<()>,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        let dirs = ProjectDirs::from("com", "locald", "locald")
            .context("Could not determine project directories")?;
        let data_dir = dirs.data_dir();
        let state_path = data_dir.join("state.json");

        info!("State file configured at: {:?}", state_path);

        Ok(Self {
            state_path,
            write_lock: Mutex::new(()),
        })
    }

    #[must_use]
    pub fn with_path(state_path: PathBuf) -> Self {
        Self {
            state_path,
            write_lock: Mutex::new(()),
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

        if !self.state_path.exists() {
            debug!("No state file found, returning default state");
            return Ok(ServerState::default());
        }

        debug!("Loading state from {:?}", self.state_path);
        let content = fs::read_to_string(&self.state_path)
            .await
            .context("Failed to read state file")?;

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
        let content = serde_json::to_string_pretty(state)?;
        let tmp_path = self.state_path.with_extension("tmp");

        fs::write(&tmp_path, content)
            .await
            .context("Failed to write temp state file")?;

        fs::rename(&tmp_path, &self.state_path)
            .await
            .context("Failed to rename state file")?;

        debug!("State saved successfully");
        Ok(())
    }
}
