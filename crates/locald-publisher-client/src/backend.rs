use std::io::{Read as _, Write as _};
use std::net::Shutdown;
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use locald_core::{IpcRequest, IpcResponse};
use locald_publisher_protocol::{
    AbsolutePath, EncodedRequestFrame, ProjectInstanceId, PublishedEndpointProtocolInfo,
};
use nix::errno::Errno;
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::socket::{
    AddressFamily, SockFlag, SockType, UnixAddr, connect, getsockopt, socket, sockopt::SocketError,
};
use thiserror::Error;

const COMMAND_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_COMMAND_RESPONSE_BYTES: usize = locald_core::ipc::MAX_IPC_REQUEST_BYTES;

/// Coarse backend failure class retained across discovery implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendErrorKind {
    /// The endpoint could not be reached.
    Unreachable,
    /// Peer authentication failed.
    Authentication,
    /// The daemon is reachable but has not activated publisher transport.
    ProtocolUnavailable,
    /// Framing, version, or response validation failed.
    Protocol,
    /// Another local I/O operation failed.
    Io,
}

/// What the transport can prove about delivery of a failed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCertainty {
    /// No request byte reached the daemon.
    NotSent,
    /// The request may have committed, but no complete authenticated response arrived.
    OutcomeUnknown,
    /// Retrying the request cannot recover this transport or authentication failure.
    Fatal,
}

/// One publisher-transport failure with delivery certainty preserved.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{certainty:?}: {error}")]
pub struct TransportFailure {
    /// Whether exact request replay is required to resolve the outcome.
    pub certainty: DeliveryCertainty,
    /// Redaction-friendly underlying failure.
    pub error: BackendError,
}

impl TransportFailure {
    /// Construct a transport failure.
    #[must_use]
    pub const fn new(certainty: DeliveryCertainty, error: BackendError) -> Self {
        Self { certainty, error }
    }
}

/// A redaction-friendly failure returned by an authenticated backend.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind:?}: {message}")]
pub struct BackendError {
    /// Stable coarse failure class.
    pub kind: BackendErrorKind,
    /// Actionable explanation that must not contain handles or private endpoints.
    pub message: String,
}

impl BackendError {
    /// Construct a backend failure.
    #[must_use]
    pub fn new(kind: BackendErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// One authenticated ordinary-IPC value and its kernel-verified daemon UID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedValue<T> {
    /// Kernel-verified UID of the daemon peer.
    pub peer_uid: u32,
    /// Typed daemon response.
    pub value: T,
}

/// Authenticated ordinary-daemon IPC needed before publication begins.
pub trait AuthenticatedDaemonDiscovery: Send + Sync + std::fmt::Debug {
    /// Read exact protocol info without requiring a project locator.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] when the command socket cannot be reached,
    /// authenticated, decoded, or shown to support publisher discovery.
    fn protocol_info(
        &self,
        command_socket: &AbsolutePath,
    ) -> Result<AuthenticatedValue<PublishedEndpointProtocolInfo>, BackendError>;

    /// Resolve the daemon-observed physical instance for an absolute locator.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] when authenticated project resolution fails.
    fn resolve_project(
        &self,
        command_socket: &AbsolutePath,
        project_locator: &AbsolutePath,
    ) -> Result<AuthenticatedValue<ProjectInstanceId>, BackendError>;
}

/// Bounded, same-UID ordinary command-socket discovery implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnixCommandSocketDiscovery;

impl AuthenticatedDaemonDiscovery for UnixCommandSocketDiscovery {
    fn protocol_info(
        &self,
        command_socket: &AbsolutePath,
    ) -> Result<AuthenticatedValue<PublishedEndpointProtocolInfo>, BackendError> {
        let (peer_uid, response) = exchange_command(
            command_socket,
            &IpcRequest::GetPublishedEndpointProtocolInfo,
        )?;
        match response {
            IpcResponse::PublishedEndpointProtocolInfo(info) => {
                info.validate().map_err(|error| {
                    BackendError::new(
                        BackendErrorKind::Protocol,
                        format!("daemon returned incompatible publisher policy: {error}"),
                    )
                })?;
                Ok(AuthenticatedValue {
                    peer_uid,
                    value: info,
                })
            }
            IpcResponse::PublishedEndpointProtocolUnavailable => Err(BackendError::new(
                BackendErrorKind::ProtocolUnavailable,
                "locald is not advertising the complete publisher transport; upgrade or restart locald",
            )),
            IpcResponse::Error(message) => Err(BackendError::new(
                BackendErrorKind::Protocol,
                format!("locald rejected publisher discovery: {message}"),
            )),
            _ => Err(BackendError::new(
                BackendErrorKind::Protocol,
                "locald returned an unexpected publisher discovery response variant",
            )),
        }
    }

