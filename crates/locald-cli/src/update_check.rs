use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
pub const STATE_FILE: &str = "locald/update-state.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub last_check_timestamp: Option<u64>,
}

pub fn state_file_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(STATE_FILE))
}

pub fn load_state() -> UpdateState {
    let Some(path) = state_file_path() else {
        return UpdateState::default();
    };

    let Ok(contents) = std::fs::read_to_string(&path) else {
        return UpdateState::default();
    };

    serde_json::from_str(&contents).unwrap_or_default()
}

pub fn save_state(state: &UpdateState) -> Result<()> {
    let Some(path) = state_file_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let payload = serde_json::to_string(state)?;
    std::fs::write(path, payload)?;
    Ok(())
}

pub fn should_check(state: &UpdateState) -> bool {
    let Some(last_check) = state.last_check_timestamp else {
        return true;
    };

    let now = current_timestamp();
    now.saturating_sub(last_check) > CHECK_INTERVAL_SECS
}

pub fn spawn_update_check(callback: impl FnOnce(Option<String>) + Send + 'static) {
    std::thread::spawn(move || {
        let mut state = load_state();
        let mut result = None;

        if should_check(&state) {
            result = crate::selfupgrade::check().unwrap_or(None);
            state.last_check_timestamp = Some(current_timestamp());
            let _ = save_state(&state);
        }

        callback(result);
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
