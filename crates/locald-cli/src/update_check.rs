//! Background update check with rate limiting.
//!
//! This module provides an opt-in update check that runs in the background
//! when `locald up` is executed. The check is rate-limited to once per 24 hours
//! and persists state to avoid redundant network requests.

use anyhow::Result;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
const STATE_FILE: &str = "locald/update-state.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub last_check_timestamp: Option<u64>,
}

pub fn state_file_path() -> Option<PathBuf> {
    BaseDirs::new().map(|base| base.data_local_dir().join(STATE_FILE))
}

pub fn load_state() -> UpdateState {
    let Some(path) = state_file_path() else {
        return UpdateState::default();
    };

    fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Save state atomically using write-to-temp + rename pattern.
pub fn save_state(state: &UpdateState) -> Result<()> {
    let Some(path) = state_file_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload = serde_json::to_string(state)?;

    // Write to temp file then atomically rename
    let temp_path = path.with_extension("json.tmp");
    let mut file = fs::File::create(&temp_path)?;
    file.write_all(payload.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temp_path, &path)?;

    Ok(())
}

pub fn should_check(state: &UpdateState) -> bool {
    let Some(last_check) = state.last_check_timestamp else {
        return true;
    };

    let now = current_timestamp();
    now.saturating_sub(last_check) > CHECK_INTERVAL_SECS
}

/// Spawn a background thread to check for updates.
///
/// This is intentionally fire-and-forget: the spawned thread is detached and
/// will not block program termination. The callback is invoked with the result
/// once the check completes (or `None` if skipped due to rate limiting).
///
/// The timestamp is only updated on successful check, so transient failures
/// (e.g., network issues) will retry on the next run.
pub fn spawn_update_check(callback: impl FnOnce(Option<String>) + Send + 'static) {
    std::thread::spawn(move || {
        let state = load_state();

        if !should_check(&state) {
            callback(None);
            return;
        }

        // Perform the check - only update timestamp on success
        match crate::selfupgrade::check() {
            Ok(result) => {
                // Successful check (even if no update available) - update timestamp
                let new_state = UpdateState {
                    last_check_timestamp: Some(current_timestamp()),
                };
                let _ = save_state(&new_state);
                callback(result);
            }
            Err(_) => {
                // Network or other error - don't update timestamp, retry next time
                callback(None);
            }
        }
    });
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_check_when_no_timestamp() {
        let state = UpdateState::default();
        assert!(should_check(&state));
    }

    #[test]
    fn should_not_check_when_recent() {
        let now = current_timestamp();
        let state = UpdateState {
            last_check_timestamp: Some(now.saturating_sub(60 * 60)),
        };
        assert!(!should_check(&state));
    }

    #[test]
    fn should_check_when_old() {
        let now = current_timestamp();
        let state = UpdateState {
            last_check_timestamp: Some(now.saturating_sub(25 * 60 * 60)),
        };
        assert!(should_check(&state));
    }

    #[test]
    fn state_round_trip() {
        let state = UpdateState {
            last_check_timestamp: Some(1_700_000_000),
        };
        let payload = serde_json::to_string(&state).expect("serialize update state");
        let decoded: UpdateState =
            serde_json::from_str(&payload).expect("deserialize update state");
        assert_eq!(state, decoded);
    }
}
