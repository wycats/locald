use std::io::{IoSlice, Read as _, Write as _};
use std::mem::{size_of, zeroed};
use std::net::Shutdown;
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use locald_core::{IpcRequest, IpcResponse};
#[cfg(target_os = "macos")]
use locald_publisher_protocol::MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES;
use locald_publisher_protocol::{
    AbsolutePath, DescriptorPrelude, EncodedRequestFrame, FRAME_TIMEOUT_MS, MAX_FRAME_JSON_BYTES,
    ProjectInstanceId, PublishedEndpointProtocolInfo,
};
use nix::errno::Errno;
#[cfg(target_os = "macos")]
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::socket::{
    AddressFamily, ControlMessage, MsgFlags, SockFlag, SockType, UnixAddr, connect, getsockopt,
    sendmsg, socket, sockopt::SocketError,
};
use nix::unistd::Uid;
use thiserror::Error;

const COMMAND_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLISHER_REQUEST_TIMEOUT: Duration = Duration::from_millis(FRAME_TIMEOUT_MS);
const MAX_COMMAND_RESPONSE_BYTES: usize = locald_core::ipc::MAX_IPC_REQUEST_BYTES;
const MAX_PUBLISHER_RESPONSE_BYTES: usize = 4 + MAX_FRAME_JSON_BYTES;
const RESPONSE_CONTROL_WORDS: usize = 512;

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
///
/// On macOS, the publisher host must apply the same process-global
/// [`crate::ProcessSpawnBarrier`] contract documented for
/// [`UnixPublisherTransport`].
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
    let descriptor = create_nonblocking_unix_socket_before(deadline).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Io,
            format!("cannot create locald command socket: {error}"),
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

/// Strict production transport for locald's dedicated Unix publisher socket.
///
/// Every exchange uses a fresh close-on-exec, nonblocking connection and one
/// absolute deadline. The first semantic-frame byte carries the complete
/// `SCM_RIGHTS` contract. On macOS, the transport inserts the fixed native
/// audit-token proof after that byte and before the semantic frame's length;
/// Linux sends the [`EncodedRequestFrame`] bytes unchanged. Response ancillary
/// data is never accepted.
///
/// On macOS, every process-spawn or direct-exec path in the publisher host must
/// hold the process-global [`crate::ProcessSpawnBarrier`] spawn permit. The
/// transport takes the exclusive side of that same barrier through successful
/// socket close-on-exec and nonblocking setup, and through disposal of any
/// unexpected descriptors installed while receiving a response.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnixPublisherTransport;

impl PublisherTransport for UnixPublisherTransport {
    fn exchange(
        &self,
        publisher_socket: &AbsolutePath,
        request: &EncodedRequestFrame,
        listener: Option<BorrowedFd<'_>>,
    ) -> Result<TransportReply, TransportFailure> {
        validate_listener_contract(request, listener)?;
        let request_deadline = Instant::now()
            .checked_add(PUBLISHER_REQUEST_TIMEOUT)
            .ok_or_else(|| {
                publisher_failure(
                    DeliveryCertainty::NotSent,
                    BackendErrorKind::Io,
                    "publisher transport deadline overflowed",
                )
            })?;
        let mut stream =
            connect_publisher_before(publisher_socket, request_deadline).map_err(|error| {
                let certainty = if error.kind == BackendErrorKind::Protocol {
                    DeliveryCertainty::Fatal
                } else {
                    DeliveryCertainty::NotSent
                };
                TransportFailure::new(certainty, error)
            })?;
        let peer_uid = authenticate_publisher_peer(&stream)?;

        send_request_before(&mut stream, request, listener, request_deadline)?;
        stream.shutdown(Shutdown::Write).map_err(|error| {
            publisher_failure(
                DeliveryCertainty::OutcomeUnknown,
                BackendErrorKind::Unreachable,
                format!("cannot finish locald publisher request: {error}"),
            )
        })?;
        let response_deadline = Instant::now()
            .checked_add(Duration::from_millis(request.response_timeout_ms()))
            .ok_or_else(|| {
                publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Io,
                    "publisher response deadline overflowed",
                )
            })?;
        let response_frame = receive_response_before(&stream, response_deadline)?;

        Ok(TransportReply {
            peer_uid,
            response_frame,
        })
    }
}

fn validate_listener_contract(
    request: &EncodedRequestFrame,
    listener: Option<BorrowedFd<'_>>,
) -> Result<(), TransportFailure> {
    let valid = matches!(
        (request.descriptor(), listener),
        (DescriptorPrelude::None, None) | (DescriptorPrelude::Listener, Some(_))
    );
    if valid {
        Ok(())
    } else {
        Err(publisher_failure(
            DeliveryCertainty::Fatal,
            BackendErrorKind::Protocol,
            "publisher listener descriptor does not match the encoded request contract",
        ))
    }
}

fn connect_publisher_before(
    publisher_socket: &AbsolutePath,
    deadline: Instant,
) -> Result<UnixStream, BackendError> {
    let descriptor = create_nonblocking_unix_socket_before(deadline).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Io,
            format!("cannot create locald publisher socket: {error}"),
        )
    })?;
    let address = UnixAddr::new(publisher_socket.as_path()).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Protocol,
            format!("invalid locald publisher socket path: {error}"),
        )
    })?;
    match connect(descriptor.as_raw_fd(), &address) {
        Ok(()) | Err(Errno::EISCONN) => {}
        Err(Errno::EINPROGRESS | Errno::EALREADY | Errno::EAGAIN) => {
            wait_for_publisher(descriptor.as_fd(), PollFlags::POLLOUT, deadline)?;
            let socket_error = getsockopt(&descriptor, SocketError).map_err(|error| {
                BackendError::new(
                    BackendErrorKind::Unreachable,
                    format!("cannot inspect locald publisher connection: {error}"),
                )
            })?;
            if socket_error != 0 {
                return Err(BackendError::new(
                    BackendErrorKind::Unreachable,
                    format!(
                        "cannot connect to locald publisher socket: {}",
                        std::io::Error::from_raw_os_error(socket_error)
                    ),
                ));
            }
        }
        Err(error) => {
            return Err(BackendError::new(
                BackendErrorKind::Unreachable,
                format!("cannot connect to locald publisher socket: {error}"),
            ));
        }
    }
    Ok(UnixStream::from(descriptor))
}

