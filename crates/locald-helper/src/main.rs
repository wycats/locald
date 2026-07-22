//! macOS privileged helper daemon for locald.
//!
//! Runs as a `LaunchDaemon` and performs the one operation that requires
//! persistent privilege: binding locald's listeners on ports 80 and 443.

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
    use locald_helper_protocol::code_signing;
    use locald_helper_protocol::{
        AUTHORITY_PATH, AuthorityError, HelperAuthority, HelperCommand, HelperErrorCode,
        HelperStatus, MACH_SERVICE, PROTOCOL_VERSION, is_supported_bind_port, load_authority,
    };
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::io::RawFd;
    use std::path::Path;
    use std::sync::Arc;
    use xpc_connection::{Message, XpcClient, XpcListener};

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let authority =
            load_and_validate_authority().map_err(|(_, message)| anyhow::anyhow!(message))?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(serve(Arc::new(authority)))?;
        Ok(())
    }

    fn load_and_validate_authority()
    -> std::result::Result<HelperAuthority, (HelperErrorCode, String)> {
        let authority = load_authority(Path::new(AUTHORITY_PATH)).map_err(|error| {
            let code = authority_error_code(&error);
            let state = if code == HelperErrorCode::AuthorityUnavailable {
                "unavailable"
            } else {
                "invalid"
            };
            (
                code,
                format!("helper authority is {state}: {error}; run `sudo locald admin setup`"),
            )
        })?;
        code_signing::validate_requirement(&authority.designated_requirement).map_err(|error| {
            (
                HelperErrorCode::AuthorityInvalid,
                format!(
                    "helper authority contains an invalid code requirement: {error}; run `sudo locald admin setup`"
                ),
            )
        })?;
        Ok(authority)
    }

    const fn authority_error_code(error: &AuthorityError) -> HelperErrorCode {
        match error {
            AuthorityError::Io(_) => HelperErrorCode::AuthorityUnavailable,
            _ => HelperErrorCode::AuthorityInvalid,
        }
    }

    #[allow(clippy::future_not_send)]
    async fn serve(authority: Arc<HelperAuthority>) -> Result<()> {
        #[allow(clippy::expect_used)]
        let name = CString::new(MACH_SERVICE).expect("static CString");
        let mut listener = XpcListener::listen(&name);

        while let Some(client) = listener.next().await {
            tokio::spawn(handle_client(client, Arc::clone(&authority)));
        }

        Ok(())
    }

    #[allow(clippy::future_not_send)]
    async fn handle_client(mut client: XpcClient, startup_authority: Arc<HelperAuthority>) {
        let token = client.audit_token();
        let caller_uid = audit_token_euid(&token);
        if let Err(error) = authenticate_caller(&startup_authority, &token) {
            client.send_message(error_response(
                HelperErrorCode::AuthenticationFailed,
                &format!("caller authentication failed: {error}"),
            ));
            return;
        }

        loop {
            match client.next().await {
                None | Some(Message::Error(_)) => break,
                Some(Message::Dictionary(dict)) => {
                    let console_uid = console_owner_uid();
                    let response = match load_and_validate_authority() {
                        Ok(authority) => match authenticate_caller(&authority, &token) {
                            Ok(()) => handle_command(&dict, &authority, caller_uid, &console_uid),
                            Err(error) => CommandResponse::message(error_response(
                                HelperErrorCode::AuthenticationFailed,
                                &format!("caller authentication failed: {error}"),
                            )),
                        },
                        Err((code, message)) => {
                            CommandResponse::message(error_response(code, &message))
                        }
                    };
                    send_response(&client, response);
                }
                Some(_) => {
                    client.send_message(error_response(
                        HelperErrorCode::InvalidRequest,
                        "expected dictionary message",
                    ));
                }
            }
        }
    }

    fn authenticate_caller(authority: &HelperAuthority, token: &[u8; 32]) -> Result<()> {
        code_signing::audit_token_satisfies_requirement(token, &authority.designated_requirement)
            .context("code requirement mismatch")
    }

    const fn audit_token_euid(token: &[u8; 32]) -> u32 {
        // audit_token_t is eight native-endian u32 values. Effective UID is field 1.
        u32::from_ne_bytes([token[4], token[5], token[6], token[7]])
    }

    fn console_owner_uid() -> Result<u32> {
        let metadata =
            std::fs::metadata("/dev/console").context("could not inspect /dev/console")?;
        Ok(metadata.uid())
    }

    #[cfg_attr(test, derive(Debug))]
    struct CommandResponse {
        message: Message,
        close_fd: Option<RawFd>,
    }

    impl CommandResponse {
        const fn message(message: Message) -> Self {
            Self {
                message,
                close_fd: None,
            }
        }

        const fn transferred_fd(message: Message, fd: RawFd) -> Self {
            Self {
                message,
                close_fd: Some(fd),
            }
        }
    }

    fn close_transferred_fd(fd: RawFd) {
        if let Err(error) = nix::unistd::close(fd) {
            tracing::warn!("failed to close transferred fd {fd}: {error}");
        }
    }

    fn send_response(client: &XpcClient, response: CommandResponse) {
        send_response_with(response, |message| client.send_message(message));
    }

    fn send_response_with(response: CommandResponse, send: impl FnOnce(Message)) {
        let CommandResponse { message, close_fd } = response;
        send(message);
        if let Some(fd) = close_fd {
            close_transferred_fd(fd);
        }
    }

    fn handle_command(
        dict: &HashMap<CString, Message>,
        authority: &HelperAuthority,
        caller_uid: u32,
        console_uid: &Result<u32>,
    ) -> CommandResponse {
        let protocol_version = match int_field(dict, "protocol_version") {
            Ok(version) => version,
            Err(message) => {
                return CommandResponse::message(error_response(
                    HelperErrorCode::InvalidRequest,
                    &message,
                ));
            }
        };
        if protocol_version != i64::from(PROTOCOL_VERSION) {
            return CommandResponse::message(error_response(
                HelperErrorCode::ProtocolMismatch,
                &format!("helper protocol version {PROTOCOL_VERSION} required"),
            ));
        }

        let command_name = match string_field(dict, "command") {
            Ok(command) => command,
            Err(message) => {
                return CommandResponse::message(error_response(
                    HelperErrorCode::InvalidRequest,
                    &message,
                ));
            }
        };
        let Some(command) = HelperCommand::parse(&command_name) else {
            return CommandResponse::message(error_response(
                HelperErrorCode::UnknownCommand,
                &format!("unknown command: {command_name}"),
            ));
        };

        if let Err((code, message)) =
            authorize_command(authority.console_user_uid, caller_uid, console_uid, command)
        {
            return CommandResponse::message(error_response(code, message));
        }

        match command {
            HelperCommand::Probe => CommandResponse::message(success_response()),
            HelperCommand::Bind => execute_bind(dict),
        }
    }

    const fn authorize_command(
        configured_uid: u32,
        caller_uid: u32,
        console_uid: &Result<u32>,
        command: HelperCommand,
    ) -> Result<(), (HelperErrorCode, &'static str)> {
        match command {
            HelperCommand::Probe if caller_uid == 0 || caller_uid == configured_uid => Ok(()),
            HelperCommand::Probe => Err((
                HelperErrorCode::CallerUserMismatch,
                "probe caller is not the configured locald user",
            )),
            HelperCommand::Bind if caller_uid == 0 => Err((
                HelperErrorCode::RootBindDenied,
                "root is not authorized to bind locald listener ports",
            )),
            HelperCommand::Bind if caller_uid != configured_uid => Err((
                HelperErrorCode::CallerUserMismatch,
                "bind caller is not the configured locald user",
            )),
            HelperCommand::Bind => match console_uid {
                Ok(uid) if *uid == configured_uid => Ok(()),
                Ok(_) => Err((
                    HelperErrorCode::ConsoleUserMismatch,
                    "configured locald user does not own /dev/console",
                )),
                Err(_) => Err((
                    HelperErrorCode::ConsoleUserMismatch,
                    "could not inspect /dev/console to verify configured-user ownership",
                )),
            },
        }
    }

    fn string_field(dict: &HashMap<CString, Message>, name: &str) -> Result<String, String> {
        let key = CString::new(name).map_err(|_| format!("invalid field name {name}"))?;
        match dict.get(&key) {
            Some(Message::String(value)) => Ok(value.to_string_lossy().to_string()),
            _ => Err(format!("missing or invalid '{name}' field")),
        }
    }

    fn int_field(dict: &HashMap<CString, Message>, name: &str) -> Result<i64, String> {
        let key = CString::new(name).map_err(|_| format!("invalid field name {name}"))?;
        match dict.get(&key) {
            Some(Message::Int64(value)) => Ok(*value),
            _ => Err(format!("missing or invalid '{name}' field")),
        }
    }

    fn execute_bind(dict: &HashMap<CString, Message>) -> CommandResponse {
        let port = match int_field(dict, "port") {
            Ok(port) => match u16::try_from(port) {
                Ok(port) => port,
                Err(_) => {
                    return CommandResponse::message(error_response(
                        HelperErrorCode::UnsupportedPort,
                        "bind port is outside the supported range",
                    ));
                }
            },
            Err(message) => {
                return CommandResponse::message(error_response(
                    HelperErrorCode::InvalidRequest,
                    &message,
                ));
            }
        };

        if !is_supported_bind_port(port) {
            return CommandResponse::message(error_response(
                HelperErrorCode::UnsupportedPort,
                &format!("refused to bind port {port}: only 80 and 443 are allowed"),
            ));
        }

        match bind_privileged_port(port) {
            Ok(fd) => CommandResponse::transferred_fd(success_fd_response(fd), fd),
            Err(error) => CommandResponse::message(error_response(
                HelperErrorCode::BindFailed,
                &format!("{error:#}"),
            )),
        }
    }

    fn bind_privileged_port(port: u16) -> Result<RawFd> {
        use std::os::unix::io::IntoRawFd;

        let address: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .context("failed to create socket")?;
        socket.set_reuse_address(true)?;
        socket
            .bind(&address.into())
            .with_context(|| format!("failed to bind port {port}"))?;
        socket.listen(1024).context("failed to listen")?;
        Ok(socket.into_raw_fd())
    }

    fn success_response() -> Message {
        response_dictionary(HelperStatus::Success, None, None, None)
    }

    fn success_fd_response(fd: RawFd) -> Message {
        response_dictionary(HelperStatus::Success, None, None, Some(fd))
    }

    fn error_response(code: HelperErrorCode, message: &str) -> Message {
        response_dictionary(HelperStatus::Error, Some(code), Some(message), None)
    }

    fn response_dictionary(
        status: HelperStatus,
        code: Option<HelperErrorCode>,
        message: Option<&str>,
        fd: Option<RawFd>,
    ) -> Message {
        let mut dict = HashMap::new();
        insert_int(&mut dict, "protocol_version", i64::from(PROTOCOL_VERSION));
        insert_string(&mut dict, "status", status.as_str());
        if let Some(code) = code {
            insert_string(&mut dict, "error_code", code.as_str());
        }
        if let Some(message) = message {
            insert_string(&mut dict, "message", &message.replace('\0', ""));
        }
        if let Some(fd) = fd {
            #[allow(clippy::expect_used)]
            dict.insert(CString::new("fd").expect("static CString"), Message::Fd(fd));
        }
        Message::Dictionary(dict)
    }

    fn insert_string(dict: &mut HashMap<CString, Message>, key: &str, value: &str) {
        #[allow(clippy::expect_used)]
        dict.insert(
            CString::new(key).expect("static CString"),
            Message::String(CString::new(value).expect("sanitized CString")),
        );
    }

    fn insert_int(dict: &mut HashMap<CString, Message>, key: &str, value: i64) {
        #[allow(clippy::expect_used)]
        dict.insert(
            CString::new(key).expect("static CString"),
            Message::Int64(value),
        );
    }

    #[cfg(test)]
    #[allow(
        clippy::disallowed_methods,
        clippy::expect_used,
        clippy::let_underscore_must_use
    )]
    mod tests {
        use super::*;

        fn authority() -> HelperAuthority {
            HelperAuthority::new(
                501,
                "identifier locald".to_string(),
                "/usr/local/bin/locald".into(),
                "0.1.0".to_string(),
            )
            .expect("valid authority")
        }

        fn command_dict(command: &str) -> HashMap<CString, Message> {
            let mut dict = HashMap::new();
            insert_int(&mut dict, "protocol_version", i64::from(PROTOCOL_VERSION));
            insert_string(&mut dict, "command", command);
            dict
        }

        fn response_string(message: &Message, field: &str) -> Option<String> {
            let Message::Dictionary(dict) = message else {
                return None;
            };
            let key = CString::new(field).expect("test field");
            match dict.get(&key) {
                Some(Message::String(value)) => Some(value.to_string_lossy().to_string()),
                _ => None,
            }
        }

        fn response_int(message: &Message, field: &str) -> Option<i64> {
            let Message::Dictionary(dict) = message else {
                return None;
            };
            let key = CString::new(field).expect("test field");
            match dict.get(&key) {
                Some(Message::Int64(value)) => Some(*value),
                _ => None,
            }
        }

        fn assert_error(response: &CommandResponse, code: HelperErrorCode) {
            assert_eq!(
                response_string(&response.message, "status").as_deref(),
                Some("error")
            );
            assert_eq!(
                response_string(&response.message, "error_code").as_deref(),
                Some(code.as_str())
            );
            assert_eq!(
                response_int(&response.message, "protocol_version"),
                Some(i64::from(PROTOCOL_VERSION))
            );
            assert!(response.close_fd.is_none());
        }

        #[test]
        fn missing_and_wrong_protocol_are_rejected_stably() {
            let mut missing = command_dict("probe");
            missing.remove(&CString::new("protocol_version").expect("static"));
            assert_error(
                &handle_command(&missing, &authority(), 501, &Ok(501)),
                HelperErrorCode::InvalidRequest,
            );

            let mut wrong = command_dict("probe");
            insert_int(&mut wrong, "protocol_version", 2);
            assert_error(
                &handle_command(&wrong, &authority(), 501, &Ok(501)),
                HelperErrorCode::ProtocolMismatch,
            );
            let response = handle_command(&wrong, &authority(), 501, &Ok(501));
            let expected = format!("helper protocol version {PROTOCOL_VERSION} required");
            assert_eq!(
                response_string(&response.message, "message").as_deref(),
                Some(expected.as_str())
            );
        }

        #[test]
        fn unknown_commands_and_unsupported_ports_are_rejected() {
            assert_error(
                &handle_command(&command_dict("setup"), &authority(), 501, &Ok(501)),
                HelperErrorCode::UnknownCommand,
            );

            let mut bind = command_dict("bind");
            insert_int(&mut bind, "port", 3000);
            assert_error(
                &handle_command(&bind, &authority(), 501, &Ok(501)),
                HelperErrorCode::UnsupportedPort,
            );
        }

        #[test]
        fn root_wrong_user_and_console_mismatch_cannot_bind() {
            let mut bind = command_dict("bind");
            insert_int(&mut bind, "port", 80);
            assert_error(
                &handle_command(&bind, &authority(), 0, &Ok(501)),
                HelperErrorCode::RootBindDenied,
            );
            assert_error(
                &handle_command(&bind, &authority(), 502, &Ok(501)),
                HelperErrorCode::CallerUserMismatch,
            );
            assert_error(
                &handle_command(&bind, &authority(), 501, &Ok(502)),
                HelperErrorCode::ConsoleUserMismatch,
            );

            let console_error = Err(anyhow::anyhow!("injected console metadata failure"));
            let response = handle_command(&bind, &authority(), 501, &console_error);
            assert_error(&response, HelperErrorCode::ConsoleUserMismatch);
            assert_eq!(
                response_string(&response.message, "message").as_deref(),
                Some("could not inspect /dev/console to verify configured-user ownership")
            );
        }

        #[test]
        fn configured_user_and_root_probe_policy_is_explicit() {
            let probe = command_dict("probe");
            for caller_uid in [501, 0] {
                let response = handle_command(&probe, &authority(), caller_uid, &Ok(502));
                assert_eq!(
                    response_string(&response.message, "status").as_deref(),
                    Some("success")
                );
            }
            assert_error(
                &handle_command(&probe, &authority(), 502, &Ok(501)),
                HelperErrorCode::CallerUserMismatch,
            );

            assert!(authorize_command(501, 501, &Ok(501), HelperCommand::Bind).is_ok());
        }

        #[test]
        fn audit_token_euid_uses_native_second_field() {
            let mut token = [0_u8; 32];
            token[4..8].copy_from_slice(&501_u32.to_ne_bytes());
            assert_eq!(audit_token_euid(&token), 501);
        }

        #[test]
        fn authority_load_failures_use_stable_error_codes() {
            let missing = AuthorityError::Io(std::io::Error::from(std::io::ErrorKind::NotFound));
            assert_eq!(
                authority_error_code(&missing),
                HelperErrorCode::AuthorityUnavailable
            );
            assert_eq!(
                authority_error_code(&AuthorityError::WrongMode(0o644)),
                HelperErrorCode::AuthorityInvalid
            );
        }

        #[test]
        fn transferred_fd_is_closed_after_send() {
            use std::os::fd::{BorrowedFd, IntoRawFd};

            let path = std::env::temp_dir()
                .join(format!("locald-helper-fd-cleanup-{}", std::process::id()));
            let file = std::fs::File::create(&path).expect("temp file");
            let fd = file.into_raw_fd();
            let response = CommandResponse::transferred_fd(success_fd_response(fd), fd);

            send_response_with(response, |message| {
                assert_eq!(
                    response_string(&message, "status").as_deref(),
                    Some("success")
                );
                #[allow(unsafe_code)]
                let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
                assert!(nix::fcntl::fcntl(borrowed, nix::fcntl::F_GETFD).is_ok());
            });

            let _ = std::fs::remove_file(path);
            assert_eq!(nix::unistd::close(fd), Err(nix::errno::Errno::EBADF));
        }

        #[test]
        fn error_response_strips_null_bytes() {
            let response = error_response(HelperErrorCode::InternalError, "bad\0data");
            assert_eq!(
                response_string(&response, "message").as_deref(),
                Some("baddata")
            );
        }
    }
}
