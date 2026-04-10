//! macOS privileged helper daemon for locald.
//!
//! Runs as a `LaunchDaemon` and performs operations that require root:
//! privileged port binding (80/443) and CA trust installation.
//! Communicates with the server and agent via XPC (Mach service
//! `com.locald.helper`).

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    #[allow(clippy::print_stdout)]
    {
        println!("locald-helper is macOS-only");
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{Context, Result};
    use futures::stream::StreamExt;
    use std::collections::HashMap;
    use std::ffi::CString;
    use xpc_connection::{Message, XpcClient, XpcListener};

    const MACH_SERVICE: &str = "com.locald.helper";

    // ── entry point ─────────────────────────────────────────────────────

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(serve())?;
        Ok(())
    }

    #[allow(clippy::future_not_send)]
    async fn serve() -> Result<()> {
        #[allow(clippy::expect_used)]
        let name = CString::new(MACH_SERVICE).expect("static CString");
        let mut listener = XpcListener::listen(&name);

        while let Some(client) = listener.next().await {
            tokio::spawn(handle_client(client));
        }

        Ok(())
    }

    #[allow(clippy::future_not_send)]
    async fn handle_client(mut client: XpcClient) {
        // Validate caller: reject connections from root (uid 0).
        // Only the console user's agent should be calling us.
        // audit_token_t layout: [auid, euid, egid, ruid, rgid, pid, asid, pidver]
        // Each field is u32. We want euid (field 1, bytes 4-7).
        let token = client.audit_token();
        let caller_uid = u32::from_le_bytes([token[4], token[5], token[6], token[7]]);
        if caller_uid == 0 {
            client.send_message(error_response("rejected: caller is root"));
            return;
        }

        loop {
            match client.next().await {
                None | Some(Message::Error(_)) => break,
                Some(Message::Dictionary(dict)) => {
                    let response = handle_command(&dict, caller_uid);
                    client.send_message(response);
                }
                Some(_) => {
                    client.send_message(error_response("expected dictionary message"));
                }
            }
        }
    }

    fn handle_command(dict: &HashMap<CString, Message>, caller_uid: u32) -> Message {
        #[allow(clippy::expect_used)]
        let command_key = CString::new("command").expect("static CString");

        let command = match dict.get(&command_key) {
            Some(Message::String(s)) => s.to_string_lossy().to_string(),
            _ => return error_response("missing or invalid 'command' field"),
        };

        match command.as_str() {
            "setup" => execute_setup(caller_uid),
            "bind" => execute_bind(dict),
            _ => error_response(&format!("unknown command: {command}")),
        }
    }

    fn execute_setup(caller_uid: u32) -> Message {
        if let Err(e) = do_setup(caller_uid) {
            return error_response(&format!("{e:#}"));
        }
        success_response()
    }

    fn do_setup(caller_uid: u32) -> Result<()> {
        // 1. Trust the Root CA in the system keychain (for the calling user).
        trust_root_ca(caller_uid).context("CA trust failed")?;

        Ok(())
    }

    // ── bind (privileged port) ──────────────────────────────────────────

    fn execute_bind(dict: &HashMap<CString, Message>) -> Message {
        #[allow(clippy::expect_used)]
        let port_key = CString::new("port").expect("static CString");

        let port = match dict.get(&port_key) {
            Some(Message::Int64(p)) => *p as u16,
            _ => return error_response("missing or invalid 'port' field"),
        };

        // Only allow binding well-known privileged ports.
        if port != 80 && port != 443 {
            return error_response(&format!(
                "refused to bind port {port}: only 80 and 443 are allowed"
            ));
        }

        match bind_privileged_port(port) {
            Ok(fd) => {
                let mut resp = HashMap::new();
                #[allow(clippy::expect_used)]
                {
                    resp.insert(
                        CString::new("status").expect("static CString"),
                        Message::String(CString::new("success").expect("static CString")),
                    );
                    resp.insert(CString::new("fd").expect("static CString"), Message::Fd(fd));
                }
                Message::Dictionary(resp)
            }
            Err(e) => error_response(&format!("{e:#}")),
        }
    }

    /// Bind a privileged TCP port and return the raw FD.
    /// The caller receives the FD via XPC and takes ownership.
    fn bind_privileged_port(port: u16) -> Result<std::os::unix::io::RawFd> {
        use std::os::unix::io::IntoRawFd;

        let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .context("Failed to create socket")?;

        socket.set_reuse_address(true)?;
        socket
            .bind(&addr.into())
            .with_context(|| format!("Failed to bind port {port}"))?;
        socket.listen(1024).context("Failed to listen")?;

        Ok(socket.into_raw_fd())
    }

    // ── CA trust ────────────────────────────────────────────────────────

    /// Trust the locald Root CA in the system keychain.
    ///
    /// The helper runs as root, so the CA cert lives in the invoking user's
    /// home directory. We resolve the cert path from the caller's UID
    /// (obtained from the XPC audit token).
    #[allow(clippy::disallowed_methods)]
    fn trust_root_ca(caller_uid: u32) -> Result<()> {
        let ca_path = resolve_user_ca_path(caller_uid)?;

        if !ca_path.exists() {
            anyhow::bail!(
                "Root CA not found at {}. Run `locald admin trust` first.",
                ca_path.display()
            );
        }

        let status = std::process::Command::new("security")
            .args(["add-trusted-cert", "-d", "-r", "trustRoot", "-k"])
            .arg("/Library/Keychains/System.keychain")
            .arg(&ca_path)
            .status()
            .context("Failed to execute `security add-trusted-cert`")?;

        if !status.success() {
            anyhow::bail!("`security add-trusted-cert` failed with status: {status}");
        }

        Ok(())
    }

    /// Resolve the Root CA path for the calling user (identified by UID
    /// from the XPC audit token).
    fn resolve_user_ca_path(caller_uid: u32) -> Result<std::path::PathBuf> {
        let uid = nix::unistd::Uid::from_raw(caller_uid);
        let user = nix::unistd::User::from_uid(uid)
            .context("Failed to look up user by UID")?
            .context("User not found for UID")?;

        let ca_path = user
            .dir
            .join("Library")
            .join("Application Support")
            .join("locald")
            .join("certs")
            .join("rootCA.pem");

        Ok(ca_path)
    }

    // ── XPC response helpers ────────────────────────────────────────────

    fn success_response() -> Message {
        let mut dict = HashMap::new();
        #[allow(clippy::expect_used)]
        {
            dict.insert(
                CString::new("status").expect("static CString"),
                Message::String(CString::new("success").expect("static CString")),
            );
        }
        Message::Dictionary(dict)
    }

    fn error_response(msg: &str) -> Message {
        let mut dict = HashMap::new();
        #[allow(clippy::expect_used)]
        {
            dict.insert(
                CString::new("status").expect("static CString"),
                Message::String(CString::new("error").expect("static CString")),
            );
            dict.insert(
                CString::new("message").expect("static CString"),
                Message::String(CString::new(msg.replace('\0', "")).expect("sanitized CString")),
            );
        }
        Message::Dictionary(dict)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn make_command_dict(command: &str) -> HashMap<CString, Message> {
            let mut dict = HashMap::new();
            dict.insert(
                CString::new("command").unwrap(),
                Message::String(CString::new(command).unwrap()),
            );
            dict
        }

        fn get_status(msg: &Message) -> String {
            match msg {
                Message::Dictionary(d) => {
                    let key = CString::new("status").unwrap();
                    match d.get(&key) {
                        Some(Message::String(s)) => s.to_string_lossy().to_string(),
                        _ => panic!("no status field"),
                    }
                }
                _ => panic!("expected dictionary"),
            }
        }

        fn get_message_field(msg: &Message) -> String {
            match msg {
                Message::Dictionary(d) => {
                    let key = CString::new("message").unwrap();
                    match d.get(&key) {
                        Some(Message::String(s)) => s.to_string_lossy().to_string(),
                        _ => panic!("no message field"),
                    }
                }
                _ => panic!("expected dictionary"),
            }
        }

        #[test]
        fn unknown_command_returns_error() {
            let dict = make_command_dict("bogus");
            let resp = handle_command(&dict, 501);
            assert_eq!(get_status(&resp), "error");
            assert!(get_message_field(&resp).contains("unknown command: bogus"));
        }

        #[test]
        fn missing_command_field_returns_error() {
            let dict = HashMap::new();
            let resp = handle_command(&dict, 501);
            assert_eq!(get_status(&resp), "error");
            assert!(get_message_field(&resp).contains("missing or invalid"));
        }

        #[test]
        fn wrong_command_type_returns_error() {
            let mut dict = HashMap::new();
            dict.insert(CString::new("command").unwrap(), Message::Int64(42));
            let resp = handle_command(&dict, 501);
            assert_eq!(get_status(&resp), "error");
        }

        #[test]
        fn success_response_has_correct_format() {
            let resp = success_response();
            assert_eq!(get_status(&resp), "success");
        }

        #[test]
        fn error_response_strips_null_bytes() {
            let resp = error_response("bad\0data");
            assert_eq!(get_message_field(&resp), "baddata");
        }
    }
}
