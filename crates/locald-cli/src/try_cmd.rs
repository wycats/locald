use crate::history;
use anyhow::{Context, Result};
use crossterm::tty::IsTty;
use dialoguer::Confirm;
use std::process::Command;

pub fn run_adhoc(command: String) -> Result<()> {
    if command.trim().is_empty() {
        anyhow::bail!("Empty command");
    }

    // Simple free port finder
    let port = (10000..20000)
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .unwrap_or(3000);

    // Use sh -c to allow shell expansion (e.g. $PORT)
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .env("PORT", port.to_string())
        .spawn()
        .context("Failed to spawn command")?;

    // Handle Ctrl+C to allow graceful exit and prompt
    let _ = ctrlc::set_handler(move || {
        // Do nothing, let the child handle it.
        // We just want to prevent locald from exiting immediately.
    });

    let _status = child.wait()?;

    // Save to history
    if let Err(e) = history::append(&command) {
        eprintln!("Failed to save history: {e}");
    }

    if std::io::stdin().is_tty() {
        if Confirm::new()
            .with_prompt("Do you want to add this command to locald.toml?")
            .default(true)
            .interact()?
        {
            let name: String = dialoguer::Input::new()
                .with_prompt("Service name")
                .default("web".into())
                .interact_text()?;

            // Don't save the ephemeral port, let locald manage it
            crate::service::add_exec(command, Some(name), None)?;
        }
    } else {
        println!("Not a TTY, skipping interactive add prompt.");
    }

    Ok(())
}
