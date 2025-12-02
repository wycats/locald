use anyhow::Result;
use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
use std::path::PathBuf;
use tokio::net::UnixDatagram;
use tracing::{error, info};

pub struct NotifyServer {
    socket: UnixDatagram,
}

impl NotifyServer {
    pub async fn new(path: PathBuf) -> Result<Self> {
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        let socket = UnixDatagram::bind(&path)?;
        Ok(Self { socket })
    }

    pub async fn run(self, tx: tokio::sync::mpsc::Sender<(u32, String)>) {
        let mut buf = [0u8; 1024];
        loop {
            match self.socket.recv_from(&mut buf).await {
                Ok((size, _addr)) => {
                    let data = String::from_utf8_lossy(&buf[..size]);
                    // Get PID of sender
                    match getsockopt(&self.socket, PeerCredentials) {
                        Ok(creds) => {
                            let pid = creds.pid();
                            info!("Received notify from PID {}: {}", pid, data);
                            if data.contains("READY=1")
                                && let Err(e) = tx.send((pid as u32, "READY".to_string())).await
                            {
                                error!("Failed to send notify event: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Failed to get peer creds: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Notify socket error: {}", e);
                }
            }
        }
    }
}
