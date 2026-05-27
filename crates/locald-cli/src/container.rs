use crate::error::{CliError, CliResult, DaemonError};
use locald_core::{IpcRequest, IpcResponse, ipc::Event};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn run(
    image: String,
    command: Vec<String>,
    interactive: bool,
    detached: bool,
) -> CliResult<()> {
    let cmd_opt = if command.is_empty() {
        None
    } else {
        Some(command)
    };

    let request = IpcRequest::RunContainer {
        image,
        command: cmd_opt,
        interactive,
        detached,
    };

    // We manually handle the connection here to stream the response
    let socket_path = locald_utils::ipc::socket_path()?;
    let socket_display = socket_path.display().to_string();
    let mut stream = UnixStream::connect(&socket_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            DaemonError::NotRunning {
                socket_path: socket_display.clone(),
            }
        } else if e.kind() == std::io::ErrorKind::ConnectionRefused {
            DaemonError::ConnectionRefused {
                socket_path: socket_display.clone(),
            }
        } else {
            DaemonError::ConnectionFailed {
                socket_path: socket_display.clone(),
                source: e,
            }
        }
    })?;

    let request_bytes = serde_json::to_vec(&request)?;
    stream.write_all(&request_bytes)?;

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        if let Ok(event) = serde_json::from_str::<Event>(&line) {
            if let Event::Log(entry) = event {
                // Print raw message to stdout/stderr based on stream type
                if entry.stream == locald_core::ipc::LogStream::Stderr {
                    eprint!("{}", entry.message);
                    std::io::stderr().flush()?;
                } else {
                    print!("{}", entry.message);
                    std::io::stdout().flush()?;
                }
            }
        } else if let Ok(response) = serde_json::from_str::<IpcResponse>(&line) {
            match response {
                IpcResponse::Ok => {
                    // println!("Container finished successfully.");
                    return Ok(());
                }
                IpcResponse::Error(e) => {
                    return Err(CliError::message(format!("Container failed: {e}")));
                }
                _ => {
                    return Err(CliError::message(format!(
                        "Unexpected response: {response:?}"
                    )));
                }
            }
        }
    }

    Ok(())
}
