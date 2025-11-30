use anyhow::Result;
use locald_core::{IpcRequest, IpcResponse};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

const SOCKET_PATH: &str = "/tmp/locald.sock";

pub fn send_request(request: IpcRequest) -> Result<IpcResponse> {
    let mut stream = UnixStream::connect(SOCKET_PATH)?;
    
    let request_bytes = serde_json::to_vec(&request)?;
    stream.write_all(&request_bytes)?;
    // We don't strictly need to shutdown write if the server reads to end or uses length prefix,
    // but since the server currently reads once, it might be fine.
    // Actually, the server reads into a buffer.
    // Let's just write and read.
    
    let mut response_bytes = Vec::new();
    stream.read_to_end(&mut response_bytes)?;

    let response: IpcResponse = serde_json::from_slice(&response_bytes)?;
    Ok(response)
}
