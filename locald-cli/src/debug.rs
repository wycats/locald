use anyhow::{Context, Result};
use listeners::get_all;
use std::os::unix::process::CommandExt;
use std::process::Command;

pub fn check_port(port: u16) -> Result<()> {
    // If we are not root, try to use the shim to see all processes
    #[cfg(unix)]
    if !nix::unistd::geteuid().is_root() {
        // Check if shim exists and is configured
        if let Ok(shim_path) = locald_utils::shim::get_configured_shim() {
            // Exec shim
            let err = Command::new(&shim_path)
                .arg("debug")
                .arg("port")
                .arg(port.to_string())
                .exec();

            // If exec fails, fall through to normal execution
            eprintln!("Failed to exec shim: {err}");
        }
    }

    println!("Checking port {port}...");

    let listeners = get_all()
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .context("Failed to get system listeners")?;

    let mut found = false;
    let mut printed_header = false;

    for listener in listeners {
        if listener.socket.port() == port {
            if !printed_header {
                println!("{:<10} {:<20} {:<20}", "PID", "NAME", "ADDRESS");
                printed_header = true;
            }
            found = true;
            println!(
                "{:<10} {:<20} {:<20}",
                listener.process.pid, listener.process.name, listener.socket
            );
        }
    }

    if !found {
        println!("No process found listening on port {port}.");
    }

    Ok(())
}
