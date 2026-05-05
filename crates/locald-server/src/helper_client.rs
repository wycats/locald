//! macOS privileged helper client.
//!
//! Connects to the `com.locald.helper` Mach service via XPC and requests
//! privileged port binding. The helper binds the port as root and returns
//! the file descriptor via XPC's native FD passing.

use anyhow::{Context, Result};
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use tracing::info;

pub async fn bind_privileged_port(port: u16) -> Result<std::net::TcpListener> {
    info!("Requesting privileged port {port} from helper via XPC...");

    // XPC types are not Send, so the entire exchange runs on a dedicated thread.
    let fd = tokio::task::spawn_blocking(move || xpc_bind(port))
        .await
        .context("XPC helper task panicked")??;

    // SAFETY: We received this FD from the helper via XPC (xpc_fd_dup).
    // Wrap it immediately so fallible setup below cannot leak the listener.
    #[allow(unsafe_code)]
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    set_cloexec(&fd).context("Failed to set FD_CLOEXEC on helper listener")?;

    let fd = fd.into_raw_fd();

    // SAFETY: We received this FD from the helper via XPC (xpc_fd_dup).
    // We are taking ownership and converting it to a TcpListener.
    #[allow(unsafe_code)]
    let tcp_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };

    info!("Acquired port {port} from helper");
    Ok(tcp_listener)
}

fn set_cloexec(fd: &OwnedFd) -> Result<()> {
    let flags = nix::fcntl::fcntl(fd, nix::fcntl::F_GETFD).context("Failed to get FD flags")?;
    nix::fcntl::fcntl(
        fd,
        nix::fcntl::F_SETFD(
            nix::fcntl::FdFlag::from_bits_truncate(flags) | nix::fcntl::FdFlag::FD_CLOEXEC,
        ),
    )
    .context("Failed to set FD_CLOEXEC")?;
    Ok(())
}

fn xpc_bind(port: u16) -> Result<std::os::unix::io::RawFd> {
    use futures::stream::StreamExt;
    use std::collections::HashMap;
    use std::ffi::CString;
    use xpc_connection::{Message, XpcClient};

    #[allow(clippy::expect_used)]
    let name = CString::new("com.locald.helper").expect("static CString");
    let mut client = XpcClient::connect(&name);

    // Build { "command": "bind", "port": <port> }
    let mut dict = HashMap::new();
    #[allow(clippy::expect_used)]
    {
        dict.insert(
            CString::new("command").expect("static CString"),
            Message::String(CString::new("bind").expect("static CString")),
        );
        dict.insert(
            CString::new("port").expect("static CString"),
            Message::Int64(i64::from(port)),
        );
    }
    client.send_message(Message::Dictionary(dict));

    // Single-threaded runtime for the XPC response.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime for XPC")?;

    let response = rt.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), client.next())
            .await
            .context("Timed out waiting for helper response")?
            .context("Helper connection closed without response")
    })?;

    #[allow(clippy::wildcard_enum_match_arm)]
    match response {
        Message::Dictionary(ref dict) => {
            #[allow(clippy::expect_used)]
            let status_key = CString::new("status").expect("static CString");
            #[allow(clippy::expect_used)]
            let fd_key = CString::new("fd").expect("static CString");
            #[allow(clippy::expect_used)]
            let msg_key = CString::new("message").expect("static CString");

            // Check status
            let status = match dict.get(&status_key) {
                Some(Message::String(s)) => s.to_string_lossy().to_string(),
                _ => anyhow::bail!("Helper returned invalid response (no status)"),
            };

            if status != "success" {
                let msg = dict
                    .get(&msg_key)
                    .and_then(|m| {
                        #[allow(clippy::wildcard_enum_match_arm)]
                        match m {
                            Message::String(s) => Some(s.to_string_lossy().to_string()),
                            _ => None,
                        }
                    })
                    .unwrap_or_else(|| "unknown error".to_string());
                anyhow::bail!("Helper refused to bind port {port}: {msg}");
            }

            // Extract the FD
            match dict.get(&fd_key) {
                Some(Message::Fd(fd)) => Ok(*fd),
                _ => anyhow::bail!("Helper response missing fd"),
            }
        }
        _ => anyhow::bail!("Unexpected XPC response type"),
    }
}