#[cfg(target_os = "linux")]
fn create_nonblocking_unix_socket_before(
    _deadline: Instant,
) -> Result<std::os::fd::OwnedFd, Errno> {
    socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::SOCK_CLOEXEC | SockFlag::SOCK_NONBLOCK,
        None,
    )
}

#[cfg(target_os = "macos")]
fn create_nonblocking_unix_socket_before(deadline: Instant) -> Result<std::os::fd::OwnedFd, Errno> {
    create_nonblocking_unix_socket_before_with_barrier(
        crate::ProcessSpawnBarrier::global(),
        deadline,
        || {},
        || {},
    )
}

#[cfg(target_os = "macos")]
fn create_nonblocking_unix_socket_before_with_barrier(
    barrier: &crate::ProcessSpawnBarrier,
    deadline: Instant,
    after_socket: impl FnOnce(),
    after_flags: impl FnOnce(),
) -> Result<std::os::fd::OwnedFd, Errno> {
    // Waiting here is safe because no descriptor exists yet. Reuse the
    // transport's absolute request deadline so contention is bounded without
    // burning an exact NotSent replay in a busy loop.
    let _acquisition = barrier
        .enter_descriptor_acquisition_before(deadline)
        .map_err(|_| Errno::EBUSY)?;
    if Instant::now() >= deadline {
        return Err(Errno::ETIMEDOUT);
    }
    create_nonblocking_unix_socket_after_acquisition(after_socket, after_flags)
}

#[cfg(all(target_os = "macos", test))]
fn create_nonblocking_unix_socket_with_barrier(
    barrier: &crate::ProcessSpawnBarrier,
    after_socket: impl FnOnce(),
    after_flags: impl FnOnce(),
) -> Result<std::os::fd::OwnedFd, Errno> {
    // Darwin has no SOCK_CLOEXEC/SOCK_NONBLOCK creation flags. Exclude every
    // cooperating spawn or exec from socket() through both flag updates, so
    // the descriptor cannot cross an exec boundary in the transient window.
    let _acquisition = barrier
        .try_enter_descriptor_acquisition()
        .map_err(|_| Errno::EBUSY)?;
    create_nonblocking_unix_socket_after_acquisition(after_socket, after_flags)
}

