use anyhow::Result;
use locald_core::{IpcRequest, IpcResponse};
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error};

const SOCKET_PATH: &str = "/tmp/locald.sock";

pub async fn run_ipc_server() -> Result<()> {
    if std::fs::metadata(SOCKET_PATH).is_ok() {
        std::fs::remove_file(SOCKET_PATH)?;
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;
    info!("IPC server listening on {}", SOCKET_PATH);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream).await {
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

async fn handle_connection(mut stream: UnixStream) -> Result<()> {
    let mut buf = [0; 4096];
    let n = stream.read(&mut buf).await?;
    
    if n == 0 {
        return Ok(());
    }

    let request: IpcRequest = serde_json::from_slice(&buf[..n])?;
    info!("Received request: {:?}", request);

    let response = match request {
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::Register(_) => IpcResponse::Ok, // TODO: Implement registration
    };

    let response_bytes = serde_json::to_vec(&response)?;
    stream.write_all(&response_bytes).await?;

    Ok(())
}
