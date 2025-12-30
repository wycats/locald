use anyhow::{Context, Result};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixListener;
use tracing::{debug, info};

pub async fn bind_privileged_port(port: u16) -> Result<std::net::TcpListener> {
    info!("Requesting privileged port {} from locald-shim...", port);

    // 1. Create a temporary directory for the socket
    let temp_dir = tempfile::tempdir().context("Failed to create temp dir")?;
    let socket_path = temp_dir.path().join("shim_handshake.sock");

    // 2. Create a Unix Domain Socket listener
    let listener = UnixListener::bind(&socket_path).context("Failed to bind handshake socket")?;

    // 3. Spawn locald-shim
    // Daemon context: never prompt for sudo. Require an already-installed privileged shim.
    let mut cmd = locald_utils::shim::tokio_command_privileged()?;

    debug!("Invoking shim");

    let status = cmd
        .arg("bind")
        .arg(port.to_string())
        .arg(&socket_path)
        .status()
        .await
        .context("Failed to execute locald-shim")?;

    if !status.success() {
        return Err(anyhow::anyhow!(
            "locald-shim failed with status: {}",
            status
        ));
    }

    // 4-6. Accept connection + receive FD + convert to TcpListener.
    // This uses blocking syscalls (accept + recvmsg), so run it off the async executor.
    let fd = tokio::task::spawn_blocking(move || -> Result<std::os::unix::io::RawFd> {
        let (stream, _) = listener
            .accept()
            .context("Failed to accept connection from shim")?;

        let mut buf = [0u8; 16];
        let mut cmsg_buffer = nix::cmsg_space!([std::os::unix::io::RawFd; 1]);
        let mut iov = [std::io::IoSliceMut::new(&mut buf)];

        let msg = nix::sys::socket::recvmsg::<nix::sys::socket::UnixAddr>(
            stream.as_raw_fd(),
            &mut iov,
            Some(&mut cmsg_buffer),
            nix::sys::socket::MsgFlags::empty(),
        )
        .context("Failed to receive message from shim")?;

        let fd = if let Some(cmsg) = msg.cmsgs()?.next() {
            #[allow(clippy::wildcard_enum_match_arm)]
            match cmsg {
                nix::sys::socket::ControlMessageOwned::ScmRights(fds) => {
                    if fds.is_empty() {
                        return Err(anyhow::anyhow!("No FDs received from shim"));
                    }
                    fds[0]
                }
                _ => return Err(anyhow::anyhow!("Unexpected control message from shim")),
            }
        } else {
            return Err(anyhow::anyhow!("No control message received from shim"));
        };

        // Set FD_CLOEXEC to prevent leaking the privileged port to child processes
        // SAFETY: We own the raw fd returned by recvmsg, so borrowing it is safe here.
        #[allow(unsafe_code)]
        let borrowed_fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        let flags = nix::fcntl::fcntl(borrowed_fd, nix::fcntl::F_GETFD)
            .context("Failed to get FD flags")?;
        nix::fcntl::fcntl(
            borrowed_fd,
            nix::fcntl::F_SETFD(
                nix::fcntl::FdFlag::from_bits_truncate(flags) | nix::fcntl::FdFlag::FD_CLOEXEC,
            ),
        )
        .context("Failed to set FD_CLOEXEC")?;

        Ok(fd)
    })
    .await
    .context("Blocking shim handshake task panicked")??;

    // SAFETY: We received this FD from the shim via SCM_RIGHTS.
    // We are taking ownership of it and converting it to a TcpListener.
    #[allow(unsafe_code)]
    let tcp_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };

    info!("Successfully acquired port {} from shim", port);
    Ok(tcp_listener)
}