#[cfg(target_os = "macos")]
fn create_nonblocking_unix_socket_after_acquisition(
    after_socket: impl FnOnce(),
    after_flags: impl FnOnce(),
) -> Result<std::os::fd::OwnedFd, Errno> {
    let descriptor = socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::empty(),
        None,
    )?;
    after_socket();
    fcntl(&descriptor, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
    fcntl(&descriptor, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
    after_flags();
    Ok(descriptor)
}

fn authenticate_publisher_peer(stream: &UnixStream) -> Result<u32, TransportFailure> {
    let peer_uid = authenticate_peer_uid(stream).map_err(|error| {
        TransportFailure::new(
            DeliveryCertainty::Fatal,
            BackendError {
                message: error.message.replace("command peer", "publisher peer"),
                ..error
            },
        )
    })?;
    let expected_uid = Uid::effective().as_raw();
    if peer_uid == expected_uid {
        Ok(peer_uid)
    } else {
        Err(publisher_failure(
            DeliveryCertainty::Fatal,
            BackendErrorKind::Authentication,
            format!("locald publisher peer UID {peer_uid} differs from client UID {expected_uid}"),
        ))
    }
}

fn send_request_before(
    stream: &mut UnixStream,
    request: &EncodedRequestFrame,
    listener: Option<BorrowedFd<'_>>,
    deadline: Instant,
) -> Result<(), TransportFailure> {
    let bytes = request.as_bytes();
    let Some(first_byte) = bytes.first() else {
        return Err(publisher_failure(
            DeliveryCertainty::Fatal,
            BackendErrorKind::Protocol,
            "publisher request frame is empty",
        ));
    };
    let mut request_start = Vec::with_capacity(1 + MACOS_REQUEST_PROOF_BYTES);
    request_start.push(*first_byte);
    #[cfg(target_os = "macos")]
    request_start.extend_from_slice(
        &current_macos_audit_token()
            .map_err(|error| TransportFailure::new(DeliveryCertainty::NotSent, error))?,
    );

    let start_written = loop {
        let iov = [IoSlice::new(&request_start)];
        let sent = listener.map_or_else(
            || sendmsg::<UnixAddr>(stream.as_raw_fd(), &iov, &[], MsgFlags::MSG_DONTWAIT, None),
            |listener| {
                let descriptors = [listener.as_raw_fd()];
                let control = [ControlMessage::ScmRights(&descriptors)];
                sendmsg::<UnixAddr>(
                    stream.as_raw_fd(),
                    &iov,
                    &control,
                    MsgFlags::MSG_DONTWAIT,
                    None,
                )
            },
        );
        match sent {
            Ok(sent @ 1..) if sent <= request_start.len() => break sent,
            Ok(0) => {
                return Err(publisher_failure(
                    DeliveryCertainty::NotSent,
                    BackendErrorKind::Unreachable,
                    "locald publisher closed before receiving the first request byte",
                ));
            }
            Ok(_) => {
                return Err(publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Protocol,
                    "locald publisher reported an invalid initial request byte count",
                ));
            }
            Err(Errno::EINTR) => {}
            Err(Errno::EAGAIN) => {
                wait_for_publisher(stream.as_fd(), PollFlags::POLLOUT, deadline)
                    .map_err(|error| TransportFailure::new(DeliveryCertainty::NotSent, error))?;
            }
            Err(error) => {
                return Err(publisher_failure(
                    DeliveryCertainty::NotSent,
                    BackendErrorKind::Unreachable,
                    format!("cannot send first locald publisher request byte: {error}"),
                ));
            }
        }
    };

    let mut proof_written = start_written;
    while proof_written < request_start.len() {
        match stream.write(&request_start[proof_written..]) {
            Ok(0) => {
                return Err(publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Unreachable,
                    "locald publisher closed while receiving the process-identity proof",
                ));
            }
            Ok(count) => proof_written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for_publisher(stream.as_fd(), PollFlags::POLLOUT, deadline).map_err(
                    |error| TransportFailure::new(DeliveryCertainty::OutcomeUnknown, error),
                )?;
            }
            Err(error) => {
                return Err(publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Unreachable,
                    format!("cannot send locald publisher process-identity proof: {error}"),
                ));
            }
        }
    }

    let mut written = 1;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Unreachable,
                    "locald publisher closed while receiving the request",
                ));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for_publisher(stream.as_fd(), PollFlags::POLLOUT, deadline).map_err(
                    |error| TransportFailure::new(DeliveryCertainty::OutcomeUnknown, error),
                )?;
            }
            Err(error) => {
                return Err(publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Unreachable,
                    format!("cannot send locald publisher request: {error}"),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
const MACOS_REQUEST_PROOF_BYTES: usize = MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES;
#[cfg(not(target_os = "macos"))]
const MACOS_REQUEST_PROOF_BYTES: usize = 0;

#[cfg(target_os = "macos")]
fn current_macos_audit_token() -> Result<[u8; MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES], BackendError>
{
    const TASK_AUDIT_TOKEN: libc::task_flavor_t = 15;
    const TASK_AUDIT_TOKEN_WORDS: usize =
        MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES / size_of::<u32>();

    let mut words = [0_u32; TASK_AUDIT_TOKEN_WORDS];
    let mut word_count = libc::mach_msg_type_number_t::try_from(words.len()).map_err(|error| {
        BackendError::new(
            BackendErrorKind::Authentication,
            format!("publisher audit-token length is invalid: {error}"),
        )
    })?;
    // SAFETY: `words` is writable storage for exactly `word_count` Mach
    // natural words, and the current task port remains valid for this call.
    #[allow(unsafe_code, deprecated)]
    let result = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            TASK_AUDIT_TOKEN,
            words.as_mut_ptr().cast(),
            &raw mut word_count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return Err(BackendError::new(
            BackendErrorKind::Authentication,
            format!("cannot obtain publisher audit token: Mach error {result}"),
        ));
    }
    if usize::try_from(word_count).ok() != Some(words.len()) {
        return Err(BackendError::new(
            BackendErrorKind::Authentication,
            "publisher audit token had an invalid length",
        ));
    }

    let mut token = [0_u8; MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES];
    for (bytes, word) in token.chunks_exact_mut(size_of::<u32>()).zip(words) {
        bytes.copy_from_slice(&word.to_ne_bytes());
    }
    Ok(token)
}

fn receive_response_before(
    stream: &UnixStream,
    deadline: Instant,
) -> Result<Vec<u8>, TransportFailure> {
    let mut response = Vec::new();
    let mut expected = None;
    loop {
        let mut chunk = [0_u8; 4096];
        let received = receive_publisher_chunk(stream.as_raw_fd(), &mut chunk);
        let (count, flags, had_control) = match received {
            Ok(message) => message,
            Err(Errno::EINTR) => continue,
            Err(Errno::EAGAIN) => {
                wait_for_publisher(stream.as_fd(), PollFlags::POLLIN, deadline).map_err(
                    |error| TransportFailure::new(DeliveryCertainty::OutcomeUnknown, error),
                )?;
                continue;
            }
            Err(error) => {
                return Err(publisher_receive_failure(error));
            }
        };
        if had_control || flags.intersects(MsgFlags::MSG_TRUNC | MsgFlags::MSG_CTRUNC) {
            return Err(publisher_failure(
                DeliveryCertainty::OutcomeUnknown,
                BackendErrorKind::Protocol,
                "locald publisher response carried ancillary data or was truncated",
            ));
        }
        if count == 0 {
            return finish_publisher_response(response, expected);
        }
        response.extend_from_slice(&chunk[..count]);

        if response.len() > MAX_PUBLISHER_RESPONSE_BYTES {
            return Err(publisher_failure(
                DeliveryCertainty::OutcomeUnknown,
                BackendErrorKind::Protocol,
                "locald publisher response exceeds the protocol frame bound",
            ));
        }
        if expected.is_none() && response.len() >= 4 {
            let body_length = u32::from_be_bytes(response[..4].try_into().map_err(|_| {
                publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Protocol,
                    "locald publisher response has an invalid length prefix",
                )
            })?) as usize;
            if body_length > MAX_FRAME_JSON_BYTES {
                return Err(publisher_failure(
                    DeliveryCertainty::OutcomeUnknown,
                    BackendErrorKind::Protocol,
                    "locald publisher response exceeds the protocol JSON bound",
                ));
            }
            expected = Some(4 + body_length);
        }
        if expected.is_some_and(|expected| response.len() > expected) {
            return Err(publisher_failure(
                DeliveryCertainty::OutcomeUnknown,
                BackendErrorKind::Protocol,
                "locald publisher response contains trailing bytes",
            ));
        }
    }
}

