use cliclack::{ProgressBar, spinner};
use locald_core::ipc::BootEvent;
use std::collections::HashMap;

pub struct ProgressRenderer {
    active_spinner: Option<(String, ProgressBar)>,
    logs: HashMap<String, Vec<String>>,
}

impl ProgressRenderer {
    pub fn new() -> Self {
        Self {
            active_spinner: None,
            logs: HashMap::new(),
        }
    }

    pub fn handle_event(&mut self, event: BootEvent) {
        match event {
            BootEvent::StepStarted { id, description } => {
                // If there's an existing spinner, stop it? Or is this a nested step?
                // Manager is sequential. So if we get a new start, the old one should be done.
                // But just in case:
                if let Some((old_id, s)) = self.active_spinner.take() {
                    s.stop(format!("{} (interrupted)", old_id));
                }

                let s = spinner();
                s.start(description);
                self.active_spinner = Some((id.clone(), s));
                self.logs.insert(id, Vec::new());
            }
            BootEvent::StepProgress { id, message } => {
                if let Some((current_id, s)) = &self.active_spinner {
                    if current_id == &id {
                        s.start(message); // Update message
                    }
                }
            }
            BootEvent::StepFinished { id, result } => {
                if let Some((current_id, s)) = self.active_spinner.take() {
                    if current_id == id {
                        match result {
                            Ok(()) => {
                                s.stop(format!("{} registered", id));
                            }
                            Err(e) => {
                                s.error(format!("{} failed: {}", id, e));
                            }
                        }
                    } else {
                        // Mismatch?
                        // Maybe we just log it
                        match result {
                            Ok(()) => cliclack::log::info(format!("{} registered", id)).ok(),
                            Err(e) => cliclack::log::error(format!("{} failed: {}", id, e)).ok(),
                        };
                        // Put the spinner back
                        self.active_spinner = Some((current_id, s));
                    }
                } else {
                    // No active spinner, just log
                    match result {
                        Ok(()) => cliclack::log::info(format!("{} registered", id)).ok(),
                        Err(e) => cliclack::log::error(format!("{} failed: {}", id, e)).ok(),
                    };
                }
                self.logs.remove(&id);
            }
            BootEvent::Log {
                id,
                line,
                stream: _,
            } => {
                // If we have an active spinner, we can't easily print "above" it without breaking it in some terminals.
                // cliclack doesn't support "log while spinner is active" easily without clearing/redrawing.
                // But `cliclack::log` might handle it?
                // Let's try just printing.
                // Or we can accumulate logs and only show on error?
                // The user wants to see logs.

                // For now, let's just print.
                // If it messes up the spinner, we might need a better strategy.
                // But `locald up` usually doesn't show logs unless verbose?
                // The `manager` broadcasts logs.

                // If we are in a spinner, we can try to use `s.println` if it existed.
                // `cliclack` doesn't expose the underlying console term easily.

                // Use println! directly to avoid cliclack's spacer lines for logs
                // We need to clear the current line if a spinner is active, but we don't have access to it easily.
                // However, since we are in verbose mode, maybe we don't care about the spinner artifacts as much as the log readability.
                // Or we can try to use `\r` to overwrite?

                // Actually, if we just print, it might be fine.
                println!("[{}] {}", id, line.trim_end());
            }
        }
    }
}
