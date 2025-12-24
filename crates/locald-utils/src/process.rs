use anyhow::Result;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use portable_pty::Child;
use tracing::{error, info, warn};

/// Kills a process by PID.
///
/// # Errors
///
/// Returns an error if the signal cannot be sent to the process.
pub fn kill_pid(pid: i32, signal: Signal) -> Result<()> {
    kill(Pid::from_raw(pid), signal).map_err(|e| anyhow::anyhow!("Failed to kill pid {pid}: {e}"))
}

/// Terminates a child process gracefully.
///
/// Sends the specified signal (usually SIGTERM or SIGINT), waits for the process to exit,
/// and force-kills it with SIGKILL if it doesn't exit within 5 seconds.
pub async fn terminate_gracefully(child: &mut Box<dyn Child + Send>, name: &str, signal: Signal) {
    let Some(pid) = child.process_id() else {
        return;
    };

    info!("Sending {:?} to service {} (PGID: {})", signal, name, pid);
    let pid_i32 = i32::try_from(pid).unwrap_or(i32::MAX);

    // Send signal to process group (negative PID)
    if let Err(e) = kill(Pid::from_raw(-pid_i32), signal) {
        // Ignore ESRCH (process already gone)
        if e != nix::errno::Errno::ESRCH {
            error!("Failed to send {:?} to {}: {}", signal, name, e);
        }
    }

    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > std::time::Duration::from_secs(5) {
            warn!("Service {} did not exit, sending SIGKILL", name);
            if let Err(e) = kill(Pid::from_raw(-pid_i32), Signal::SIGKILL) {
                warn!("Failed to force kill service {}: {}", name, e);
            }
            break;
        }

        match child.try_wait() {
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            _ => break,
        }
    }
}