fn publisher_receive_failure(error: Errno) -> TransportFailure {
    publisher_failure(
        DeliveryCertainty::OutcomeUnknown,
        BackendErrorKind::Unreachable,
        format!("cannot receive locald publisher response: {error}"),
    )
}

#[cfg(not(target_os = "macos"))]
fn receive_publisher_chunk(
    descriptor: std::os::fd::RawFd,
    chunk: &mut [u8],
) -> Result<(usize, MsgFlags, bool), Errno> {
    receive_publisher_chunk_unlocked(descriptor, chunk, || {}, |_| {}, || {})
}

#[cfg(target_os = "macos")]
fn receive_publisher_chunk(
    descriptor: std::os::fd::RawFd,
    chunk: &mut [u8],
) -> Result<(usize, MsgFlags, bool), Errno> {
    receive_publisher_chunk_with_barrier(
        crate::ProcessSpawnBarrier::global(),
        descriptor,
        chunk,
        || {},
        |_| {},
        || {},
    )
}

#[cfg(target_os = "macos")]
fn receive_publisher_chunk_with_barrier(
    barrier: &crate::ProcessSpawnBarrier,
    descriptor: std::os::fd::RawFd,
    chunk: &mut [u8],
    after_receive: impl FnOnce(),
    after_descriptor_closed: impl FnMut(std::os::fd::RawFd),
    after_control_cleanup: impl FnOnce(),
) -> Result<(usize, MsgFlags, bool), Errno> {
    // Darwin has no MSG_CMSG_CLOEXEC. Exclude every cooperating spawn or exec
    // before recvmsg can install an unexpected descriptor, and retain the
    // exclusion until every installed descriptor has been secured and closed.
    let _acquisition = barrier
        .try_enter_descriptor_acquisition()
        .map_err(|_| Errno::EBUSY)?;
    receive_publisher_chunk_unlocked(
        descriptor,
        chunk,
        after_receive,
        after_descriptor_closed,
        after_control_cleanup,
    )
}

#[allow(
    unsafe_code,
    reason = "recvmsg response control data must be inspected so every unexpected SCM_RIGHTS descriptor is closed even when ancillary data is truncated"
)]
fn receive_publisher_chunk_unlocked(
    descriptor: std::os::fd::RawFd,
    chunk: &mut [u8],
    after_receive: impl FnOnce(),
    after_descriptor_closed: impl FnMut(std::os::fd::RawFd),
    after_control_cleanup: impl FnOnce(),
) -> Result<(usize, MsgFlags, bool), Errno> {
    let mut control = [0_usize; RESPONSE_CONTROL_WORDS];
    let mut iov = libc::iovec {
        iov_base: chunk.as_mut_ptr().cast(),
        iov_len: chunk.len(),
    };
    // SAFETY: Every pointer in the message references a live, correctly
    // aligned stack allocation for the duration of recvmsg.
    let mut message = unsafe { zeroed::<libc::msghdr>() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = size_of::<[usize; RESPONSE_CONTROL_WORDS]>() as _;

    // SAFETY: `message` names initialized writable buffers and the borrowed
    // socket remains live for this call.
    let received =
        unsafe { libc::recvmsg(descriptor, &raw mut message, publisher_recv_flags().bits()) };
    if received < 0 {
        return Err(Errno::last());
    }
    after_receive();
    let had_control = close_response_control_messages(&message, after_descriptor_closed);
    after_control_cleanup();
    Ok((
        received as usize,
        MsgFlags::from_bits_truncate(message.msg_flags),
        had_control,
    ))
}

#[allow(
    unsafe_code,
    reason = "kernel-produced cmsghdr records must be walked to close all unexpected response descriptors before rejecting the response"
)]
fn close_response_control_messages(
    message: &libc::msghdr,
    mut after_descriptor_closed: impl FnMut(std::os::fd::RawFd),
) -> bool {
    let control_start = message.msg_control as usize;
    #[allow(
        clippy::unnecessary_cast,
        reason = "libc::msghdr::msg_controllen uses target-specific integer types"
    )]
    let control_end = control_start.saturating_add(message.msg_controllen as usize);
    let mut had_control = false;
    // SAFETY: recvmsg initialized the control buffer described by `message`.
    let mut header = unsafe { libc::CMSG_FIRSTHDR(message) };
    while !header.is_null() {
        had_control = true;
        let header_address = header as usize;
        if header_address < control_start
            || header_address.saturating_add(size_of::<libc::cmsghdr>()) > control_end
        {
            break;
        }
        // SAFETY: The complete header was bounds-checked against the control
        // buffer that recvmsg initialized.
        let header_value = unsafe { &*header };
        let header_length = header_value.cmsg_len as usize;
        // SAFETY: CMSG_LEN only computes the platform-aligned fixed header
        // size for an empty payload.
        let data_offset = unsafe { libc::CMSG_LEN(0) as usize };
        if header_length < data_offset || header_address.saturating_add(header_length) > control_end
        {
            break;
        }
        if header_value.cmsg_level == libc::SOL_SOCKET && header_value.cmsg_type == libc::SCM_RIGHTS
        {
            let descriptor_bytes = header_length - data_offset;
            let descriptor_count = descriptor_bytes / size_of::<std::os::fd::RawFd>();
            // SAFETY: CMSG_DATA points into the checked header payload. The
            // kernel encodes SCM_RIGHTS as an array of RawFd values.
            let descriptors = unsafe {
                std::slice::from_raw_parts(
                    libc::CMSG_DATA(header).cast::<std::os::fd::RawFd>(),
                    descriptor_count,
                )
            };
            for &descriptor in descriptors {
                // Mark the descriptor close-on-exec before any further
                // interpretation, then close it unconditionally. Failures do
                // not change the protocol rejection disposition.
                // SAFETY: Each value is an owned descriptor installed by
                // recvmsg for this process.
                unsafe {
                    let flags = libc::fcntl(descriptor, libc::F_GETFD);
                    if flags >= 0 {
                        libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                    }
                    libc::close(descriptor);
                }
                after_descriptor_closed(descriptor);
            }
        }
        // SAFETY: The current header is complete and lies inside the live
        // control buffer, as established above.
        header = unsafe { libc::CMSG_NXTHDR(message, header) };
    }
    had_control
}

