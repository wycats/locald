use locald_core::config::GlobalConfig;
use std::path::PathBuf;

pub fn global_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("locald/config.toml"))
}

pub fn load() -> GlobalConfig {
    global_config_path()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}
