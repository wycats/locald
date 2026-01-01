use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use tokio::fs;
use tracing::debug;

#[derive(Debug)]
struct TcpEntry {
    local_port: u16,
    inode: u64,
    _state: u8,
}

async fn parse_tcp_file(path: &Path) -> Result<Vec<TcpEntry>> {
    let content = fs::read_to_string(path).await?;
    let mut entries = Vec::new();

    for line in content.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }

        // local_address is column 1 (0-indexed) -> "0100007F:1F90"
        let local_addr_str = parts[1];
        let state_str = parts[3];
        let inode_str = parts[9];

        let state = u8::from_str_radix(state_str, 16)?;
        if state != 0x0A {
            // Not LISTEN
            continue;
        }

        let (_ip, port_hex) = local_addr_str
            .split_once(':')
            .context("Invalid local address format")?;
        let local_port = u16::from_str_radix(port_hex, 16)?;
        let inode = inode_str.parse::<u64>()?;

        entries.push(TcpEntry {
            local_port,
            inode,
            _state: state,
        });
    }

    Ok(entries)
}

async fn get_sockets_for_pid(pid: u32) -> Result<HashSet<u64>> {
    let fd_path = format!("/proc/{pid}/fd");
    let mut sockets = HashSet::new();

    if !Path::new(&fd_path).exists() {
        return Ok(sockets);
    }

    let mut entries = fs::read_dir(fd_path).await?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(target) = fs::read_link(entry.path()).await
            && let Some(target_str) = target.to_str()
            && target_str.starts_with("socket:[")
        {
            let inode_str = target_str
                .trim_start_matches("socket:[")
                .trim_end_matches(']');
            if let Ok(inode) = inode_str.parse::<u64>() {
                sockets.insert(inode);
            }
        }
    }

    Ok(sockets)
}

async fn get_descendants(pid: u32) -> Vec<u32> {
    let mut descendants = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(pid);

    while let Some(current_pid) = queue.pop_front() {
        descendants.push(current_pid);

        let children_path = format!("/proc/{current_pid}/task/{current_pid}/children");
        if let Ok(content) = fs::read_to_string(&children_path).await {
            for child_str in content.split_whitespace() {
                if let Ok(child_pid) = child_str.parse::<u32>() {
                    queue.push_back(child_pid);
                }
            }
        }
    }

    descendants
}

/// Find listening TCP ports for a process and its descendants.
///
/// # Errors
///
/// Returns an error if:
/// - Reading `/proc` fails.
/// - Parsing port numbers fails.
pub async fn find_listening_ports(pid: u32) -> Result<Vec<u16>> {
    debug!("Scanning ports for PID {} and descendants", pid);
    let pids = get_descendants(pid).await;
    debug!("Found {} descendants for PID {}", pids.len(), pid);

    let mut all_sockets = HashSet::new();
    for p in &pids {
        if let Ok(sockets) = get_sockets_for_pid(*p).await {
            all_sockets.extend(sockets);
        }
    }

    if all_sockets.is_empty() {
        return Ok(Vec::new());
    }

    // 2. Read /proc/net/tcp and tcp6
    let mut ports = Vec::new();

    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let path = Path::new(file);
        if !path.exists() {
            continue;
        }

        if let Ok(entries) = parse_tcp_file(path).await {
            debug!("Parsed {} entries from {:?}", entries.len(), path);
            for entry in entries {
                if all_sockets.contains(&entry.inode) {
                    debug!(
                        "Found match: port {} inode {}",
                        entry.local_port, entry.inode
                    );
                    ports.push(entry.local_port);
                }
            }
        }
    }

    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_entry_construction() {
        // Verify TcpEntry can be constructed with the underscore-prefixed field.
        // The _state field is parsed from /proc/net/tcp but not used after filtering.
        let entry = TcpEntry {
            local_port: 8080,
            inode: 12345,
            _state: 0x0A, // LISTEN state
        };
        assert_eq!(entry.local_port, 8080);
        assert_eq!(entry.inode, 12345);
    }
}