fn finish_publisher_response(
    response: Vec<u8>,
    expected: Option<usize>,
) -> Result<Vec<u8>, TransportFailure> {
    let Some(expected) = expected else {
        return Err(publisher_failure(
            DeliveryCertainty::OutcomeUnknown,
            BackendErrorKind::Protocol,
            "locald publisher closed with a truncated response header",
        ));
    };
    if response.len() == expected {
        Ok(response)
    } else {
        Err(publisher_failure(
            DeliveryCertainty::OutcomeUnknown,
            BackendErrorKind::Protocol,
            "locald publisher closed with a truncated response body",
        ))
    }
}

#[cfg(target_os = "linux")]
const fn publisher_recv_flags() -> MsgFlags {
    MsgFlags::MSG_DONTWAIT.union(MsgFlags::MSG_CMSG_CLOEXEC)
}

#[cfg(not(target_os = "linux"))]
const fn publisher_recv_flags() -> MsgFlags {
    MsgFlags::MSG_DONTWAIT
}

fn wait_for_publisher(
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
                    "locald publisher I/O exceeded its operation deadline",
                )
            })?;
        let timeout = PollTimeout::try_from(remaining).unwrap_or(PollTimeout::MAX);
        let mut descriptors = [PollFd::new(descriptor, events)];
        match poll(&mut descriptors, timeout) {
            Ok(0) => {
                return Err(BackendError::new(
                    BackendErrorKind::Unreachable,
                    "locald publisher I/O exceeded its operation deadline",
                ));
            }
            Ok(_) => {
                let revents = descriptors[0].revents().unwrap_or(PollFlags::POLLNVAL);
                if revents.contains(PollFlags::POLLNVAL) {
                    return Err(BackendError::new(
                        BackendErrorKind::Io,
                        "locald publisher descriptor became invalid",
                    ));
                }
                return Ok(());
            }
            Err(Errno::EINTR) => {}
            Err(error) => {
                return Err(BackendError::new(
                    BackendErrorKind::Io,
                    format!("cannot wait for locald publisher I/O: {error}"),
                ));
            }
        }
    }
}

fn publisher_failure(
    certainty: DeliveryCertainty,
    kind: BackendErrorKind,
    message: impl Into<String>,
) -> TransportFailure {
    TransportFailure::new(certainty, BackendError::new(kind, message))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "socket fixtures fail immediately when their deterministic transport script is invalid"
)]
mod tests {
    use std::fs::File;
    use std::io::{IoSliceMut, Read as _, Write as _};
    use std::net::TcpListener;
    use std::os::fd::{AsFd as _, AsRawFd as _, RawFd};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::thread;

    use locald_publisher_protocol::{
        AcquireArguments, AcquisitionAttemptHandle, DaemonEpoch, LeaseHandle, PublisherRequest,
        ReleaseArguments, ReleaseResult, RequestEnvelope, ResponseEnvelope, SemanticOrigin,
        encode_request_frame, encode_response_frame,
    };
    use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
    use nix::sys::socket::{ControlMessageOwned, MsgFlags, UnixAddr, recvmsg, sendmsg};
    use tempfile::TempDir;

    use super::*;

    fn epoch() -> DaemonEpoch {
        DaemonEpoch::from_bytes([1; 16])
    }

    fn release_frame() -> EncodedRequestFrame {
        encode_request_frame(&RequestEnvelope::v1(
            epoch(),
            PublisherRequest::Release(ReleaseArguments {
                lease_handle: LeaseHandle::from_bytes([2; 32]),
            }),
        ))
        .expect("encode release frame")
    }

    fn acquire_frame() -> EncodedRequestFrame {
        encode_request_frame(&RequestEnvelope::v1(
            epoch(),
            PublisherRequest::Acquire(AcquireArguments {
                acquisition_attempt_handle: AcquisitionAttemptHandle::from_bytes([3; 32]),
                acknowledged_origin: SemanticOrigin::parse("https://workbench.example.localhost")
                    .expect("semantic origin"),
            }),
        ))
        .expect("encode acquire frame")
    }

    fn release_response() -> Vec<u8> {
        encode_response_frame(&ResponseEnvelope::success(
            epoch(),
            ReleaseResult::released(),
        ))
        .expect("encode release response")
    }

    fn publisher_listener() -> (TempDir, AbsolutePath, UnixListener) {
        let directory = tempfile::tempdir().expect("publisher tempdir");
        let path = directory.path().join("publisher-v1.sock");
        let listener = UnixListener::bind(&path).expect("bind publisher socket");
        let path = AbsolutePath::parse(path.to_str().expect("UTF-8 publisher path"))
            .expect("absolute publisher path");
        (directory, path, listener)
    }

