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
            "Refusing privileged locald operation: daemon at {socket_display} belongs to uid {}, expected uid {expected_uid}.",
            peer_uid.as_raw()
        )));
    }
    send_request_on_stream(stream, request)
}

#[cfg(target_os = "macos")]
pub fn send_request_on_verified_stream_with_timeout(
    stream: &mut UnixStream,
    request: &IpcRequest,
    expected_uid: u32,
    socket_display: &str,
    timeout: std::time::Duration,
) -> CliResult<IpcResponse> {
    stream.set_write_timeout(Some(timeout)).map_err(|error| {
        CliError::message(format!(
            "Failed to bound the locald shutdown request at {socket_display}: {error}"
        ))
    })?;
    let (peer_uid, _) = nix::unistd::getpeereid(&*stream).map_err(|error| {
        CliError::message(format!(
            "Failed to authenticate locald at {socket_display}: {error}"
        ))
    })?;
    if peer_uid.as_raw() != expected_uid {
        return Err(CliError::message(format!(
            "Refusing privileged locald operation: daemon at {socket_display} belongs to uid {}, expected uid {expected_uid}.",
            peer_uid.as_raw()
        )));
    }

    let request_bytes = serialize_request(request)?;
    stream.write_all(&request_bytes).map_err(|error| {
        CliError::message(format!(
            "Failed to send the locald shutdown request at {socket_display}: {error}"
        ))
    })?;

    let deadline = std::time::Instant::now() + timeout;
    let mut response_bytes = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for locald shutdown response",
            )
            .into());
        }
        stream.set_read_timeout(Some(remaining)).map_err(|error| {
            CliError::message(format!(
                "Failed to bound the locald shutdown response at {socket_display}: {error}"
            ))
        })?;
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).map_err(|error| {
            CliError::message(format!(
                "Failed to read the locald shutdown response at {socket_display}: {error}"
            ))
        })?;
        if count == 0 {
            break;
        }
        response_bytes.extend_from_slice(&chunk[..count]);
        if response_bytes.len() > MAX_IPC_REQUEST_BYTES {
            return Err(CliError::message(format!(
                "locald shutdown response is too large (maximum is {MAX_IPC_REQUEST_BYTES} bytes)"
            )));
        }
        match serde_json::from_slice::<IpcResponse>(&response_bytes) {
            Ok(response) => return Ok(response),
            Err(error) if error.is_eof() => {}
            Err(error) => return Err(error.into()),
        }
    }
    if response_bytes.is_empty() {
        return Err(DaemonError::RequestFailed {
            message: "daemon closed the connection without a response".to_owned(),
        }
        .into());
    }
    Ok(serde_json::from_slice(&response_bytes)?)
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

pub fn stream_logs(
    service: Option<String>,
    project_path: Option<std::path::PathBuf>,
    follow: bool,
) -> CliResult<()> {
    let (mut stream, _socket_display) = connect_to_daemon()?;
    let mode = if follow {
        locald_core::ipc::LogMode::Follow
    } else {
        locald_core::ipc::LogMode::Snapshot
    };
    let request = IpcRequest::Logs {
        service,
        project_path,
        mode,
    };
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
        } else if let Ok(IpcResponse::Error(message)) = serde_json::from_str(&line) {
            return Err(DaemonError::RequestFailed { message }.into());
        }
    }
    Ok(())
}

pub fn stream_boot_events(request: &IpcRequest) -> CliResult<()> {
    let (mut stream, _socket_display) = connect_to_daemon()?;
    stream_boot_events_on_stream(&mut stream, request)
}

fn stream_boot_events_on_stream(stream: &mut UnixStream, request: &IpcRequest) -> CliResult<()> {
    match stream_boot_events_response_on_stream(stream, request)? {
        IpcResponse::Ok => Ok(()),
        IpcResponse::Error(message) => Err(DaemonError::RequestFailed { message }.into()),
        response => Err(DaemonError::RequestFailed {
            message: format!("unexpected streamed start response: {response:?}"),
        }
        .into()),
    }
}

pub fn stream_project_ensure(request: &IpcRequest) -> CliResult<IpcResponse> {
    let (mut stream, _socket_display) = connect_to_daemon()?;
    stream_boot_events_response_on_stream(&mut stream, request)
}

