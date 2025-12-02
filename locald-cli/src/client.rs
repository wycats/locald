use anyhow::Result;
use crossterm::style::{Color, Stylize};
use locald_core::{IpcRequest, IpcResponse, ipc::LogEntry};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

const SOCKET_PATH: &str = "/tmp/locald.sock";

pub fn send_request(request: &IpcRequest) -> Result<IpcResponse> {
    let mut stream = UnixStream::connect(SOCKET_PATH).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("locald is not running (socket not found at {SOCKET_PATH})")
        } else if e.kind() == std::io::ErrorKind::ConnectionRefused {
            anyhow::anyhow!("locald is not running (connection refused at {SOCKET_PATH})")
        } else {
            anyhow::Error::new(e)
        }
    })?;

    let request_bytes = serde_json::to_vec(request)?;
    stream.write_all(&request_bytes)?;

    let mut response_bytes = Vec::new();
    stream.read_to_end(&mut response_bytes)?;

    let response: IpcResponse = serde_json::from_slice(&response_bytes)?;
    Ok(response)
}

pub fn stream_logs(service: Option<String>) -> Result<()> {
    let mut stream = UnixStream::connect(SOCKET_PATH)?;
    let request = IpcRequest::Logs { service };
    let request_bytes = serde_json::to_vec(&request)?;
    stream.write_all(&request_bytes)?;

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
            let timestamp = chrono::DateTime::from_timestamp(entry.timestamp, 0).map_or_else(
                || entry.timestamp.to_string(),
                |dt| dt.format("%H:%M:%S").to_string(),
            );

            let stream_style = if entry.stream == "stderr" {
                "ERR".with(Color::Red)
            } else {
                "OUT".with(Color::Green)
            };

            println!(
                "{} {} {} | {}",
                timestamp.with(Color::DarkGrey),
                entry.service.cyan().bold(),
                stream_style,
                entry.message
            );
        }
    }
    Ok(())
}
