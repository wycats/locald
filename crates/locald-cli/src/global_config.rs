use directories::BaseDirs;
use locald_core::config::GlobalConfig;
use std::path::PathBuf;

pub fn global_config_path() -> Option<PathBuf> {
    BaseDirs::new().map(|base| base.config_dir().join("locald/config.toml"))
}

pub fn load() -> GlobalConfig {
    global_config_path()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Save the global config to disk, creating the directory if needed.
#[allow(dead_code)]
pub fn save(config: GlobalConfig) -> anyhow::Result<()> {
    let path = global_config_path()
        .ok_or_else(|| anyhow::anyhow!("Could not determine global config path"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(&config)?;
    std::fs::write(&path, contents)?;
    Ok(())
}