    fn resolve_project(
        &self,
        command_socket: &AbsolutePath,
        project_locator: &AbsolutePath,
    ) -> Result<AuthenticatedValue<ProjectInstanceId>, BackendError> {
        let (peer_uid, response) = exchange_command(
            command_socket,
            &IpcRequest::ResolvePublishedEndpointProject {
                project_locator: project_locator.to_path_buf(),
            },
        )?;
        match response {
            IpcResponse::PublishedEndpointProject(instance) => {
                let value = instance.to_string().parse().map_err(|error| {
                    BackendError::new(
                        BackendErrorKind::Protocol,
                        format!("daemon returned an invalid project instance identity: {error}"),
                    )
                })?;
                Ok(AuthenticatedValue { peer_uid, value })
            }
            IpcResponse::Error(message) => Err(BackendError::new(
                BackendErrorKind::Protocol,
                format!("locald could not resolve the publisher project: {message}"),
            )),
            _ => Err(BackendError::new(
                BackendErrorKind::Protocol,
                "locald returned an unexpected project-resolution response variant",
            )),
        }
    }
}

fn exchange_command(
    command_socket: &AbsolutePath,
    request: &IpcRequest,
) -> Result<(u32, IpcResponse), BackendError> {
    let deadline = Instant::now()
        .checked_add(COMMAND_EXCHANGE_TIMEOUT)
        .ok_or_else(|| BackendError::new(BackendErrorKind::Io, "command deadline overflowed"))?;
    let mut stream = connect_before(command_socket, deadline)?;
    let peer_uid = authenticate_peer_uid(&stream)?;
    let request = serde_json::to_vec(request).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Protocol,
            format!("cannot encode locald discovery request: {error}"),
        )
    })?;
    if request.len() > MAX_COMMAND_RESPONSE_BYTES {
        return Err(BackendError::new(
            BackendErrorKind::Protocol,
            "locald discovery request exceeds the command IPC bound",
        ));
    }
    write_all_before(&mut stream, &request, deadline)?;
    stream.shutdown(Shutdown::Write).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Unreachable,
            format!("cannot finish locald discovery request: {error}"),
        )
    })?;

    let mut response = Vec::new();
    loop {
        let mut chunk = [0_u8; 4096];
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for(stream.as_fd(), PollFlags::POLLIN, deadline)?;
                continue;
            }
            Err(error) => {
                return Err(BackendError::new(
                    BackendErrorKind::Unreachable,
                    format!("cannot read locald discovery response: {error}"),
                ));
            }
        };
        if count == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..count]);
        if response.len() > MAX_COMMAND_RESPONSE_BYTES {
            return Err(BackendError::new(
                BackendErrorKind::Protocol,
                "locald discovery response exceeds the command IPC bound",
            ));
        }
    }
    if response.is_empty() {
        return Err(BackendError::new(
            BackendErrorKind::Unreachable,
            "locald closed discovery without a response",
        ));
    }
    serde_json::from_slice(&response)
        .map(|response| (peer_uid, response))
        .map_err(|error| {
            BackendError::new(
                BackendErrorKind::Protocol,
                format!("cannot decode locald discovery response: {error}"),
            )
        })
}

fn connect_before(
    command_socket: &AbsolutePath,
    deadline: Instant,
) -> Result<UnixStream, BackendError> {
    let descriptor = socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::empty(),
        None,
    )
    .map_err(|error| {
        BackendError::new(
            BackendErrorKind::Io,
            format!("cannot create locald command socket: {error}"),
        )
    })?;
    fcntl(&descriptor, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Io,
            format!("cannot make locald command socket close-on-exec: {error}"),
        )
    })?;
    fcntl(&descriptor, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Io,
            format!("cannot make locald command socket nonblocking: {error}"),
        )
    })?;
    let address = UnixAddr::new(command_socket.as_path()).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Protocol,
            format!("invalid locald command socket path: {error}"),
        )
    })?;
    match connect(descriptor.as_raw_fd(), &address) {
        Ok(()) | Err(Errno::EISCONN) => {}
        Err(Errno::EINPROGRESS | Errno::EALREADY | Errno::EAGAIN) => {
            wait_for(descriptor.as_fd(), PollFlags::POLLOUT, deadline)?;
            let socket_error = getsockopt(&descriptor, SocketError).map_err(|error| {
                BackendError::new(
                    BackendErrorKind::Unreachable,
                    format!("cannot inspect locald command connection: {error}"),
                )
            })?;
            if socket_error != 0 {
                return Err(BackendError::new(
                    BackendErrorKind::Unreachable,
                    format!(
                        "cannot connect to locald command socket: {}",
                        std::io::Error::from_raw_os_error(socket_error)
                    ),
                ));
            }
        }
        Err(error) => {
            return Err(BackendError::new(
                BackendErrorKind::Unreachable,
                format!("cannot connect to locald command socket: {error}"),
            ));
        }
    }
    Ok(UnixStream::from(descriptor))
}

