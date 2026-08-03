//! Authenticated macOS privileged-helper client.

use anyhow::{Context, Result};
use locald_helper_protocol::{
    HelperCommand, HelperErrorCode, HelperStatus, MACH_SERVICE, PROTOCOL_VERSION,
    is_supported_bind_port, validate_complete_host_set,
};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd, RawFd};
use tracing::info;
use xpc_connection::Message;

/// Ask the helper to bind one of locald's privileged listener ports.
pub async fn bind_privileged_port(port: u16) -> Result<std::net::TcpListener> {
    if !is_supported_bind_port(port) {
        anyhow::bail!("privileged helper only supports ports 80 and 443 (requested {port})");
    }
    info!("Requesting privileged port {port} from helper via XPC...");

    let fd = tokio::task::spawn_blocking(move || xpc_request(&HelperRequest::Bind(port)))
        .await
        .context("XPC helper task panicked")??
        .context("helper bind response did not include a listener")?;

    // SAFETY: xpc-connection duplicated this descriptor from the authenticated helper response.
    #[allow(unsafe_code)]
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    set_cloexec(&fd).context("Failed to set FD_CLOEXEC on helper listener")?;
    let fd = fd.into_raw_fd();

    // SAFETY: ownership transfers from OwnedFd into TcpListener exactly once.
    #[allow(unsafe_code)]
    let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
    info!("Acquired port {port} from helper");
    Ok(listener)
}

/// Verify helper reachability, caller authorization, and protocol agreement.
pub async fn probe_helper() -> Result<()> {
    tokio::task::spawn_blocking(|| xpc_request(&HelperRequest::Probe))
        .await
        .context("XPC helper probe task panicked")??;
    Ok(())
}

/// Atomically replace the complete locald-managed hosts-file section.
///
/// An empty set removes the managed section. Callers must supply canonical,
/// sorted, duplicate-free exact hostnames. The helper independently repeats
/// this validation before performing the privileged mutation.
pub async fn sync_hosts(domains: Vec<String>) -> Result<()> {
    validate_complete_host_set(&domains)
        .context("refused invalid complete host set before contacting privileged helper")?;
    tokio::task::spawn_blocking(move || xpc_request(&HelperRequest::SyncHosts(domains)))
        .await
        .context("XPC helper hosts-sync task panicked")??;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HelperRequest {
    Probe,
    Bind(u16),
    SyncHosts(Vec<String>),
}

impl HelperRequest {
    #[must_use]
    const fn command(&self) -> HelperCommand {
        match self {
            Self::Probe => HelperCommand::Probe,
            Self::Bind(_) => HelperCommand::Bind,
            Self::SyncHosts(_) => HelperCommand::SyncHosts,
        }
    }
}

fn set_cloexec(fd: &OwnedFd) -> Result<()> {
    let flags = nix::fcntl::fcntl(fd, nix::fcntl::F_GETFD).context("Failed to get FD flags")?;
    nix::fcntl::fcntl(
        fd,
        nix::fcntl::F_SETFD(
            nix::fcntl::FdFlag::from_bits_truncate(flags) | nix::fcntl::FdFlag::FD_CLOEXEC,
        ),
    )
    .context("Failed to set FD_CLOEXEC")?;
    Ok(())
}

fn xpc_request(request: &HelperRequest) -> Result<Option<RawFd>> {
    use futures::stream::StreamExt;
    use xpc_connection::XpcClient;

    #[allow(clippy::expect_used)]
    let name = CString::new(MACH_SERVICE).expect("static CString");
    let mut client = XpcClient::connect(&name);
    let command = request.command();
    client.send_message(request_message(request));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime for XPC")?;
    let response = runtime
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), client.next())
                .await
                .context("Timed out waiting for helper response")?
                .context("Helper connection closed without response")
        })
        .map_err(|error| helper_unavailable(&error))?;

    parse_response(command, &response)
}