    #[test]
    fn created_unix_transport_socket_is_nonblocking_and_close_on_exec() {
        let descriptor =
            create_nonblocking_unix_socket_before(Instant::now() + PUBLISHER_REQUEST_TIMEOUT)
                .expect("create Unix transport socket");
        let descriptor_flags = FdFlag::from_bits_truncate(
            fcntl(&descriptor, FcntlArg::F_GETFD).expect("read descriptor flags"),
        );
        let status_flags = OFlag::from_bits_truncate(
            fcntl(&descriptor, FcntlArg::F_GETFL).expect("read socket status flags"),
        );

        assert!(descriptor_flags.contains(FdFlag::FD_CLOEXEC));
        assert!(status_flags.contains(OFlag::O_NONBLOCK));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_socket_creation_fails_before_acquisition_during_spawn() {
        let barrier = crate::ProcessSpawnBarrier::isolated_for_test();
        let _spawn = barrier.enter_spawn();
        let mut socket_created = false;

        let error = create_nonblocking_unix_socket_with_barrier(
            &barrier,
            || {
                socket_created = true;
            },
            || {},
        )
        .expect_err("active spawn must close the descriptor-acquisition gate");

        assert_eq!(error, Errno::EBUSY);
        assert!(!socket_created);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_socket_creation_waits_for_spawn_before_acquiring_a_descriptor() {
        let barrier = crate::ProcessSpawnBarrier::isolated_for_test();
        let spawn = barrier.enter_spawn();
        let worker_barrier = barrier.clone();
        let (socket_created_tx, socket_created_rx) = std::sync::mpsc::channel();

        let worker = thread::spawn(move || {
            create_nonblocking_unix_socket_before_with_barrier(
                &worker_barrier,
                Instant::now() + Duration::from_secs(1),
                || {
                    socket_created_tx
                        .send(())
                        .expect("announce socket creation")
                },
                || {},
            )
            .expect("socket creation waits for active spawn")
        });

        assert!(
            socket_created_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "contention wait must not acquire a descriptor during the spawn"
        );
        drop(spawn);
        socket_created_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("socket is created after spawn finishes");
        let descriptor = worker.join().expect("socket worker exits");
        let descriptor_flags = FdFlag::from_bits_truncate(
            fcntl(&descriptor, FcntlArg::F_GETFD).expect("read descriptor flags"),
        );
        let status_flags = OFlag::from_bits_truncate(
            fcntl(&descriptor, FcntlArg::F_GETFL).expect("read socket status flags"),
        );
        assert!(descriptor_flags.contains(FdFlag::FD_CLOEXEC));
        assert!(status_flags.contains(OFlag::O_NONBLOCK));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_socket_contention_deadline_expires_before_descriptor_creation() {
        let barrier = crate::ProcessSpawnBarrier::isolated_for_test();
        let _spawn = barrier.enter_spawn();
        let mut socket_created = false;

        let error = create_nonblocking_unix_socket_before_with_barrier(
            &barrier,
            Instant::now(),
            || {
                socket_created = true;
            },
            || {},
        )
        .expect_err("expired contention wait must fail before socket creation");

        assert_eq!(error, Errno::EBUSY);
        assert!(!socket_created);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_socket_creation_excludes_spawn_until_cloexec_is_installed() {
        let barrier = crate::ProcessSpawnBarrier::isolated_for_test();
        let worker_barrier = barrier.clone();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let mut worker = None;

        let descriptor = create_nonblocking_unix_socket_with_barrier(
            &barrier,
            || {
                worker = Some(thread::spawn(move || {
                    let _spawn = worker_barrier.enter_spawn();
                    acquired_tx.send(()).expect("announce spawn permit");
                }));

                let deadline = Instant::now() + Duration::from_secs(1);
                while barrier.announced_spawns_for_test() == 0 {
                    assert!(
                        Instant::now() < deadline,
                        "spawn intent must become observable"
                    );
                    thread::yield_now();
                }
                assert_eq!(
                    acquired_rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty),
                    "spawn must remain excluded before close-on-exec setup",
                );
            },
            || {
                assert_eq!(
                    acquired_rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty),
                    "spawn must remain excluded after both descriptor flags are installed",
                );
            },
        )
        .expect("create guarded Unix transport socket");

        let descriptor_flags = FdFlag::from_bits_truncate(
            fcntl(&descriptor, FcntlArg::F_GETFD).expect("read descriptor flags"),
        );
        let status_flags = OFlag::from_bits_truncate(
            fcntl(&descriptor, FcntlArg::F_GETFL).expect("read socket status flags"),
        );
        assert!(descriptor_flags.contains(FdFlag::FD_CLOEXEC));
        assert!(status_flags.contains(OFlag::O_NONBLOCK));
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("spawn proceeds after guarded socket setup");
        worker.expect("spawn worker").join().expect("worker exits");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_response_receive_fails_before_recv_during_spawn() {
        let (sender, receiver) = UnixStream::pair().expect("create response socket pair");
        let file = File::open("/dev/null").expect("open descriptor fixture");
        let descriptors = [file.as_raw_fd()];
        let control = [ControlMessage::ScmRights(&descriptors)];
        let response = [IoSlice::new(b"x")];
        assert_eq!(
            sendmsg::<UnixAddr>(
                sender.as_raw_fd(),
                &response,
                &control,
                MsgFlags::empty(),
                None,
            )
            .expect("send response descriptor"),
            1
        );

        let barrier = crate::ProcessSpawnBarrier::isolated_for_test();
        let spawn = barrier.enter_spawn();
        let mut chunk = [0_u8; 1];
        let mut received = false;
        let error = receive_publisher_chunk_with_barrier(
            &barrier,
            receiver.as_raw_fd(),
            &mut chunk,
            || {
                received = true;
            },
            |_| {},
            || {},
        )
        .expect_err("active spawn must close the response descriptor-acquisition gate");

        assert_eq!(error, Errno::EBUSY);
        assert!(!received, "barrier contention must fail before recvmsg");
        assert_eq!(
            publisher_receive_failure(error).certainty,
            DeliveryCertainty::OutcomeUnknown,
            "post-send response-barrier contention has uncertain outcome",
        );
        drop(spawn);

        let closed_descriptors = std::cell::Cell::new(0);
        let (count, _, had_control) = receive_publisher_chunk_with_barrier(
            &barrier,
            receiver.as_raw_fd(),
            &mut chunk,
            || {},
            |_| closed_descriptors.set(closed_descriptors.get() + 1),
            || {},
        )
        .expect("receive response after spawn finishes");
        assert_eq!(count, 1);
        assert_eq!(chunk, *b"x");
        assert!(had_control);
        assert_eq!(closed_descriptors.get(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[allow(
        unsafe_code,
        reason = "the fixture verifies that each raw descriptor installed by recvmsg is closed before the acquisition guard is released"
    )]
    fn macos_response_receive_excludes_spawn_until_descriptors_are_closed() {
        let (sender, receiver) = UnixStream::pair().expect("create response socket pair");
        let first_file = File::open("/dev/null").expect("open first descriptor fixture");
        let second_file = File::open("/dev/null").expect("open second descriptor fixture");
        let descriptors = [first_file.as_raw_fd(), second_file.as_raw_fd()];
        let control = [ControlMessage::ScmRights(&descriptors)];
        let response = [IoSlice::new(b"x")];
        assert_eq!(
            sendmsg::<UnixAddr>(
                sender.as_raw_fd(),
                &response,
                &control,
                MsgFlags::empty(),
                None,
            )
            .expect("send response descriptors"),
            1
        );

        let barrier = crate::ProcessSpawnBarrier::isolated_for_test();
        let worker_barrier = barrier.clone();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let closed_descriptors = std::cell::Cell::new(0);
        let mut worker = None;
        let mut chunk = [0_u8; 1];

        let (count, _, had_control) = receive_publisher_chunk_with_barrier(
            &barrier,
            receiver.as_raw_fd(),
            &mut chunk,
            || {
                worker = Some(thread::spawn(move || {
                    let _spawn = worker_barrier.enter_spawn();
                    acquired_tx.send(()).expect("announce spawn permit");
                }));

                let deadline = Instant::now() + Duration::from_secs(1);
                while barrier.announced_spawns_for_test() == 0 {
                    assert!(
                        Instant::now() < deadline,
                        "spawn intent must become observable"
                    );
                    thread::yield_now();
                }
                assert_eq!(
                    acquired_rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty),
                    "spawn must remain excluded after recvmsg installs descriptors",
                );
            },
            |descriptor| {
                // SAFETY: The cleanup callback runs immediately after the
                // transport closes this kernel-installed raw descriptor.
                assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
                assert_eq!(Errno::last(), Errno::EBADF);
                closed_descriptors.set(closed_descriptors.get() + 1);
                assert_eq!(
                    acquired_rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty),
                    "spawn must remain excluded while response descriptors are closed",
                );
            },
            || {
                assert_eq!(closed_descriptors.get(), 2);
                assert_eq!(
                    acquired_rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty),
                    "spawn must remain excluded after cleanup and before guard release",
                );
            },
        )
        .expect("receive guarded response descriptors");

        assert_eq!(count, 1);
        assert_eq!(chunk, *b"x");
        assert!(had_control);
        acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("spawn proceeds after response descriptor cleanup");
        worker.expect("spawn worker").join().expect("worker exits");
    }

    fn receive_request(mut stream: UnixStream) -> (Vec<u8>, Vec<RawFd>) {
        let mut first = [0_u8; 1];
        let descriptors = {
            let mut iov = [IoSliceMut::new(&mut first)];
            let mut control = nix::cmsg_space!([RawFd; 2]);
            let message = recvmsg::<UnixAddr>(
                stream.as_raw_fd(),
                &mut iov,
                Some(&mut control),
                MsgFlags::empty(),
            )
            .expect("receive first request byte");
            assert_eq!(message.bytes, 1);
            assert!(!message.flags.contains(MsgFlags::MSG_CTRUNC));
            let mut descriptors = Vec::new();
            for message in message.cmsgs().expect("decode request control messages") {
                match message {
                    ControlMessageOwned::ScmRights(received) => descriptors.extend(received),
                    other => panic!("unexpected request control message: {other:?}"),
                }
            }
            descriptors
        };
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).expect("read request frame");
        #[cfg(target_os = "macos")]
        {
            assert!(
                rest.len() >= MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES,
                "macOS request carries a complete audit proof"
            );
            let received_proof = rest
                .drain(..MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES)
                .collect::<Vec<_>>();
            let peer_token = getsockopt(&stream, nix::sys::socket::sockopt::LocalPeerToken)
                .expect("read kernel publisher peer audit token");
            let mut expected_proof = [0_u8; MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES];
            for (bytes, word) in expected_proof
                .chunks_exact_mut(size_of::<u32>())
                .zip(peer_token.val)
            {
                bytes.copy_from_slice(&word.to_ne_bytes());
            }
            assert_eq!(
                received_proof, expected_proof,
                "request proof matches the kernel-observed client"
            );
        }
        let mut frame = first.to_vec();
        frame.extend(rest);
        (frame, descriptors)
    }

    fn send_response(mut stream: UnixStream, response: &[u8]) {
        stream
            .write_all(response)
            .expect("write publisher response");
        stream
            .shutdown(Shutdown::Write)
            .expect("finish publisher response");
    }

    #[test]
    fn unix_transport_preserves_exact_request_bytes_without_a_descriptor() {
        let (_directory, socket, listener) = publisher_listener();
        let request = release_frame();
        let expected_request = request.as_bytes().to_vec();
        let response = release_response();
        let expected_response = response.clone();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept publisher connection");
            let (actual_request, descriptors) = receive_request(stream.try_clone().expect("clone"));
            assert_eq!(actual_request, expected_request);
            assert!(descriptors.is_empty());
            send_response(stream, &response);
        });

        let reply = UnixPublisherTransport
            .exchange(&socket, &request, None)
            .expect("publisher exchange");
        assert_eq!(reply.peer_uid, Uid::effective().as_raw());
        assert_eq!(reply.response_frame, expected_response);
        server.join().expect("publisher server");
    }

