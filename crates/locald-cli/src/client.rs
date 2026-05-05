use crossterm::style::{Color, Stylize};
use locald_core::{
    IpcRequest, IpcResponse,
    ipc::{LogEntry, LogStream},
};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

use crate::error::{CliResult, DaemonError};

fn connect_to_daemon() -> Result<(UnixStream, String), DaemonError> {
    let socket_path = locald_utils::ipc::socket_path().map_err(DaemonError::from)?;
    let socket_display = socket_path.display().to_string();
    let stream = UnixStream::connect(&socket_path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => DaemonError::NotRunning {
            socket_path: socket_display.clone(),
        },
        std::io::ErrorKind::ConnectionRefused => DaemonError::ConnectionRefused {
            socket_path: socket_display.clone(),
        },
        std::io::ErrorKind::PermissionDenied => DaemonError::PermissionDenied {
            socket_path: socket_display.clone(),
        },
        _ => DaemonError::ConnectionFailed {
            socket_path: socket_display.clone(),
            source: e,
        },
    })?;
    Ok((stream, socket_display))
}

pub fn send_request(request: &IpcRequest) -> CliResult<IpcResponse> {
    let (mut stream, _socket_display) = connect_to_daemon()?;
    send_request_on_stream(&mut stream, request)
}

fn send_request_on_stream(stream: &mut UnixStream, request: &IpcRequest) -> CliResult<IpcResponse> {
    let request_bytes = serde_json::to_vec(request)?;
    stream.write_all(&request_bytes)?;

    let mut response_bytes = Vec::new();
    stream.read_to_end(&mut response_bytes)?;
    if response_bytes.is_empty() {
        return Err(DaemonError::RequestFailed {
            message: "daemon closed the connection without a response".to_string(),
        }
        .into());
    }

    let response: IpcResponse = serde_json::from_slice(&response_bytes)?;
    Ok(response)
}

pub fn stream_logs(service: Option<String>, follow: bool) -> CliResult<()> {
    let (mut stream, _socket_display) = connect_to_daemon()?;
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

pub fn stream_boot_events(request: &IpcRequest) -> CliResult<()> {
    let (mut stream, _socket_display) = connect_to_daemon()?;
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
                IpcResponse::Error(msg) => {
                    return Err(DaemonError::RequestFailed { message: msg }.into());
                }
                _ => {} // Ignore other responses?
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::send_request_on_stream;
    use locald_core::IpcRequest;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::thread;

    #[test]
    fn send_request_reports_empty_response() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let server_thread = thread::spawn(move || {
            let mut server = server;
            let mut request = [0; 1024];
            let _ = server.read(&mut request).unwrap();
        });

        let err = send_request_on_stream(&mut client, &IpcRequest::Ping).unwrap_err();
        server_thread.join().unwrap();

        assert!(
            err.to_string()
                .contains("daemon closed the connection without a response")
        );
    }
}
