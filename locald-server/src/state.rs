use anyhow::{Context, Result};
use directories::ProjectDirs;
use locald_core::state::ServerState;
use std::path::PathBuf;
use tokio::fs;
use tracing::info;

pub struct StateManager {
    state_path: PathBuf,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        let dirs = ProjectDirs::from("com", "locald", "locald")
            .context("Could not determine project directories")?;
        let data_dir = dirs.data_dir();
        
        // Ensure directory exists (sync is fine here as it's startup/rare)
        std::fs::create_dir_all(data_dir)?;
        
        let state_path = data_dir.join("state.json");
        info!("State file location: {:?}", state_path);
        
        Ok(Self { state_path })
    }

    pub async fn load(&self) -> Result<ServerState> {
        if !self.state_path.exists() {
            return Ok(ServerState::default());
        }

        let content = fs::read_to_string(&self.state_path).await?;
        let state: ServerState = serde_json::from_str(&content)
            .context("Failed to parse state file")?;
            
        Ok(state)
    }

    pub async fn save(&self, state: &ServerState) -> Result<()> {
        let content = serde_json::to_string_pretty(state)?;
        fs::write(&self.state_path, content).await?;
        Ok(())
    }
}
