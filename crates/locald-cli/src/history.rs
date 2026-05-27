use crate::error::{CliError, CliResult};
use anyhow::Context;
use std::io::{BufRead, Write};
use std::path::PathBuf;

fn get_history_path() -> CliResult<PathBuf> {
    let data_dir = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        PathBuf::from(home).join(".local/share")
    };

    Ok(data_dir.join("locald/history"))
}

pub fn append(command: &str) -> CliResult<()> {
    let path = get_history_path()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    writeln!(file, "{}", command.trim())?;
    Ok(())
}

pub fn get_last() -> CliResult<String> {
    let path = get_history_path()?;

    if !path.exists() {
        return Err(CliError::message("No history found."));
    }

    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);

    // Read all lines and get the last one
    // This is inefficient for huge files but fine for command history
    let last_line = reader
        .lines()
        .collect::<Result<Vec<_>, _>>()?
        .last()
        .cloned()
        .context("History file is empty")?;

    Ok(last_line)
}