fn request_message(request: &HelperRequest) -> Message {
    let mut dict = HashMap::new();
    insert_int(&mut dict, "protocol_version", i64::from(PROTOCOL_VERSION));
    insert_string(&mut dict, "command", request.command().as_str());
    match request {
        HelperRequest::Probe => {}
        HelperRequest::Bind(port) => insert_int(&mut dict, "port", i64::from(*port)),
        HelperRequest::SyncHosts(domains) => {
            insert_strings(&mut dict, "domains", domains);
        }
    }
    Message::Dictionary(dict)
}

fn parse_response(command: HelperCommand, response: &Message) -> Result<Option<RawFd>> {
    let Message::Dictionary(dict) = response else {
        return Err(setup_required("helper returned an unexpected XPC response"));
    };

    let protocol_version = response_int(dict, "protocol_version")
        .ok_or_else(|| setup_required("helper response omitted protocol_version"))?;
    if protocol_version != i64::from(PROTOCOL_VERSION) {
        return Err(setup_required(&format!(
            "helper protocol mismatch (client {PROTOCOL_VERSION}, helper {protocol_version})"
        )));
    }

    let status = response_string(dict, "status")
        .ok_or_else(|| setup_required("helper response omitted status"))?;
    if status == HelperStatus::Error.as_str() {
        let raw_code = response_string(dict, "error_code")
            .unwrap_or_else(|| HelperErrorCode::InternalError.as_str().to_string());
        let code = HelperErrorCode::parse(&raw_code).unwrap_or(HelperErrorCode::InternalError);
        let message = response_string(dict, "message").unwrap_or_else(|| "no detail".to_string());
        return Err(helper_rejection(command, code, &raw_code, &message));
    }
    if status != HelperStatus::Success.as_str() {
        return Err(setup_required(&format!(
            "helper returned unknown status {status:?}"
        )));
    }

    match command {
        HelperCommand::Probe | HelperCommand::SyncHosts => Ok(None),
        HelperCommand::Bind => response_fd(dict, "fd")
            .map(Some)
            .ok_or_else(|| setup_required("helper bind response omitted fd")),
    }
}

fn setup_required(detail: &str) -> anyhow::Error {
    anyhow::anyhow!("{detail}. Run `sudo locald admin setup` to repair the installation.")
}

fn helper_unavailable(error: &anyhow::Error) -> anyhow::Error {
    setup_required(&format!("privileged helper is unavailable: {error}"))
}

fn helper_rejection(
    command: HelperCommand,
    code: HelperErrorCode,
    raw_code: &str,
    message: &str,
) -> anyhow::Error {
    let detail = format!(
        "helper rejected {} ({}): {message}",
        command.as_str(),
        raw_code
    );
    match code {
        HelperErrorCode::ProtocolMismatch
        | HelperErrorCode::AuthorityUnavailable
        | HelperErrorCode::AuthorityInvalid
        | HelperErrorCode::AuthenticationFailed => setup_required(&detail),
        HelperErrorCode::RootBindDenied | HelperErrorCode::RootHostsMutationDenied => {
            anyhow::anyhow!("{detail}. Run locald without sudo as the configured user.")
        }
        HelperErrorCode::CallerUserMismatch => anyhow::anyhow!(
            "{detail}. Run locald as the configured user, or rerun `sudo locald admin setup` from the intended console-user session."
        ),
        HelperErrorCode::ConsoleUserMismatch => anyhow::anyhow!(
            "{detail}. Run locald from the configured user's active console session."
        ),
        HelperErrorCode::InvalidRequest
        | HelperErrorCode::UnknownCommand
        | HelperErrorCode::UnsupportedPort
        | HelperErrorCode::BindFailed
        | HelperErrorCode::InvalidHostSet
        | HelperErrorCode::HostsReadFailed
        | HelperErrorCode::HostsWriteFailed
        | HelperErrorCode::InternalError => anyhow::anyhow!(detail),
    }
}

fn insert_string(dict: &mut HashMap<CString, Message>, key: &str, value: &str) {
    #[allow(clippy::expect_used)]
    dict.insert(
        CString::new(key).expect("static CString"),
        Message::String(CString::new(value).expect("static CString")),
    );
}

