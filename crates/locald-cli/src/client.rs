use crossterm::style::{Color, Stylize};
use locald_core::{
    IpcRequest, IpcResponse,
    ipc::{LogEntry, LogStream, MAX_IPC_REQUEST_BYTES},
};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

use crate::error::{CliError, CliResult, DaemonError};

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

#[cfg(target_os = "macos")]
pub fn send_request_from_uid(request: &IpcRequest, expected_uid: u32) -> CliResult<IpcResponse> {
    let (mut stream, socket_display) = connect_to_daemon()?;
    send_request_on_verified_stream(&mut stream, request, expected_uid, &socket_display)
}

#[cfg(target_os = "macos")]
fn send_request_on_verified_stream(
    stream: &mut UnixStream,
    request: &IpcRequest,
    expected_uid: u32,
    socket_display: &str,
) -> CliResult<IpcResponse> {
    let (peer_uid, _) = nix::unistd::getpeereid(&*stream).map_err(|error| {
        CliError::message(format!(
            "Failed to authenticate locald at {socket_display}: {error}"
        ))
    })?;
    if peer_uid.as_raw() != expected_uid {
        return Err(CliError::message(format!(
            "Refusing privileged hosts synchronization: locald at {socket_display} belongs to uid {}, expected uid {expected_uid}.",
            peer_uid.as_raw()
        )));
    }
    send_request_on_stream(stream, request)
}

fn send_request_on_stream(stream: &mut UnixStream, request: &IpcRequest) -> CliResult<IpcResponse> {
    let request_bytes = serialize_request(request)?;
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

pub fn serialize_request(request: &IpcRequest) -> CliResult<Vec<u8>> {
    let request_bytes = serde_json::to_vec(request)?;
    if request_bytes.len() > MAX_IPC_REQUEST_BYTES {
        return Err(CliError::message(format!(
            "locald request is too large ({} bytes; maximum is {MAX_IPC_REQUEST_BYTES})",
            request_bytes.len()
        )));
    }
    Ok(request_bytes)
}

pub fn stream_logs(service: Option<String>, follow: bool) -> CliResult<()> {
    let (mut stream, _socket_display) = connect_to_daemon()?;
    let mode = if follow {
        locald_core::ipc::LogMode::Follow
    } else {
        locald_core::ipc::LogMode::Snapshot
    };
    let request = IpcRequest::Logs { service, mode };
    let request_bytes = serialize_request(&request)?;
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
    stream_boot_events_on_stream(&mut stream, request)
}

fn stream_boot_events_on_stream(stream: &mut UnixStream, request: &IpcRequest) -> CliResult<()> {
    let request_bytes = serialize_request(request)?;
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
    Err(DaemonError::RequestFailed {
        message: "daemon closed the connection before reporting the start result".to_owned(),
    }
    .into())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::send_request_on_verified_stream;
    use super::{send_request_on_stream, serialize_request, stream_boot_events_on_stream};
    use locald_core::IpcRequest;
    use std::io::Read;
    #[cfg(target_os = "macos")]
    use std::io::Write;
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

    #[test]
    fn oversized_request_is_rejected_before_writing() {
        let request = IpcRequest::Start {
            project_path: "/tmp/project".into(),
            verbose: false,
            launch_path: Some("x".repeat(locald_core::ipc::MAX_IPC_REQUEST_BYTES)),
            manual_cli_session: None,
        };

        let error = serialize_request(&request).expect_err("oversized IPC request must fail");

        assert!(error.to_string().contains("request is too large"));
    }

    #[test]
    fn streamed_start_reports_eof_before_final_response() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let server_thread = thread::spawn(move || {
            let mut server = server;
            let mut request = [0; 1024];
            let _ = server.read(&mut request).unwrap();
        });

        let error = stream_boot_events_on_stream(&mut client, &IpcRequest::Ping)
            .expect_err("EOF before a final response must fail");
        server_thread.join().unwrap();

        assert!(
            error
                .to_string()
                .contains("closed the connection before reporting the start result")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn privileged_request_rejects_a_foreign_peer_uid() {
        let (mut client, _server) = UnixStream::pair().unwrap();
        let current_uid = nix::unistd::geteuid().as_raw();
        let foreign_uid = if current_uid == u32::MAX {
            current_uid - 1
        } else {
            current_uid + 1
        };

        let error = send_request_on_verified_stream(
            &mut client,
            &IpcRequest::GetHostsDomains,
            foreign_uid,
            "test socket",
        )
        .expect_err("foreign peer must be rejected before the request is sent");

        assert!(error.to_string().contains("belongs to uid"));
        assert!(error.to_string().contains("expected uid"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn privileged_request_rejects_an_injected_domain() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let server_thread = thread::spawn(move || {
            let mut request = [0; 1024];
            let _ = server.read(&mut request).unwrap();
            let response = serde_json::to_vec(&serde_json::json!({
                "HostsDomains": ["app.localhost\n127.0.0.1 injected.example"]
            }))
            .unwrap();
            server.write_all(&response).unwrap();
        });

        let current_uid = nix::unistd::geteuid().as_raw();
        let error = send_request_on_verified_stream(
            &mut client,
            &IpcRequest::GetHostsDomains,
            current_uid,
            "test socket",
        )
        .expect_err("invalid domain response must fail deserialization");
        server_thread.join().unwrap();

        assert!(error.to_string().contains("invalid domain"));
    }
}
