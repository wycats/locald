use anyhow::Result;
use std::path::PathBuf;

pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn require(bin: &str) -> Result<PathBuf> {
    which(bin).ok_or_else(|| anyhow::anyhow!("missing required command in PATH: {bin}"))
}
