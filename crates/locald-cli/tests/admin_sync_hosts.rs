//! Integration coverage for daemon-owned hosts-file synchronization.

#![cfg(target_os = "linux")]

use assert_cmd::Command;
use locald_core::{IpcRequest, IpcResponse};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::thread;
use std::time::{Duration, Instant};

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn admin_sync_hosts_delegates_to_the_daemon_owned_domain_index() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let socket_path = root.path().join("locald.sock");
    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;

    let server = thread::spawn(move || -> anyhow::Result<()> {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::park_timeout(Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    anyhow::bail!("CLI did not connect within {TEST_TIMEOUT:?}");
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_read_timeout(Some(TEST_TIMEOUT))?;
        let mut request_bytes = Vec::new();
        let request = loop {
            let mut chunk = [0_u8; 128];
            let count = stream.read(&mut chunk)?;
            anyhow::ensure!(
                count > 0,
                "CLI closed the connection before sending a request"
            );
            request_bytes.extend_from_slice(&chunk[..count]);

            match serde_json::from_slice::<IpcRequest>(&request_bytes) {
                Ok(request) => break request,
                Err(error) if error.is_eof() => {}
                Err(error) => return Err(error.into()),
            }
        };

        anyhow::ensure!(
            request == IpcRequest::SyncHosts,
            "unexpected request: {request:?}"
        );
        let response = serde_json::to_vec(&IpcResponse::Ok)?;
        stream.write_all(&response)?;
        Ok(())
    });

    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("locald"));
    command
        .timeout(TEST_TIMEOUT)
        .env("LOCALD_SANDBOX_ACTIVE", "1")
        .env("LOCALD_SOCKET", &socket_path)
        .args(["admin", "sync-hosts"]);
    let output = command.output();

    server
        .join()
        .map_err(|_| anyhow::anyhow!("fake daemon thread panicked"))??;
    let output = output?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::ensure!(output.status.success(), "CLI failed: {stderr}");
    anyhow::ensure!(stdout.contains("Hosts file updated."), "stdout: {stdout}");

    Ok(())
}