fn insert_int(dict: &mut HashMap<CString, Message>, key: &str, value: i64) {
    #[allow(clippy::expect_used)]
    dict.insert(
        CString::new(key).expect("static CString"),
        Message::Int64(value),
    );
}

fn insert_strings(dict: &mut HashMap<CString, Message>, key: &str, values: &[String]) {
    #[allow(clippy::expect_used)]
    dict.insert(
        CString::new(key).expect("static CString"),
        Message::Array(
            values
                .iter()
                .map(|value| {
                    Message::String(
                        CString::new(value.as_str()).expect("validated hostname CString"),
                    )
                })
                .collect(),
        ),
    );
}

fn response_string(dict: &HashMap<CString, Message>, field: &str) -> Option<String> {
    let key = CString::new(field).ok()?;
    match dict.get(&key) {
        Some(Message::String(value)) => Some(value.to_string_lossy().to_string()),
        _ => None,
    }
}

fn response_int(dict: &HashMap<CString, Message>, field: &str) -> Option<i64> {
    let key = CString::new(field).ok()?;
    match dict.get(&key) {
        Some(Message::Int64(value)) => Some(*value),
        _ => None,
    }
}

fn response_fd(dict: &HashMap<CString, Message>, field: &str) -> Option<RawFd> {
    let key = CString::new(field).ok()?;
    match dict.get(&key) {
        Some(Message::Fd(fd)) => Some(*fd),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn response(status: HelperStatus) -> HashMap<CString, Message> {
        let mut dict = HashMap::new();
        insert_int(&mut dict, "protocol_version", i64::from(PROTOCOL_VERSION));
        insert_string(&mut dict, "status", status.as_str());
        dict
    }

    #[test]
    fn requests_always_include_protocol_and_supported_command() {
        let Message::Dictionary(probe) = request_message(&HelperRequest::Probe) else {
            panic!("probe request must be a dictionary");
        };
        assert_eq!(
            response_int(&probe, "protocol_version"),
            Some(i64::from(PROTOCOL_VERSION))
        );
        assert_eq!(response_string(&probe, "command").as_deref(), Some("probe"));
        assert_eq!(response_int(&probe, "port"), None);

        let Message::Dictionary(bind) = request_message(&HelperRequest::Bind(443)) else {
            panic!("bind request must be a dictionary");
        };
        assert_eq!(response_string(&bind, "command").as_deref(), Some("bind"));
        assert_eq!(response_int(&bind, "port"), Some(443));

        let Message::Dictionary(sync) = request_message(&HelperRequest::SyncHosts(vec![
            "api.example.test".to_owned(),
            "web.example.test".to_owned(),
        ])) else {
            panic!("sync-hosts request must be a dictionary");
        };
        assert_eq!(
            response_string(&sync, "command").as_deref(),
            Some("sync_hosts")
        );
        let domains_key = CString::new("domains").expect("static");
        let Some(Message::Array(domains)) = sync.get(&domains_key) else {
            panic!("sync-hosts request must carry a domain array");
        };
        assert_eq!(domains.len(), 2);
    }

    #[test]
    fn probe_accepts_current_success_response_without_fd() {
        let message = Message::Dictionary(response(HelperStatus::Success));
        assert_eq!(
            parse_response(HelperCommand::Probe, &message).expect("probe success"),
            None
        );
        assert_eq!(
            parse_response(HelperCommand::SyncHosts, &message).expect("host sync success"),
            None
        );
    }

    #[test]
    fn wrong_or_missing_protocol_requires_setup() {
        let mut wrong = response(HelperStatus::Success);
        insert_int(
            &mut wrong,
            "protocol_version",
            i64::from(PROTOCOL_VERSION + 1),
        );
        let error = parse_response(HelperCommand::Probe, &Message::Dictionary(wrong))
            .expect_err("wrong protocol must fail");
        assert!(error.to_string().contains("protocol mismatch"));
        assert!(error.to_string().contains("sudo locald admin setup"));

        let mut missing = response(HelperStatus::Success);
        missing.remove(&CString::new("protocol_version").expect("static"));
        assert!(
            parse_response(HelperCommand::Probe, &Message::Dictionary(missing))
                .expect_err("missing protocol must fail")
                .to_string()
                .contains("sudo locald admin setup")
        );
    }

    #[test]
    fn authentication_error_is_actionable() {
        let mut dict = response(HelperStatus::Error);
        insert_string(
            &mut dict,
            "error_code",
            HelperErrorCode::AuthenticationFailed.as_str(),
        );
        insert_string(&mut dict, "message", "code requirement mismatch");
        let error = parse_response(HelperCommand::Probe, &Message::Dictionary(dict))
            .expect_err("authentication must fail");
        assert!(error.to_string().contains("authentication_failed"));
        assert!(error.to_string().contains("sudo locald admin setup"));
    }

    #[test]
    fn unknown_error_code_is_preserved_for_diagnostics() {
        let mut dict = response(HelperStatus::Error);
        insert_string(&mut dict, "error_code", "future_policy_failure");
        insert_string(&mut dict, "message", "future helper detail");

        let error = parse_response(HelperCommand::Probe, &Message::Dictionary(dict))
            .expect_err("unknown helper error code must fail");

        assert!(error.to_string().contains("future_policy_failure"));
        assert!(error.to_string().contains("future helper detail"));
    }

    #[test]
    fn unavailable_helper_requires_setup() {
        let error = helper_unavailable(&anyhow::anyhow!("connection closed"));
        assert!(error.to_string().contains("connection closed"));
        assert!(error.to_string().contains("sudo locald admin setup"));
    }

    #[test]
    fn policy_and_mutation_errors_have_specific_remediation() {
        for (code, expected, excludes_setup) in [
            (
                HelperErrorCode::RootBindDenied,
                "without sudo as the configured user",
                true,
            ),
            (
                HelperErrorCode::CallerUserMismatch,
                "intended console-user session",
                false,
            ),
            (
                HelperErrorCode::ConsoleUserMismatch,
                "active console session",
                true,
            ),
            (HelperErrorCode::BindFailed, "address already in use", true),
        ] {
            let mut dict = response(HelperStatus::Error);
            insert_string(&mut dict, "error_code", code.as_str());
            insert_string(&mut dict, "message", "address already in use");
            let error = parse_response(HelperCommand::Bind, &Message::Dictionary(dict))
                .expect_err("helper rejection must fail");
            assert!(error.to_string().contains(expected));
            if excludes_setup {
                assert!(!error.to_string().contains("sudo locald admin setup"));
            }
        }
    }

    #[test]
    fn invalid_complete_host_set_is_rejected_before_xpc() {
        let error =
            validate_complete_host_set(&["z.example.test".to_owned(), "a.example.test".to_owned()])
                .expect_err("out-of-order complete set must fail");
        assert!(error.to_string().contains("sorted and unique"));

        let mut root = response(HelperStatus::Error);
        insert_string(
            &mut root,
            "error_code",
            HelperErrorCode::RootHostsMutationDenied.as_str(),
        );
        insert_string(&mut root, "message", "root cannot mutate managed hosts");
        let error = parse_response(HelperCommand::SyncHosts, &Message::Dictionary(root))
            .expect_err("root host mutation must fail");
        assert!(error.to_string().contains("without sudo"));

        for code in [
            HelperErrorCode::InvalidHostSet,
            HelperErrorCode::HostsReadFailed,
            HelperErrorCode::HostsWriteFailed,
        ] {
            let mut dict = response(HelperStatus::Error);
            insert_string(&mut dict, "error_code", code.as_str());
            insert_string(&mut dict, "message", "injected host operation failure");
            let error = parse_response(HelperCommand::SyncHosts, &Message::Dictionary(dict))
                .expect_err("host operation rejection must fail");
            assert!(error.to_string().contains(code.as_str()));
            assert!(
                error
                    .to_string()
                    .contains("injected host operation failure")
            );
        }
    }
}