    #[test]
    fn unix_transport_transfers_exactly_one_listener_with_the_first_byte() {
        let (_directory, socket, listener) = publisher_listener();
        let request = acquire_frame();
        let expected_request = request.as_bytes().to_vec();
        let response = release_response();
        let published_listener = TcpListener::bind("127.0.0.1:0").expect("published listener");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept publisher connection");
            let (actual_request, descriptors) = receive_request(stream.try_clone().expect("clone"));
            assert_eq!(actual_request, expected_request);
            assert_eq!(descriptors.len(), 1);
            nix::unistd::close(descriptors[0]).expect("close received listener");
            send_response(stream, &response);
        });

        UnixPublisherTransport
            .exchange(&socket, &request, Some(published_listener.as_fd()))
            .expect("publisher exchange");
        server.join().expect("publisher server");
    }

    #[test]
    fn unix_transport_classifies_pre_send_and_post_send_failures_conservatively() {
        let directory = tempfile::tempdir().expect("publisher tempdir");
        let missing = AbsolutePath::parse(
            directory
                .path()
                .join("missing.sock")
                .to_str()
                .expect("UTF-8 publisher path"),
        )
        .expect("absolute publisher path");
        let request = release_frame();
        let failure = UnixPublisherTransport
            .exchange(&missing, &request, None)
            .expect_err("missing publisher socket");
        assert_eq!(failure.certainty, DeliveryCertainty::NotSent);

        let (_directory, socket, listener) = publisher_listener();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept publisher connection");
            let (actual_request, descriptors) = receive_request(stream);
            assert_eq!(actual_request, request.as_bytes());
            assert!(descriptors.is_empty());
        });
        let failure = UnixPublisherTransport
            .exchange(&socket, &release_frame(), None)
            .expect_err("publisher closed without response");
        assert_eq!(failure.certainty, DeliveryCertainty::OutcomeUnknown);
        server.join().expect("publisher server");
    }

    #[test]
    fn unix_transport_rejects_listener_contract_mismatch_before_connecting() {
        let directory = tempfile::tempdir().expect("publisher tempdir");
        let missing = AbsolutePath::parse(
            directory
                .path()
                .join("missing.sock")
                .to_str()
                .expect("UTF-8 publisher path"),
        )
        .expect("absolute publisher path");
        let listener = TcpListener::bind("127.0.0.1:0").expect("published listener");

        for failure in [
            UnixPublisherTransport
                .exchange(&missing, &acquire_frame(), None)
                .expect_err("missing required listener"),
            UnixPublisherTransport
                .exchange(&missing, &release_frame(), Some(listener.as_fd()))
                .expect_err("surplus listener"),
        ] {
            assert_eq!(failure.certainty, DeliveryCertainty::Fatal);
            assert_eq!(failure.error.kind, BackendErrorKind::Protocol);
        }
    }

    #[test]
    fn unix_transport_rejects_response_descriptors_and_trailing_bytes() {
        let (_directory, socket, listener) = publisher_listener();
        let response = release_response();
        let file = File::open("/dev/null").expect("open descriptor fixture");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept publisher connection");
            let (request, descriptors) = receive_request(stream.try_clone().expect("clone"));
            assert_eq!(request, release_frame().as_bytes());
            assert!(descriptors.is_empty());

            let descriptors = [file.as_raw_fd()];
            let control = [ControlMessage::ScmRights(&descriptors)];
            let first = [IoSlice::new(&response[..1])];
            assert_eq!(
                sendmsg::<UnixAddr>(
                    stream.as_raw_fd(),
                    &first,
                    &control,
                    MsgFlags::empty(),
                    None,
                )
                .expect("send unexpected response descriptor"),
                1
            );
            let mut stream = stream;
            if let Err(error) = stream.write_all(&response[1..]) {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe,
                    "only the client's expected early rejection may stop the fixture write"
                );
            }
        });
        let failure = UnixPublisherTransport
            .exchange(&socket, &release_frame(), None)
            .expect_err("unexpected response descriptor");
        assert_eq!(failure.certainty, DeliveryCertainty::OutcomeUnknown);
        assert_eq!(failure.error.kind, BackendErrorKind::Protocol);
        server.join().expect("publisher server");

        let (_directory, socket, listener) = publisher_listener();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept publisher connection");
            let (_request, descriptors) = receive_request(stream.try_clone().expect("clone"));
            assert!(descriptors.is_empty());
            let mut response = release_response();
            response.push(0);
            send_response(stream, &response);
        });
        let failure = UnixPublisherTransport
            .exchange(&socket, &release_frame(), None)
            .expect_err("trailing response byte");
        assert_eq!(failure.certainty, DeliveryCertainty::OutcomeUnknown);
        assert_eq!(failure.error.kind, BackendErrorKind::Protocol);
        server.join().expect("publisher server");
    }
}
