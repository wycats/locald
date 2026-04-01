use anyhow::Result;
use std::collections::HashSet;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};
use tracing::debug;

/// Collect all descendant PIDs of `root_pid` (inclusive) via `sysinfo`.
fn get_descendants(root_pid: u32) -> HashSet<u32> {
    // Only fetch the minimal process info — parent PID is always included.
    let sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );

    // Build a parent → children map.
    let mut children_map: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    for (pid, process) in sys.processes() {
        if let Some(parent) = process.parent() {
            children_map
                .entry(parent.as_u32())
                .or_default()
                .push(pid.as_u32());
        }
    }

    // BFS from root_pid.
    let mut descendants = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root_pid);

    while let Some(current) = queue.pop_front() {
        descendants.insert(current);
        if let Some(kids) = children_map.get(&current) {
            for &kid in kids {
                if descendants.insert(kid) {
                    queue.push_back(kid);
                }
            }
        }
    }

    descendants
}

/// Find listening TCP ports for a process and its descendants.
///
/// Uses `sysinfo` for cross-platform process tree walking and `listeners`
/// for cross-platform listening socket discovery. No shell-outs required.
///
/// # Errors
///
/// Returns an error if listening socket enumeration fails.
pub async fn find_listening_ports(pid: u32) -> Result<Vec<u16>> {
    debug!("Scanning ports for PID {} and descendants", pid);

    // Both sysinfo and listeners are blocking; run off the async runtime.
    tokio::task::spawn_blocking(move || {
        let descendant_pids = get_descendants(pid);
        debug!(
            "Found {} descendants for PID {}",
            descendant_pids.len(),
            pid
        );

        let all_listeners = listeners::get_all()
            .map_err(|e| anyhow::anyhow!("Failed to enumerate listeners: {e}"))?;

        let ports: HashSet<u16> = all_listeners
            .into_iter()
            .filter(|l| {
                l.protocol == listeners::Protocol::TCP && descendant_pids.contains(&l.process.pid)
            })
            .map(|l| l.socket.port())
            .collect();

        for &port in &ports {
            debug!(
                "Found listening port {} in process tree of PID {}",
                port, pid
            );
        }

        Ok(ports.into_iter().collect())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_descendants_includes_self() {
        let pids = get_descendants(std::process::id());
        assert!(pids.contains(&std::process::id()));
    }

    #[tokio::test]
    async fn find_listening_ports_current_process() {
        // Current process likely has no listeners, but the call should not error.
        let ports = find_listening_ports(std::process::id()).await.unwrap();
        // We can't assert specific ports, just that it didn't panic/error.
        let _ = ports;
    }
}