fn write_all_before(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), BackendError> {
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(BackendError::new(
                    BackendErrorKind::Unreachable,
                    "locald closed discovery while receiving the request",
                ));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for(stream.as_fd(), PollFlags::POLLOUT, deadline)?;
            }
            Err(error) => {
                return Err(BackendError::new(
                    BackendErrorKind::Unreachable,
                    format!("cannot send locald discovery request: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn wait_for(
    descriptor: BorrowedFd<'_>,
    events: PollFlags,
    deadline: Instant,
) -> Result<(), BackendError> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                BackendError::new(
                    BackendErrorKind::Unreachable,
                    "locald command discovery exceeded its 5-second deadline",
                )
            })?;
        let timeout = PollTimeout::try_from(remaining).unwrap_or(PollTimeout::MAX);
        let mut descriptors = [PollFd::new(descriptor, events)];
        match poll(&mut descriptors, timeout) {
            Ok(0) => {
                return Err(BackendError::new(
                    BackendErrorKind::Unreachable,
                    "locald command discovery exceeded its 5-second deadline",
                ));
            }
            Ok(_) => {
                let revents = descriptors[0].revents().unwrap_or(PollFlags::POLLNVAL);
                if revents.contains(PollFlags::POLLNVAL) {
                    return Err(BackendError::new(
                        BackendErrorKind::Io,
                        "locald command descriptor became invalid",
                    ));
                }
                return Ok(());
            }
            Err(Errno::EINTR) => {}
            Err(error) => {
                return Err(BackendError::new(
                    BackendErrorKind::Io,
                    format!("cannot wait for locald command I/O: {error}"),
                ));
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn authenticate_peer_uid(stream: &UnixStream) -> Result<u32, BackendError> {
    let (uid, _) = nix::unistd::getpeereid(stream).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Authentication,
            format!("cannot authenticate locald command peer: {error}"),
        )
    })?;
    Ok(uid.as_raw())
}

#[cfg(target_os = "linux")]
fn authenticate_peer_uid(stream: &UnixStream) -> Result<u32, BackendError> {
    let credentials =
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials).map_err(
            |error| {
                BackendError::new(
                    BackendErrorKind::Authentication,
                    format!("cannot authenticate locald command peer: {error}"),
                )
            },
        )?;
    Ok(credentials.uid())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn authenticate_peer_uid(_stream: &UnixStream) -> Result<u32, BackendError> {
    Err(BackendError::new(
        BackendErrorKind::Authentication,
        "same-UID locald command authentication is unsupported on this platform",
    ))
}

/// Result of exactly one publisher-socket request/response connection.
#[derive(Clone, PartialEq, Eq)]
pub struct TransportReply {
    /// Kernel-verified UID of the publisher-socket peer.
    pub peer_uid: u32,
    /// Complete length-prefixed response frame.
    pub response_frame: Vec<u8>,
}

impl std::fmt::Debug for TransportReply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransportReply")
            .field("peer_uid", &self.peer_uid)
            .field(
                "response_frame",
                &format_args!("<redacted; {} bytes>", self.response_frame.len()),
            )
            .finish()
    }
}

/// Dedicated publisher transport abstraction.
///
/// Each invocation opens one connection, sends exactly one complete request,
/// receives exactly one complete response, and closes. A listener is present
/// exactly when `request.descriptor()` requires it. The Unix implementation
/// added with socket activation will transfer that borrowed descriptor via
/// `SCM_RIGHTS`; it must never encode a raw descriptor or port in JSON.
pub trait PublisherTransport: Send + Sync + std::fmt::Debug {
    /// Exchange one strict frame with the authenticated publisher socket.
    ///
    /// # Errors
    ///
    /// Returns [`TransportFailure`] with the strongest known delivery
    /// certainty when the request cannot produce an authenticated response.
    fn exchange(
        &self,
        publisher_socket: &AbsolutePath,
        request: &EncodedRequestFrame,
        listener: Option<BorrowedFd<'_>>,
    ) -> Result<TransportReply, TransportFailure>;
}
