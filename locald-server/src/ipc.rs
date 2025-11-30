use anyhow::Result;
use locald_core::{IpcRequest, IpcResponse};
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Sender;
use tracing::{info, error};
use crate::manager::ProcessManager;

const SOCKET_PATH: &str = "/tmp/locald.sock";

pub async fn run_ipc_server(manager: ProcessManager, shutdown_tx: Sender<()>) -> Result<()> {
    if std::fs::metadata(SOCKET_PATH).is_ok() {
        // Try to connect to see if it's alive
        if UnixStream::connect(SOCKET_PATH).await.is_ok() {
            anyhow::bail!("Socket {} is already in use. Is locald-server already running?", SOCKET_PATH);
        }
        // If we can't connect, it's likely a stale socket
        std::fs::remove_file(SOCKET_PATH)?;
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;
    info!("IPC server listening on {}", SOCKET_PATH);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let manager = manager.clone();
                let shutdown_tx = shutdown_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, manager, shutdown_tx).await {
                        error!("Error handling connection: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Error accepting connection: {}", e);
            }
        }
    }
}

async fn handle_connection(mut stream: UnixStream, manager: ProcessManager, shutdown_tx: Sender<()>) -> Result<()> {
    let mut buf = [0; 4096];
    let n = stream.read(&mut buf).await?;
    
    if n == 0 {
        return Ok(());
    }

    let request: IpcRequest = serde_json::from_slice(&buf[..n])?;
    info!("Received request: {:?}", request);

    let response = match request {
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::Start { path } => {
            match manager.start(path).await {
                Ok(_) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e.to_string()),
            }
        }
        IpcRequest::Stop { name } => {
            match manager.stop(&name).await {
                Ok(_) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e.to_string()),
            }
        }
        IpcRequest::Status => {
            let status = manager.list().await;
            IpcResponse::Status(status)
        }
        IpcRequest::Shutdown => {
            let _ = shutdown_tx.send(()).await;
            IpcResponse::Ok
        }
    };

    let response_bytes = serde_json::to_vec(&response)?;
    stream.write_all(&response_bytes).await?;

    Ok(())
}
