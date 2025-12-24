use anyhow::Result;
use crossterm::style::{Color, Stylize};
use locald_core::{
    IpcRequest, IpcResponse,
    ipc::{LogEntry, LogStream},
};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

pub fn send_request(request: &IpcRequest) -> Result<IpcResponse> {
    let socket_path = locald_utils::ipc::socket_path()?;
    let mut stream = UnixStream::connect(&socket_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "locald is not running (socket not found at {})",
                socket_path.display()
            )
        } else if e.kind() == std::io::ErrorKind::ConnectionRefused {
            anyhow::anyhow!(
                "locald is not running (connection refused at {})",
                socket_path.display()
            )
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

pub fn stream_logs(service: Option<String>, follow: bool) -> Result<()> {
    let socket_path = locald_utils::ipc::socket_path()?;
    let mut stream = UnixStream::connect(socket_path)?;
    let mode = if follow {
        locald_core::ipc::LogMode::Follow
    } else {
        locald_core::ipc::LogMode::Snapshot
    };
    let request = IpcRequest::Logs { service, mode };
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

            let stream_style = if entry.stream == LogStream::Stderr {
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

pub fn stream_boot_events(request: &IpcRequest) -> Result<()> {
    let socket_path = locald_utils::ipc::socket_path()?;
    let mut stream = UnixStream::connect(socket_path)?;
    let request_bytes = serde_json::to_vec(request)?;
    stream.write_all(&request_bytes)?;

    let mut renderer = crate::progress::ProgressRenderer::new();
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        // Check if it's a BootEvent
        if let Ok(event) = serde_json::from_str::<locald_core::ipc::BootEvent>(&line) {
            renderer.handle_event(event);
        } else if let Ok(response) = serde_json::from_str::<IpcResponse>(&line) {
            // It might be the final response (Ok or Error)
            match response {
                IpcResponse::Ok => return Ok(()),
                IpcResponse::Error(msg) => anyhow::bail!(msg),
                _ => {} // Ignore other responses?
            }
        }
    }
    Ok(())
}