fn stream_boot_events_response_on_stream(
    stream: &mut UnixStream,
    request: &IpcRequest,
) -> CliResult<IpcResponse> {
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
            return Ok(response);
        }
    }
    Err(DaemonError::RequestFailed {
        message: "daemon closed the connection before reporting the start result".to_owned(),
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::{
        send_request_on_stream, serialize_request, stream_boot_events_on_stream,
        stream_boot_events_response_on_stream,
    };
    #[cfg(target_os = "macos")]
    use super::{send_request_on_verified_stream, send_request_on_verified_stream_with_timeout};
    use locald_core::{
        AvailabilityReason, IpcRequest, IpcResponse, ProjectLifecycleState,
        ipc::{BootEvent, EnsureProjectResult, EnsureProjectState, EnsureProjectSuperseded},
    };
    use std::io::{Read, Write};
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

    #[test]
    fn streamed_ensure_preserves_boot_events_and_final_readiness_response() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let final_response = IpcResponse::ProjectEnsured(EnsureProjectResult {
            project_path: "/tmp/project".into(),
            project_name: Some("project".to_owned()),
            state: EnsureProjectState::Ready,
            services: Vec::new(),
            urls: vec!["https://project.localhost".to_owned()],
        });
        let expected = final_response.clone();
        let server_thread = thread::spawn(move || {
            let mut server = server;
            let mut request = [0; 1024];
            let _ = server.read(&mut request).unwrap();
            for payload in [
                serde_json::to_vec(&BootEvent::StepProgress {
                    id: "project".to_owned(),
                    message: "waiting for readiness".to_owned(),
                })
                .unwrap(),
                serde_json::to_vec(&final_response).unwrap(),
            ] {
                server.write_all(&payload).unwrap();
                server.write_all(b"\n").unwrap();
            }
        });

        let response =
            stream_boot_events_response_on_stream(&mut client, &IpcRequest::Ping).unwrap();
        server_thread.join().unwrap();

        assert_eq!(response, expected);
    }

    fn superseded_response() -> IpcResponse {
        IpcResponse::ProjectEnsureSuperseded(EnsureProjectSuperseded {
            project_path: "/tmp/project".into(),
            project_name: Some("project".to_owned()),
            state: ProjectLifecycleState::Paused,
            reasons: vec![AvailabilityReason {
                code: "paused".to_owned(),
                message: "The project is paused.".to_owned(),
            }],
        })
    }

    #[test]
    fn quiet_ensure_preserves_structured_supersession_response() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected = superseded_response();
        let response = expected.clone();
        let server_thread = thread::spawn(move || {
            let mut server = server;
            let mut request = [0; 1024];
            let _ = server.read(&mut request).unwrap();
            server
                .write_all(&serde_json::to_vec(&response).unwrap())
                .unwrap();
        });

        let response = send_request_on_stream(&mut client, &IpcRequest::Ping).unwrap();
        server_thread.join().unwrap();

        assert_eq!(response, expected);
    }

    #[test]
    fn verbose_ensure_preserves_boot_events_and_structured_supersession_response() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let expected = superseded_response();
        let response = expected.clone();
        let server_thread = thread::spawn(move || {
            let mut server = server;
            let mut request = [0; 1024];
            let _ = server.read(&mut request).unwrap();
            for payload in [
                serde_json::to_vec(&BootEvent::StepProgress {
                    id: "project".to_owned(),
                    message: "waiting for readiness".to_owned(),
                })
                .unwrap(),
                serde_json::to_vec(&response).unwrap(),
            ] {
                server.write_all(&payload).unwrap();
                server.write_all(b"\n").unwrap();
            }
        });

        let response =
            stream_boot_events_response_on_stream(&mut client, &IpcRequest::Ping).unwrap();
        server_thread.join().unwrap();

        assert_eq!(response, expected);
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

    #[cfg(target_os = "macos")]
    #[test]
    fn privileged_request_times_out_when_an_authenticated_daemon_never_responds() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let server_thread = thread::spawn(move || {
            let mut request = [0; 1024];
            let _ = server.read(&mut request).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
        });

        let started = std::time::Instant::now();
        let current_uid = nix::unistd::geteuid().as_raw();
        let error = send_request_on_verified_stream_with_timeout(
            &mut client,
            &IpcRequest::Shutdown,
            current_uid,
            "test socket",
            std::time::Duration::from_millis(25),
        )
        .expect_err("an authenticated non-responsive daemon must time out");
        let elapsed = started.elapsed();
        server_thread.join().unwrap();

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "bounded shutdown request took {elapsed:?}"
        );
        assert!(
            error.to_string().contains("timed out")
                || error.to_string().contains("temporarily unavailable"),
            "unexpected timeout error: {error}"
        );
    }
}
