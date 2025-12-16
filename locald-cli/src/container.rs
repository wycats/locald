use anyhow::Result;
use locald_core::{IpcRequest, IpcResponse, ipc::Event};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn run(image: String, command: Vec<String>, interactive: bool, detached: bool) -> Result<()> {
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
                    anyhow::bail!("Container failed: {}", e);
                }
                _ => {
                    anyhow::bail!("Unexpected response: {:?}", response);
                }
            }
        }
    }

    Ok(())
}
