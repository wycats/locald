//! Authenticated Unix transport for externally published listener capabilities.
//!
//! The publisher protocol deliberately uses a dedicated framed socket. This
//! module owns that socket, authenticates every connection from kernel state,
//! receives listener descriptors without leaking them across process spawn,
//! and hands an already-validated capability to the publication dispatcher.

#![allow(clippy::redundant_pub_crate)] // Sibling modules consume the private transport boundary.

use async_trait::async_trait;
use locald_publisher_protocol::{
    DaemonEpoch, DescriptorPrelude, FRAME_TIMEOUT_MS, FrameError, MAX_FRAME_JSON_BYTES,
    ProtocolError, RequestEnvelope, ResponseEnvelope, StableErrorCode, decode_request_frame,
    encode_response_frame,
};
use nix::fcntl::{FcntlArg, FdFlag, fcntl};
#[cfg(not(target_os = "macos"))]
use nix::sys::socket::sockopt::{AcceptConn, ReusePort};
use nix::sys::socket::{MsgFlags, SockType, getsockopt, sockopt::SockType as SocketType};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::mem::{size_of, zeroed};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
#[cfg(target_os = "linux")]
use std::os::fd::AsFd as _;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
#[cfg(target_os = "macos")]
use tokio::io::unix::AsyncFd;
#[cfg(not(target_os = "macos"))]
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};

const REQUEST_FIXED_BYTES: usize = 5;
const RESPONSE_FIXED_BYTES: usize = 4;
const RUN_DIRECTORY_MODE: u32 = 0o700;
const PUBLISHER_SOCKET_MODE: u32 = 0o600;
const PEER_CLOSE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_RECEIVED_DESCRIPTORS: usize = 16;
const RECEIVE_CONTROL_WORDS: usize = (size_of::<libc::cmsghdr>()
    + MAX_RECEIVED_DESCRIPTORS * size_of::<RawFd>())
.div_ceil(size_of::<usize>());

/// A daemon-wide exclusion entered around descriptor receipt on macOS.
///
/// Every process-spawning path in the daemon must take the corresponding spawn
/// side of the same barrier. Linux obtains close-on-exec atomically through
/// `MSG_CMSG_CLOEXEC` and does not enter this barrier.
pub(crate) trait PublisherSpawnBarrier: Send + Sync + fmt::Debug {
    /// Hold process spawning until every descriptor received by one `recvmsg`
    /// call has been made close-on-exec.
    #[cfg_attr(
        not(target_os = "macos"),
        allow(
            dead_code,
            reason = "Linux receives descriptors atomically with MSG_CMSG_CLOEXEC"
        )
    )]
    fn enter_descriptor_receipt(
        &self,
    ) -> Result<Box<dyn PublisherSpawnBarrierGuard + '_>, PublisherSocketError>;
}

/// Type-erased lifetime guard returned by [`PublisherSpawnBarrier`].
#[cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "Linux receives descriptors atomically with MSG_CMSG_CLOEXEC"
    )
)]
pub(crate) trait PublisherSpawnBarrierGuard: Send {}

impl PublisherSpawnBarrierGuard for locald_utils::process_spawn::DescriptorReceiptGuard {}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct GlobalPublisherSpawnBarrier;

#[cfg(target_os = "macos")]
impl PublisherSpawnBarrier for GlobalPublisherSpawnBarrier {
    fn enter_descriptor_receipt(
        &self,
    ) -> Result<Box<dyn PublisherSpawnBarrierGuard + '_>, PublisherSocketError> {
        locald_utils::process_spawn::ProcessSpawnBarrier::global()
            .try_enter_descriptor_receipt()
            .map(|guard| Box::new(guard) as Box<dyn PublisherSpawnBarrierGuard>)
            .map_err(|_| PublisherSocketError::SpawnBarrierBusy)
    }
}

/// Return the process-global descriptor-receipt adapter on platforms that
/// cannot atomically receive descriptors with close-on-exec set.
#[allow(clippy::unnecessary_wraps)] // One cross-platform constructor feeds the shared config.
pub(crate) fn publisher_spawn_barrier() -> Option<Arc<dyn PublisherSpawnBarrier>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(GlobalPublisherSpawnBarrier))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Server configuration that is fixed before the publisher socket is bound.
pub(crate) struct PublisherSocketConfig {
    socket_path: PathBuf,
    expected_uid: u32,
    front_door_ports: BTreeSet<u16>,
    spawn_barrier: Option<Arc<dyn PublisherSpawnBarrier>>,
}

impl PublisherSocketConfig {
    /// Construct a configuration for the daemon's effective UID.
    pub(crate) fn for_current_user(
        socket_path: PathBuf,
        front_door_ports: impl IntoIterator<Item = u16>,
        spawn_barrier: Option<Arc<dyn PublisherSpawnBarrier>>,
    ) -> Self {
        Self {
            socket_path,
            expected_uid: nix::unistd::geteuid().as_raw(),
            front_door_ports: front_door_ports.into_iter().collect(),
            spawn_barrier,
        }
    }

    #[cfg(test)]
    fn for_test(socket_path: PathBuf, front_door_ports: impl IntoIterator<Item = u16>) -> Self {
        Self {
            socket_path,
            expected_uid: nix::unistd::geteuid().as_raw(),
            front_door_ports: front_door_ports.into_iter().collect(),
            spawn_barrier: test_spawn_barrier(),
        }
    }
}

impl fmt::Debug for PublisherSocketConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublisherSocketConfig")
            .field("socket_path", &self.socket_path)
            .field("expected_uid", &self.expected_uid)
            .field("front_door_ports", &self.front_door_ports)
            .field("has_spawn_barrier", &self.spawn_barrier.is_some())
            .finish()
    }
}

/// Kernel-observed process-birth evidence for one publisher connection.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum PublisherProcessBirthEvidence {
    /// Darwin process start time from `proc_bsdinfo`.
    #[cfg(target_os = "macos")]
    MacOs {
        start_seconds: u64,
        start_microseconds: u64,
    },
    /// Linux boot identity plus `/proc/<pid>/stat` start ticks.
    #[cfg(target_os = "linux")]
    Linux { boot_id: Box<str>, start_ticks: u64 },
}

impl fmt::Debug for PublisherProcessBirthEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublisherProcessBirthEvidence(<redacted>)")
    }
}

/// Exact same-user publisher authority observed from the accepted socket.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct PublisherPrincipalEvidence {
    uid: u32,
    pid: u32,
    birth: PublisherProcessBirthEvidence,
}

impl PublisherPrincipalEvidence {
    pub(crate) const fn uid(&self) -> u32 {
        self.uid
    }

    pub(crate) const fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) const fn birth(&self) -> &PublisherProcessBirthEvidence {
        &self.birth
    }
}

impl fmt::Debug for PublisherPrincipalEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublisherPrincipalEvidence(<redacted>)")
    }
}

/// Stable kernel identity of a validated listener capability.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum PublisherListenerIdentity {
    /// Darwin listener identity from `PROC_PIDFDSOCKETINFO`.
    #[cfg(target_os = "macos")]
    MacOsIpv4 {
        address: [u8; 4],
        port: u16,
        pcb_generation: u64,
    },
    /// Linux listener identity and network-namespace proof.
    #[cfg(target_os = "linux")]
    LinuxIpv4 {
        address: [u8; 4],
        port: u16,
        socket_cookie: u64,
        network_namespace_cookie: u64,
    },
}

impl PublisherListenerIdentity {
    #[cfg(test)]
    pub(crate) const fn address(&self) -> [u8; 4] {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsIpv4 { address, .. } => *address,
            #[cfg(target_os = "linux")]
            Self::LinuxIpv4 { address, .. } => *address,
        }
    }

    #[cfg(test)]
    pub(crate) const fn port(&self) -> u16 {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsIpv4 { port, .. } => *port,
            #[cfg(target_os = "linux")]
            Self::LinuxIpv4 { port, .. } => *port,
        }
    }
}

impl fmt::Debug for PublisherListenerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublisherListenerIdentity(<redacted>)")
    }
}

/// An owned, validated listener whose descriptor keeps its binding reserved.
pub(crate) struct ValidatedPublisherListener {
    identity: PublisherListenerIdentity,
    guard: Arc<TcpListener>,
}

impl ValidatedPublisherListener {
    #[cfg(test)]
    const fn identity(&self) -> &PublisherListenerIdentity {
        &self.identity
    }

    /// Consume the capability into its identity and root ownership guard.
    pub(crate) fn into_parts(self) -> (PublisherListenerIdentity, Arc<dyn Send + Sync>) {
        (self.identity, self.guard)
    }
}

impl fmt::Debug for ValidatedPublisherListener {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedPublisherListener(<redacted>)")
    }
}

/// Authenticated authority accompanying one decoded publisher request.
pub(crate) struct PublisherRequestContext {
    principal: PublisherPrincipalEvidence,
    listener: Option<ValidatedPublisherListener>,
    peer_closed: Option<oneshot::Receiver<()>>,
}

impl PublisherRequestContext {
    pub(crate) const fn principal(&self) -> &PublisherPrincipalEvidence {
        &self.principal
    }

    pub(crate) fn take_listener(&mut self) -> Option<ValidatedPublisherListener> {
        self.listener.take()
    }

    /// Wait until the publisher closes the request connection completely.
    ///
    /// A cooperative request write-half shutdown is part of framing and does
    /// not complete this future. Contexts constructed outside the socket
    /// server have no connection lifetime and therefore remain pending.
    pub(crate) async fn wait_for_peer_close(&mut self) {
        match self.peer_closed.as_mut() {
            Some(peer_closed) => {
                let _ = peer_closed.await;
            }
            None => std::future::pending().await,
        }
    }
}

impl fmt::Debug for PublisherRequestContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublisherRequestContext")
            .field("principal", &self.principal)
            .field("has_listener", &self.listener.is_some())
            .field("tracks_peer_close", &self.peer_closed.is_some())
            .finish()
    }
}

/// Narrow dispatch boundary between transport validation and publication state.
#[async_trait]
pub(crate) trait PublisherRequestHandler: Send + Sync + fmt::Debug + 'static {
    /// Return the current daemon lifetime for a transport-generated rejection.
    /// This read-only hook must not enter request dispatch or mutate authority.
    async fn daemon_epoch(&self) -> DaemonEpoch;

    /// Handle one fully decoded request and return one complete framed response.
    async fn handle(
        &self,
        request: RequestEnvelope,
        context: PublisherRequestContext,
    ) -> Result<Vec<u8>, PublisherSocketError>;
}

/// Dedicated publisher socket server and its owned accept task.
pub(crate) struct PublisherSocketServer {
    socket_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl PublisherSocketServer {
    /// Securely bind the dedicated socket and begin serving connections.
    pub(crate) async fn bind(
        config: PublisherSocketConfig,
        handler: Arc<dyn PublisherRequestHandler>,
    ) -> Result<Self, PublisherSocketError> {
        ensure_supported_platform()?;
        let (listener, socket_identity) = bind_socket(&config).await?;
        let socket_path = config.socket_path.clone();
        #[cfg(target_os = "macos")]
        let listener = AsyncFd::new(listener).map_err(PublisherSocketError::Bind)?;
        #[cfg(not(target_os = "macos"))]
        let listener = UnixListener::from_std(listener).map_err(PublisherSocketError::Bind)?;
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let cleanup_path = socket_path.clone();
        let config = Arc::new(config);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    completed = connections.join_next(), if !connections.is_empty() => {
                        if let Some(Err(error)) = completed {
                            debug!(error = %error, "publisher connection task failed");
                        }
                    }
                    accepted = accept_publisher_connection(&listener, &config) => {
                        match accepted {
                            Ok(stream) => {
                                let config = Arc::clone(&config);
                                let handler = Arc::clone(&handler);
                                connections.spawn(async move {
                                    if let Err(error) = serve_connection(stream, config, handler).await {
                                    debug!(error = %error, "publisher connection rejected");
                                    }
                                });
                            }
                            Err(error) => {
                                warn!(error = %error, "publisher socket accept failed");
                            }
                        }
                    }
                }
            }
            while let Some(completed) = connections.join_next().await {
                if let Err(error) = completed {
                    debug!(error = %error, "publisher connection task failed during shutdown");
                }
            }
            remove_owned_socket(&cleanup_path, socket_identity);
        });
        Ok(Self {
            socket_path,
            shutdown: Some(shutdown_tx),
            task: Some(task),
        })
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Stop accepting, wait for the accept task, and remove only this socket.
    pub(crate) async fn shutdown(mut self) -> Result<(), PublisherSocketError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.map_err(PublisherSocketError::Join)?;
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
async fn accept_publisher_connection(
    listener: &UnixListener,
    _config: &PublisherSocketConfig,
) -> Result<tokio::net::UnixStream, PublisherSocketError> {
    listener
        .accept()
        .await
        .map(|(stream, _)| stream)
        .map_err(PublisherSocketError::Io)
}

#[cfg(target_os = "macos")]
async fn accept_publisher_connection(
    listener: &AsyncFd<StdUnixListener>,
    config: &PublisherSocketConfig,
) -> Result<tokio::net::UnixStream, PublisherSocketError> {
    loop {
        let mut readiness = listener
            .readable()
            .await
            .map_err(PublisherSocketError::Io)?;
        let Some(barrier) = config.spawn_barrier.as_ref() else {
            return Err(PublisherSocketError::SpawnBarrierUnavailable);
        };
        let guard = match barrier.enter_descriptor_receipt() {
            Ok(guard) => guard,
            Err(PublisherSocketError::SpawnBarrierBusy) => {
                tokio::time::sleep(Duration::from_millis(1)).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        let accepted = readiness.try_io(|listener| {
            // SAFETY: `listener` is a live AF_UNIX listener. Null address
            // pointers request no peer pathname, and a successful return
            // transfers one new descriptor into this process.
            #[allow(unsafe_code)]
            let raw = unsafe {
                libc::accept(
                    listener.get_ref().as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if raw == -1 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: accept returned a newly owned descriptor.
            #[allow(unsafe_code)]
            let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
            make_close_on_exec_io(&descriptor)?;
            Ok(descriptor)
        });
        drop(guard);
        let descriptor = match accepted {
            Ok(Ok(descriptor)) => descriptor,
            Ok(Err(error)) => return Err(PublisherSocketError::Io(error)),
            Err(_) => continue,
        };
        let stream = UnixStream::from(descriptor);
        stream
            .set_nonblocking(true)
            .map_err(PublisherSocketError::Io)?;
        return tokio::net::UnixStream::from_std(stream).map_err(PublisherSocketError::Io);
    }
}

impl fmt::Debug for PublisherSocketServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublisherSocketServer")
            .field("socket_path", &self.socket_path)
            .field("shutdown_requested", &self.shutdown.is_none())
            .field("running", &self.task.is_some())
            .finish()
    }
}

impl Drop for PublisherSocketServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

/// Fail-closed publisher transport failure.
#[derive(Debug, Error)]
pub(crate) enum PublisherSocketError {
    #[error("publisher socket is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("publisher socket path must have an absolute parent directory")]
    InvalidSocketPath,
    #[error("publisher run directory is not a real daemon-owned directory: {0}")]
    InsecureRunDirectory(String),
    #[error("publisher socket path is occupied by an active or unsafe entry: {0}")]
    UnsafeSocketOccupant(String),
    #[error("failed to bind publisher socket: {0}")]
    Bind(#[source] io::Error),
    #[error("publisher connection exceeded its five-second frame deadline")]
    FrameTimeout,
    #[error("publisher connection I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("publisher request frame is invalid: {0}")]
    InvalidFrame(#[from] FrameError),
    #[error("publisher request carried an invalid descriptor transfer")]
    InvalidDescriptorTransfer,
    #[error("publisher request requires one listener descriptor")]
    ListenerMissing,
    #[error("publisher peer identity is unavailable: {0}")]
    PeerIdentityUnavailable(String),
    #[error("publisher peer UID {actual} does not match daemon UID {expected}")]
    PeerUidMismatch { expected: u32, actual: u32 },
    #[error("transferred descriptor is not an IPv4 TCP listener")]
    ListenerInvalid,
    #[error("transferred listener is not bound exactly to 127.0.0.1 on a nonzero port")]
    ListenerNotIpv4Loopback,
    #[error("transferred listener permits address sharing")]
    ListenerShareable,
    #[error("transferred listener collides with locald front-door port {0}")]
    ListenerFrontDoorConflict(u16),
    #[error("listener network namespace differs from locald")]
    #[cfg(target_os = "linux")]
    NetworkNamespaceMismatch,
    #[error("listener network namespace cannot be verified: {0}")]
    #[cfg(target_os = "linux")]
    NetworkNamespaceUnverifiable(String),
    #[error("macOS descriptor receipt requires the daemon process-spawn barrier")]
    #[cfg_attr(
        not(target_os = "macos"),
        allow(
            dead_code,
            reason = "the descriptor receipt barrier is required only on macOS"
        )
    )]
    SpawnBarrierUnavailable,
    #[error("publisher descriptor receipt collided with process creation; retry the request")]
    #[cfg_attr(
        not(target_os = "macos"),
        allow(
            dead_code,
            reason = "the descriptor receipt barrier is required only on macOS"
        )
    )]
    SpawnBarrierBusy,
    #[error("publisher response frame is invalid")]
    InvalidResponseFrame,
    #[error("publisher accept task failed: {0}")]
    Join(#[source] tokio::task::JoinError),
}

impl PublisherSocketError {
    /// Map transport and capability failures into the stable wire vocabulary.
    pub(crate) const fn stable_code(&self) -> StableErrorCode {
        match self {
            Self::PeerIdentityUnavailable(_) => StableErrorCode::PeerIdentityUnavailable,
            Self::PeerUidMismatch { .. } => StableErrorCode::PeerUidMismatch,
            Self::ListenerMissing => StableErrorCode::ListenerMissing,
            Self::ListenerInvalid | Self::InvalidDescriptorTransfer => {
                StableErrorCode::ListenerInvalid
            }
            Self::ListenerNotIpv4Loopback => StableErrorCode::ListenerNotIpv4Loopback,
            Self::ListenerShareable => StableErrorCode::ListenerShareable,
            Self::ListenerFrontDoorConflict(_) => StableErrorCode::ListenerFrontDoorConflict,
            #[cfg(target_os = "linux")]
            Self::NetworkNamespaceMismatch => StableErrorCode::NetworkNamespaceMismatch,
            #[cfg(target_os = "linux")]
            Self::NetworkNamespaceUnverifiable(_) => StableErrorCode::NetworkNamespaceUnverifiable,
            Self::UnsupportedPlatform | Self::SpawnBarrierUnavailable => {
                StableErrorCode::PublicationUnsupported
            }
            Self::FrameTimeout | Self::InvalidFrame(_) | Self::InvalidResponseFrame => {
                StableErrorCode::InvalidRequest
            }
            Self::InvalidSocketPath
            | Self::InsecureRunDirectory(_)
            | Self::UnsafeSocketOccupant(_)
            | Self::Bind(_)
            | Self::Io(_)
            | Self::SpawnBarrierBusy
            | Self::Join(_) => StableErrorCode::Internal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SocketFileIdentity {
    device: u64,
    inode: u64,
}

enum ReceivedRequest {
    Dispatch {
        stream: UnixStream,
        request: RequestEnvelope,
        context: PublisherRequestContext,
    },
    Reject {
        stream: UnixStream,
        error: ProtocolError,
    },
}

async fn serve_connection(
    stream: tokio::net::UnixStream,
    config: Arc<PublisherSocketConfig>,
    handler: Arc<dyn PublisherRequestHandler>,
) -> Result<(), PublisherSocketError> {
    let stream = stream.into_std().map_err(PublisherSocketError::Io)?;
    stream
        .set_nonblocking(false)
        .map_err(PublisherSocketError::Io)?;
    let received = tokio::task::spawn_blocking(move || receive_request(stream, &config))
        .await
        .map_err(PublisherSocketError::Join)??;
    let (stream, response) = match received {
        ReceivedRequest::Dispatch {
            stream,
            request,
            mut context,
        } => {
            let disconnect_stream = stream.try_clone().map_err(PublisherSocketError::Io)?;
            let (peer_closed_tx, peer_closed_rx) = oneshot::channel();
            let (monitor_stop_tx, monitor_stop_rx) = oneshot::channel();
            let monitor = tokio::spawn(monitor_peer_close(
                disconnect_stream,
                peer_closed_tx,
                monitor_stop_rx,
            ));
            context.peer_closed = Some(peer_closed_rx);
            let response = handler.handle(request, context).await;
            let _ = monitor_stop_tx.send(());
            monitor.await.map_err(PublisherSocketError::Join)?;
            (stream, response?)
        }
        ReceivedRequest::Reject { stream, error } => {
            let epoch = handler.daemon_epoch().await;
            let response =
                encode_response_frame(&ResponseEnvelope::<serde_json::Value>::error(epoch, error))?;
            (stream, response)
        }
    };
    validate_response_frame(&response)?;
    tokio::task::spawn_blocking(move || write_response(stream, &response))
        .await
        .map_err(PublisherSocketError::Join)??;
    Ok(())
}

/// Observe a full peer close without confusing the required request
/// write-half shutdown with cancellation.
///
/// A zero-length send does not add bytes to the response stream. On the two
/// version-1 platforms it succeeds while the peer remains open after
/// `shutdown(SHUT_WR)` and fails with a connection error after the peer closes
/// its socket. Unexpected probe failures fail closed by canceling only the
/// request currently using this connection.
async fn monitor_peer_close(
    stream: UnixStream,
    peer_closed: oneshot::Sender<()>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut interval = tokio::time::interval(PEER_CLOSE_POLL_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => return,
            _ = interval.tick() => {
                match peer_connection_closed(&stream) {
                    Ok(false) => {}
                    Ok(true) => {
                        let _ = peer_closed.send(());
                        return;
                    }
                    Err(error) => {
                        debug!(error = %error, "publisher peer-close probe failed; canceling request");
                        let _ = peer_closed.send(());
                        return;
                    }
                }
            }
        }
    }
}

fn peer_connection_closed(stream: &UnixStream) -> io::Result<bool> {
    loop {
        // SAFETY: `stream` owns a valid socket for this call, the null buffer
        // is valid for a zero-length send, and no bytes are read or written.
        #[allow(unsafe_code)]
        let result = unsafe {
            libc::send(
                stream.as_raw_fd(),
                std::ptr::null(),
                0,
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if result == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return match error.raw_os_error() {
            Some(libc::EPIPE | libc::ECONNRESET | libc::ENOTCONN) => Ok(true),
            _ => Err(error),
        };
    }
}

fn receive_request(
    stream: UnixStream,
    config: &PublisherSocketConfig,
) -> Result<ReceivedRequest, PublisherSocketError> {
    let deadline = Instant::now() + Duration::from_millis(FRAME_TIMEOUT_MS);
    let principal = observe_peer_principal(&stream, config.expected_uid)?;
    let mut frame = Vec::with_capacity(REQUEST_FIXED_BYTES + MAX_FRAME_JSON_BYTES);

    let (first_byte, descriptors) = recv_one_chunk(&stream, 1, deadline, config)?;
    if first_byte.len() != 1 {
        return Err(PublisherSocketError::InvalidDescriptorTransfer);
    }
    frame.extend_from_slice(&first_byte);
    let prelude = DescriptorPrelude::parse(first_byte[0])?;
    let listener_fd = match (prelude, descriptors.len()) {
        (DescriptorPrelude::None, 0) => None,
        (DescriptorPrelude::Listener, 0) => return Err(PublisherSocketError::ListenerMissing),
        (DescriptorPrelude::Listener, 1) => descriptors.into_iter().next(),
        (DescriptorPrelude::None | DescriptorPrelude::Listener, _) => {
            return Err(PublisherSocketError::InvalidDescriptorTransfer);
        }
    };

    let (header, late_descriptors) =
        recv_exact_chunks(&stream, RESPONSE_FIXED_BYTES, deadline, config)?;
    if !late_descriptors.is_empty() {
        return Err(PublisherSocketError::InvalidDescriptorTransfer);
    }
    frame.extend_from_slice(&header);
    let body_length = u32::from_be_bytes(
        header
            .as_slice()
            .try_into()
            .map_err(|_| PublisherSocketError::InvalidDescriptorTransfer)?,
    ) as usize;
    if body_length > MAX_FRAME_JSON_BYTES {
        return Err(PublisherSocketError::InvalidFrame(FrameError::BodyTooLarge));
    }
    let (body, late_descriptors) = recv_exact_chunks(&stream, body_length, deadline, config)?;
    if !late_descriptors.is_empty() {
        return Err(PublisherSocketError::InvalidDescriptorTransfer);
    }
    frame.extend_from_slice(&body);
    require_request_eof(&stream, deadline, config)?;
    let request = match decode_request_frame(&frame) {
        Ok(request) => request,
        Err(error) => {
            let Some(error) = semantic_frame_rejection(&error) else {
                return Err(PublisherSocketError::InvalidFrame(error));
            };
            return Ok(ReceivedRequest::Reject { stream, error });
        }
    };

    let listener = match listener_fd {
        Some(listener_fd) => match validate_listener(listener_fd, &config.front_door_ports) {
            Ok(listener) => Some(listener),
            Err(error) if is_listener_capability_rejection(&error) => {
                let error = ProtocolError::new(error.stable_code(), error.to_string(), None);
                return Ok(ReceivedRequest::Reject { stream, error });
            }
            Err(error) => return Err(error),
        },
        None => None,
    };
    Ok(ReceivedRequest::Dispatch {
        stream,
        request,
        context: PublisherRequestContext {
            principal,
            listener,
            peer_closed: None,
        },
    })
}

fn semantic_frame_rejection(error: &FrameError) -> Option<ProtocolError> {
    let (code, message) = match error {
        FrameError::ProtocolVersionMismatch { actual } => (
            StableErrorCode::ProtocolMismatch,
            format!(
                "publisher protocol version {actual} is not supported; locald requires version 1"
            ),
        ),
        FrameError::Deserialize(error) if error.classify() == serde_json::error::Category::Data => {
            (
                StableErrorCode::InvalidRequest,
                "the publisher request does not satisfy the version-1 schema".to_owned(),
            )
        }
        FrameError::BodyTooLarge
        | FrameError::TruncatedRequestHeader
        | FrameError::TruncatedResponseHeader
        | FrameError::LengthMismatch { .. }
        | FrameError::InvalidDescriptorPrelude(_)
        | FrameError::DescriptorOperationMismatch { .. }
        | FrameError::Serialize(_)
        | FrameError::Deserialize(_) => return None,
    };
    Some(ProtocolError::new(code, message, None))
}

const fn is_listener_capability_rejection(error: &PublisherSocketError) -> bool {
    match error {
        PublisherSocketError::ListenerInvalid
        | PublisherSocketError::ListenerNotIpv4Loopback
        | PublisherSocketError::ListenerShareable
        | PublisherSocketError::ListenerFrontDoorConflict(_) => true,
        #[cfg(target_os = "linux")]
        PublisherSocketError::NetworkNamespaceMismatch
        | PublisherSocketError::NetworkNamespaceUnverifiable(_) => true,
        _ => false,
    }
}

fn recv_exact_chunks(
    stream: &UnixStream,
    length: usize,
    deadline: Instant,
    config: &PublisherSocketConfig,
) -> Result<(Vec<u8>, Vec<OwnedFd>), PublisherSocketError> {
    let mut bytes = Vec::with_capacity(length);
    let mut descriptors = Vec::new();
    while bytes.len() < length {
        let (chunk, received) = recv_one_chunk(stream, length - bytes.len(), deadline, config)?;
        bytes.extend_from_slice(&chunk);
        descriptors.extend(received);
    }
    Ok((bytes, descriptors))
}

fn recv_one_chunk(
    stream: &UnixStream,
    maximum: usize,
    deadline: Instant,
    config: &PublisherSocketConfig,
) -> Result<(Vec<u8>, Vec<OwnedFd>), PublisherSocketError> {
    #[cfg(not(target_os = "macos"))]
    let _ = config;
    if maximum == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    set_remaining_read_timeout(stream, deadline)?;
    let mut buffer = vec![0_u8; maximum.min(8_192)];
    #[cfg(target_os = "macos")]
    let _spawn_guard = config
        .spawn_barrier
        .as_ref()
        .ok_or(PublisherSocketError::SpawnBarrierUnavailable)?
        .enter_descriptor_receipt()?;

    #[cfg(target_os = "linux")]
    let flags = MsgFlags::MSG_CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = MsgFlags::empty();

    let received = loop {
        match receive_owned_chunk(stream, &mut buffer, flags) {
            Ok(received) => break received,
            Err(nix::errno::Errno::EAGAIN) => return Err(PublisherSocketError::FrameTimeout),
            Err(nix::errno::Errno::EINTR) => {}
            Err(error) => return Err(PublisherSocketError::Io(io::Error::from(error))),
        }
    };
    let received_bytes = received.bytes;
    let control_truncated = received.flags.contains(MsgFlags::MSG_CTRUNC);
    let descriptors = received.descriptors;
    // Own every transferred descriptor before any validation can return. This
    // makes surplus, truncated, and mixed ancillary messages leak-free.
    let close_on_exec_error = descriptors
        .iter()
        .find_map(|descriptor| make_close_on_exec(descriptor).err());
    if let Some(error) = close_on_exec_error {
        return Err(error);
    }
    if control_truncated || received.unexpected_control {
        return Err(PublisherSocketError::InvalidDescriptorTransfer);
    }
    if received_bytes == 0 {
        return Err(PublisherSocketError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "publisher closed before completing its request",
        )));
    }
    buffer.truncate(received_bytes);
    Ok((buffer, descriptors))
}

fn require_request_eof(
    stream: &UnixStream,
    deadline: Instant,
    config: &PublisherSocketConfig,
) -> Result<(), PublisherSocketError> {
    #[cfg(not(target_os = "macos"))]
    let _ = config;
    set_remaining_read_timeout(stream, deadline)?;
    let mut byte = [0_u8; 1];

    #[cfg(target_os = "macos")]
    let _spawn_guard = config
        .spawn_barrier
        .as_ref()
        .ok_or(PublisherSocketError::SpawnBarrierUnavailable)?
        .enter_descriptor_receipt()?;

    #[cfg(target_os = "linux")]
    let flags = MsgFlags::MSG_CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = MsgFlags::empty();
    let received = loop {
        match receive_owned_chunk(stream, &mut byte, flags) {
            Ok(received) => break received,
            Err(nix::errno::Errno::EAGAIN) => return Err(PublisherSocketError::FrameTimeout),
            Err(nix::errno::Errno::EINTR) => {}
            Err(error) => return Err(PublisherSocketError::Io(io::Error::from(error))),
        }
    };
    let descriptors = received.descriptors;
    let close_on_exec_error = descriptors
        .iter()
        .find_map(|descriptor| make_close_on_exec(descriptor).err());
    if let Some(error) = close_on_exec_error {
        return Err(error);
    }
    if received.bytes == 0
        && descriptors.is_empty()
        && !received.flags.contains(MsgFlags::MSG_CTRUNC)
        && !received.unexpected_control
    {
        Ok(())
    } else {
        Err(PublisherSocketError::InvalidDescriptorTransfer)
    }
}

struct OwnedReceive {
    bytes: usize,
    flags: MsgFlags,
    descriptors: Vec<OwnedFd>,
    unexpected_control: bool,
}

#[allow(
    unsafe_code,
    reason = "recvmsg control records must be walked directly so SCM_RIGHTS descriptors are owned and closed even when MSG_CTRUNC is set"
)]
fn receive_owned_chunk(
    stream: &UnixStream,
    buffer: &mut [u8],
    flags: MsgFlags,
) -> Result<OwnedReceive, nix::errno::Errno> {
    let mut control = [0_usize; RECEIVE_CONTROL_WORDS];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: all pointers installed below reference live, correctly aligned
    // stack allocations for the duration of recvmsg.
    let mut message = unsafe { zeroed::<libc::msghdr>() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    #[cfg_attr(
        target_os = "linux",
        allow(
            clippy::useless_conversion,
            reason = "libc::msghdr::msg_controllen uses target-specific integer types"
        )
    )]
    let control_length = size_of::<[usize; RECEIVE_CONTROL_WORDS]>()
        .try_into()
        .map_err(|_| nix::errno::Errno::EINVAL)?;
    message.msg_controllen = control_length;

    // SAFETY: `message` describes initialized writable storage and `stream`
    // remains borrowed for the complete syscall.
    let received = unsafe { libc::recvmsg(stream.as_raw_fd(), &raw mut message, flags.bits()) };
    if received < 0 {
        return Err(nix::errno::Errno::last());
    }
    let (descriptors, unexpected_control) = own_received_control_messages(&message);
    Ok(OwnedReceive {
        bytes: received as usize,
        flags: MsgFlags::from_bits_truncate(message.msg_flags),
        descriptors,
        unexpected_control,
    })
}

#[allow(
    unsafe_code,
    reason = "kernel-produced cmsghdr records must be bounds-walked to own every installed SCM_RIGHTS descriptor before rejecting malformed or truncated ancillary data"
)]
fn own_received_control_messages(message: &libc::msghdr) -> (Vec<OwnedFd>, bool) {
    let control_start = message.msg_control as usize;
    #[cfg_attr(
        target_os = "linux",
        allow(
            clippy::unnecessary_cast,
            reason = "libc::msghdr::msg_controllen uses target-specific integer types"
        )
    )]
    let control_end = control_start.saturating_add(message.msg_controllen as usize);
    let mut descriptors = Vec::new();
    let mut unexpected_control = false;
    // SAFETY: recvmsg initialized the control storage described by `message`.
    let mut header = unsafe { libc::CMSG_FIRSTHDR(message) };
    while !header.is_null() {
        let header_address = header as usize;
        if header_address < control_start
            || header_address.saturating_add(size_of::<libc::cmsghdr>()) > control_end
        {
            unexpected_control = true;
            break;
        }
        // SAFETY: the complete fixed header was bounds-checked above.
        let header_value = unsafe { &*header };
        let header_length = header_value.cmsg_len as usize;
        // SAFETY: CMSG_LEN computes only the platform's aligned fixed-header
        // size for an empty payload.
        let data_offset = unsafe { libc::CMSG_LEN(0) as usize };
        // SAFETY: CMSG_DATA performs pointer arithmetic from the checked header.
        let data_address = unsafe { libc::CMSG_DATA(header) as usize };
        let declared_end = header_address.saturating_add(header_length);
        let payload_end = declared_end.min(control_end);
        let header_is_complete = header_length >= data_offset && declared_end <= control_end;
        if header_length < data_offset || data_address > payload_end {
            unexpected_control = true;
            break;
        }
        if header_value.cmsg_level == libc::SOL_SOCKET && header_value.cmsg_type == libc::SCM_RIGHTS
        {
            let descriptor_bytes = payload_end - data_address;
            if !descriptor_bytes.is_multiple_of(size_of::<RawFd>()) {
                unexpected_control = true;
            }
            for offset in (0..descriptor_bytes).step_by(size_of::<RawFd>()) {
                if offset + size_of::<RawFd>() > descriptor_bytes {
                    break;
                }
                // SAFETY: this whole RawFd lies inside the bounds-checked
                // SCM_RIGHTS payload. recvmsg transferred its ownership.
                let raw = unsafe {
                    (data_address as *const RawFd)
                        .add(offset / size_of::<RawFd>())
                        .read_unaligned()
                };
                // SAFETY: every SCM_RIGHTS value installed by recvmsg is now
                // uniquely owned by this process.
                descriptors.push(unsafe { OwnedFd::from_raw_fd(raw) });
            }
        } else {
            unexpected_control = true;
        }
        if !header_is_complete {
            unexpected_control = true;
            break;
        }
        // SAFETY: the current header is complete and lies within `message`.
        header = unsafe { libc::CMSG_NXTHDR(message, header) };
    }
    (descriptors, unexpected_control)
}

fn make_close_on_exec(descriptor: &OwnedFd) -> Result<(), PublisherSocketError> {
    make_close_on_exec_io(descriptor).map_err(PublisherSocketError::Io)
}

fn make_close_on_exec_io(descriptor: &OwnedFd) -> io::Result<()> {
    let current = fcntl(descriptor, FcntlArg::F_GETFD).map_err(io::Error::from)?;
    let flags = FdFlag::from_bits_truncate(current) | FdFlag::FD_CLOEXEC;
    fcntl(descriptor, FcntlArg::F_SETFD(flags)).map_err(io::Error::from)?;
    Ok(())
}

fn write_response(mut stream: UnixStream, response: &[u8]) -> Result<(), PublisherSocketError> {
    let deadline = Instant::now() + Duration::from_millis(FRAME_TIMEOUT_MS);
    let mut remaining = response;
    while !remaining.is_empty() {
        set_remaining_write_timeout(&stream, deadline)?;
        match stream.write(remaining) {
            Ok(0) => {
                return Err(PublisherSocketError::Io(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "publisher response write returned zero",
                )));
            }
            Ok(written) => remaining = &remaining[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(PublisherSocketError::FrameTimeout);
            }
            Err(error) => return Err(PublisherSocketError::Io(error)),
        }
    }
    Ok(())
}

fn validate_response_frame(response: &[u8]) -> Result<(), PublisherSocketError> {
    let header = response
        .get(..RESPONSE_FIXED_BYTES)
        .ok_or(PublisherSocketError::InvalidResponseFrame)?;
    let body_length = u32::from_be_bytes(
        header
            .try_into()
            .map_err(|_| PublisherSocketError::InvalidResponseFrame)?,
    ) as usize;
    if body_length > MAX_FRAME_JSON_BYTES
        || response.len() != RESPONSE_FIXED_BYTES.saturating_add(body_length)
    {
        return Err(PublisherSocketError::InvalidResponseFrame);
    }
    Ok(())
}

fn set_remaining_read_timeout(
    stream: &UnixStream,
    deadline: Instant,
) -> Result<(), PublisherSocketError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(PublisherSocketError::FrameTimeout)?;
    stream
        .set_read_timeout(Some(remaining))
        .map_err(PublisherSocketError::Io)
}

fn set_remaining_write_timeout(
    stream: &UnixStream,
    deadline: Instant,
) -> Result<(), PublisherSocketError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(PublisherSocketError::FrameTimeout)?;
    stream
        .set_write_timeout(Some(remaining))
        .map_err(PublisherSocketError::Io)
}

fn observe_peer_principal(
    stream: &UnixStream,
    expected_uid: u32,
) -> Result<PublisherPrincipalEvidence, PublisherSocketError> {
    let (uid, pid) = peer_uid_pid(stream)?;
    if uid != expected_uid {
        return Err(PublisherSocketError::PeerUidMismatch {
            expected: expected_uid,
            actual: uid,
        });
    }
    let birth = process_birth(pid)?;
    Ok(PublisherPrincipalEvidence { uid, pid, birth })
}

#[cfg(target_os = "linux")]
fn peer_uid_pid(stream: &UnixStream) -> Result<(u32, u32), PublisherSocketError> {
    let credentials = getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
        .map_err(|error| PublisherSocketError::PeerIdentityUnavailable(error.to_string()))?;
    let pid = u32::try_from(credentials.pid()).map_err(|_| {
        PublisherSocketError::PeerIdentityUnavailable("peer PID is not positive".to_owned())
    })?;
    Ok((credentials.uid(), pid))
}

#[cfg(target_os = "macos")]
fn peer_uid_pid(stream: &UnixStream) -> Result<(u32, u32), PublisherSocketError> {
    let (uid, _) = nix::unistd::getpeereid(stream)
        .map_err(|error| PublisherSocketError::PeerIdentityUnavailable(error.to_string()))?;
    let pid = getsockopt(stream, nix::sys::socket::sockopt::LocalPeerPid)
        .map_err(|error| PublisherSocketError::PeerIdentityUnavailable(error.to_string()))?;
    let pid = u32::try_from(pid).map_err(|_| {
        PublisherSocketError::PeerIdentityUnavailable("peer PID is not positive".to_owned())
    })?;
    Ok((uid.as_raw(), pid))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn peer_uid_pid(_stream: &UnixStream) -> Result<(u32, u32), PublisherSocketError> {
    Err(PublisherSocketError::UnsupportedPlatform)
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::disallowed_methods,
    reason = "peer authentication requires one synchronous snapshot of the Linux procfs birth identifiers"
)]
fn process_birth(pid: u32) -> Result<PublisherProcessBirthEvidence, PublisherSocketError> {
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|error| PublisherSocketError::PeerIdentityUnavailable(error.to_string()))?;
    let boot_id = boot_id.trim();
    if boot_id.is_empty() {
        return Err(PublisherSocketError::PeerIdentityUnavailable(
            "Linux boot ID is empty".to_owned(),
        ));
    }
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|error| PublisherSocketError::PeerIdentityUnavailable(error.to_string()))?;
    let command_end = stat.rfind(") ").ok_or_else(|| {
        PublisherSocketError::PeerIdentityUnavailable("malformed Linux process stat".to_owned())
    })?;
    let start_ticks = stat[command_end + 2..]
        .split_ascii_whitespace()
        .nth(19)
        .ok_or_else(|| {
            PublisherSocketError::PeerIdentityUnavailable(
                "Linux process stat has no start time".to_owned(),
            )
        })?
        .parse::<u64>()
        .map_err(|error| PublisherSocketError::PeerIdentityUnavailable(error.to_string()))?;
    Ok(PublisherProcessBirthEvidence::Linux {
        boot_id: boot_id.into(),
        start_ticks,
    })
}

#[cfg(target_os = "macos")]
fn process_birth(pid: u32) -> Result<PublisherProcessBirthEvidence, PublisherSocketError> {
    let pid = i32::try_from(pid).map_err(|_| {
        PublisherSocketError::PeerIdentityUnavailable("peer PID is out of range".to_owned())
    })?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let info_size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).map_err(|_| {
        PublisherSocketError::PeerIdentityUnavailable(
            "Darwin process identity structure is too large".to_owned(),
        )
    })?;
    // SAFETY: `info` points to writable storage of the exact structure and
    // `proc_pidinfo` is called with that structure's Darwin flavor and size.
    #[allow(unsafe_code)]
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            info_size,
        )
    };
    if bytes != info_size {
        return Err(PublisherSocketError::PeerIdentityUnavailable(
            io::Error::last_os_error().to_string(),
        ));
    }
    // SAFETY: Darwin reported that it initialized the complete structure.
    #[allow(unsafe_code)]
    let info = unsafe { info.assume_init() };
    Ok(PublisherProcessBirthEvidence::MacOs {
        start_seconds: info.pbi_start_tvsec,
        start_microseconds: info.pbi_start_tvusec,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_birth(_pid: u32) -> Result<PublisherProcessBirthEvidence, PublisherSocketError> {
    Err(PublisherSocketError::UnsupportedPlatform)
}

fn validate_listener(
    descriptor: OwnedFd,
    front_door_ports: &BTreeSet<u16>,
) -> Result<ValidatedPublisherListener, PublisherSocketError> {
    let socket_type =
        getsockopt(&descriptor, SocketType).map_err(|_| PublisherSocketError::ListenerInvalid)?;
    #[cfg(not(target_os = "macos"))]
    let accepting =
        getsockopt(&descriptor, AcceptConn).map_err(|_| PublisherSocketError::ListenerInvalid)?;
    if socket_type != SockType::Stream {
        return Err(PublisherSocketError::ListenerInvalid);
    }
    #[cfg(not(target_os = "macos"))]
    if !accepting {
        return Err(PublisherSocketError::ListenerInvalid);
    }
    #[cfg(not(target_os = "macos"))]
    let reuse_port =
        getsockopt(&descriptor, ReusePort).map_err(|_| PublisherSocketError::ListenerInvalid)?;
    #[cfg(not(target_os = "macos"))]
    if reuse_port {
        return Err(PublisherSocketError::ListenerShareable);
    }

    let listener = TcpListener::from(descriptor);
    let address = listener
        .local_addr()
        .map_err(|_| PublisherSocketError::ListenerInvalid)?;
    let SocketAddr::V4(address) = address else {
        return Err(PublisherSocketError::ListenerNotIpv4Loopback);
    };
    if *address.ip() != Ipv4Addr::LOCALHOST || address.port() == 0 {
        return Err(PublisherSocketError::ListenerNotIpv4Loopback);
    }
    if front_door_ports.contains(&address.port()) {
        return Err(PublisherSocketError::ListenerFrontDoorConflict(
            address.port(),
        ));
    }
    let identity = platform_listener_identity(&listener, address.port())?;
    Ok(ValidatedPublisherListener {
        identity,
        guard: Arc::new(listener),
    })
}

#[cfg(target_os = "linux")]
fn platform_listener_identity(
    listener: &TcpListener,
    port: u16,
) -> Result<PublisherListenerIdentity, PublisherSocketError> {
    let socket_cookie = linux_socket_cookie(listener.as_fd(), libc::SO_COOKIE)
        .map_err(|_| PublisherSocketError::ListenerInvalid)?;
    let listener_namespace = linux_socket_cookie(listener.as_fd(), libc::SO_NETNS_COOKIE)
        .map_err(|error| PublisherSocketError::NetworkNamespaceUnverifiable(error.to_string()))?;
    let reference = create_linux_namespace_reference()
        .map_err(|error| PublisherSocketError::NetworkNamespaceUnverifiable(error.to_string()))?;
    let daemon_namespace = linux_socket_cookie(reference.as_fd(), libc::SO_NETNS_COOKIE)
        .map_err(|error| PublisherSocketError::NetworkNamespaceUnverifiable(error.to_string()))?;
    if listener_namespace != daemon_namespace {
        return Err(PublisherSocketError::NetworkNamespaceMismatch);
    }
    Ok(PublisherListenerIdentity::LinuxIpv4 {
        address: Ipv4Addr::LOCALHOST.octets(),
        port,
        socket_cookie,
        network_namespace_cookie: listener_namespace,
    })
}

#[cfg(target_os = "linux")]
fn linux_socket_cookie(
    descriptor: std::os::fd::BorrowedFd<'_>,
    option: libc::c_int,
) -> io::Result<u64> {
    let mut value = 0_u64;
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<u64>()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket-cookie storage length does not fit socklen_t",
        )
    })?;
    // SAFETY: the descriptor is borrowed for the syscall and the output points
    // to initialized, correctly sized writable storage.
    #[allow(unsafe_code)]
    let result = unsafe {
        libc::getsockopt(
            descriptor.as_raw_fd(),
            libc::SOL_SOCKET,
            option,
            (&raw mut value).cast(),
            &raw mut length,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<u64>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "kernel returned an invalid socket-cookie length",
        ));
    }
    Ok(value)
}

#[cfg(target_os = "linux")]
fn create_linux_namespace_reference() -> io::Result<OwnedFd> {
    // SAFETY: `socket` returns a new descriptor or -1. Successful ownership is
    // transferred immediately to `OwnedFd`.
    #[allow(unsafe_code)]
    let descriptor = unsafe {
        libc::socket(
            libc::AF_INET,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            libc::IPPROTO_TCP,
        )
    };
    if descriptor == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful `socket` call returned a uniquely owned fd.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

#[cfg(target_os = "macos")]
fn platform_listener_identity(
    listener: &TcpListener,
    port: u16,
) -> Result<PublisherListenerIdentity, PublisherSocketError> {
    let pcb_generation = macos_listener_generation(listener.as_raw_fd())?;
    Ok(PublisherListenerIdentity::MacOsIpv4 {
        address: Ipv4Addr::LOCALHOST.octets(),
        port,
        pcb_generation,
    })
}

#[cfg(target_os = "macos")]
fn macos_listener_generation(descriptor: RawFd) -> Result<u64, PublisherSocketError> {
    let info = libproc::file_info::pidfdinfo::<libproc::net_info::SocketFDInfo>(
        std::process::id() as i32,
        descriptor,
    )
    .map_err(|_| PublisherSocketError::ListenerInvalid)?;
    if info.psi.soi_type != libc::SOCK_STREAM
        || info.psi.soi_protocol != libc::IPPROTO_TCP
        || info.psi.soi_family != libc::AF_INET
        || !matches!(
            libproc::net_info::SocketInfoKind::from(info.psi.soi_kind),
            libproc::net_info::SocketInfoKind::Tcp
        )
    {
        return Err(PublisherSocketError::ListenerInvalid);
    }
    let options = libc::c_int::from(info.psi.soi_options);
    if options & libc::SO_ACCEPTCONN == 0 {
        return Err(PublisherSocketError::ListenerInvalid);
    }
    if options & libc::SO_REUSEPORT != 0 {
        return Err(PublisherSocketError::ListenerShareable);
    }
    // SAFETY: `soi_kind == SOCKINFO_TCP`, so Darwin initialized the TCP arm of
    // this C union.
    #[allow(unsafe_code)]
    let tcp = unsafe { info.psi.soi_proto.pri_tcp };
    if !matches!(
        libproc::net_info::TcpSIState::from(tcp.tcpsi_state),
        libproc::net_info::TcpSIState::Listen
    ) {
        return Err(PublisherSocketError::ListenerInvalid);
    }
    Ok(tcp.tcpsi_ini.insi_gencnt)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_listener_identity(
    _listener: &TcpListener,
    _port: u16,
) -> Result<PublisherListenerIdentity, PublisherSocketError> {
    Err(PublisherSocketError::UnsupportedPlatform)
}

async fn bind_socket(
    config: &PublisherSocketConfig,
) -> Result<(StdUnixListener, SocketFileIdentity), PublisherSocketError> {
    let parent = config
        .socket_path
        .parent()
        .filter(|parent| parent.is_absolute())
        .ok_or(PublisherSocketError::InvalidSocketPath)?;
    prepare_run_directory(parent, config.expected_uid)?;
    remove_safe_stale_socket(
        &config.socket_path,
        config.expected_uid,
        config.spawn_barrier.as_ref(),
    )
    .await?;
    #[cfg(target_os = "macos")]
    let listener = loop {
        let barrier = config
            .spawn_barrier
            .as_ref()
            .ok_or(PublisherSocketError::SpawnBarrierUnavailable)?;
        match barrier.enter_descriptor_receipt() {
            Ok(guard) => {
                let listener = StdUnixListener::bind(&config.socket_path)
                    .map_err(PublisherSocketError::Bind)?;
                drop(guard);
                break listener;
            }
            Err(PublisherSocketError::SpawnBarrierBusy) => {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Err(error) => return Err(error),
        }
    };
    #[cfg(not(target_os = "macos"))]
    let listener =
        StdUnixListener::bind(&config.socket_path).map_err(PublisherSocketError::Bind)?;
    fs::set_permissions(
        &config.socket_path,
        fs::Permissions::from_mode(PUBLISHER_SOCKET_MODE),
    )
    .map_err(PublisherSocketError::Bind)?;
    let metadata = fs::symlink_metadata(&config.socket_path).map_err(PublisherSocketError::Bind)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != config.expected_uid
        || metadata.mode() & 0o777 != PUBLISHER_SOCKET_MODE
    {
        return Err(PublisherSocketError::UnsafeSocketOccupant(
            config.socket_path.display().to_string(),
        ));
    }
    listener
        .set_nonblocking(true)
        .map_err(PublisherSocketError::Bind)?;
    Ok((
        listener,
        SocketFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    ))
}

fn prepare_run_directory(path: &Path, expected_uid: u32) -> Result<(), PublisherSocketError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != expected_uid
            {
                return Err(PublisherSocketError::InsecureRunDirectory(
                    path.display().to_string(),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| {
                PublisherSocketError::InsecureRunDirectory(format!("{}: {error}", path.display()))
            })?;
        }
        Err(error) => {
            return Err(PublisherSocketError::InsecureRunDirectory(format!(
                "{}: {error}",
                path.display()
            )));
        }
    }
    fs::set_permissions(path, fs::Permissions::from_mode(RUN_DIRECTORY_MODE)).map_err(|error| {
        PublisherSocketError::InsecureRunDirectory(format!("{}: {error}", path.display()))
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        PublisherSocketError::InsecureRunDirectory(format!("{}: {error}", path.display()))
    })?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_uid
        || metadata.mode() & 0o777 != RUN_DIRECTORY_MODE
    {
        return Err(PublisherSocketError::InsecureRunDirectory(
            path.display().to_string(),
        ));
    }
    Ok(())
}

async fn remove_safe_stale_socket(
    path: &Path,
    expected_uid: u32,
    spawn_barrier: Option<&Arc<dyn PublisherSpawnBarrier>>,
) -> Result<(), PublisherSocketError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(PublisherSocketError::UnsafeSocketOccupant(format!(
                "{}: {error}",
                path.display()
            )));
        }
    };
    if !metadata.file_type().is_socket() || metadata.uid() != expected_uid {
        return Err(PublisherSocketError::UnsafeSocketOccupant(
            path.display().to_string(),
        ));
    }
    match connect_for_stale_check(path, spawn_barrier).await? {
        Ok(_) => {
            return Err(PublisherSocketError::UnsafeSocketOccupant(format!(
                "{} has an active listener",
                path.display()
            )));
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) => {}
        Err(error) => {
            return Err(PublisherSocketError::UnsafeSocketOccupant(format!(
                "cannot prove {} is stale: {error}",
                path.display()
            )));
        }
    }
    if path.exists() {
        fs::remove_file(path).map_err(|error| {
            PublisherSocketError::UnsafeSocketOccupant(format!(
                "cannot remove stale {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[allow(
    clippy::unused_async,
    reason = "the shared platform call site awaits the macOS spawn barrier while non-macOS deliberately performs the same blocking connect inline"
)]
async fn connect_for_stale_check(
    path: &Path,
    _spawn_barrier: Option<&Arc<dyn PublisherSpawnBarrier>>,
) -> Result<io::Result<UnixStream>, PublisherSocketError> {
    Ok(UnixStream::connect(path))
}

#[cfg(target_os = "macos")]
async fn connect_for_stale_check(
    path: &Path,
    spawn_barrier: Option<&Arc<dyn PublisherSpawnBarrier>>,
) -> Result<io::Result<UnixStream>, PublisherSocketError> {
    let barrier = spawn_barrier.ok_or(PublisherSocketError::SpawnBarrierUnavailable)?;
    loop {
        match barrier.enter_descriptor_receipt() {
            Ok(guard) => {
                let connection = UnixStream::connect(path);
                drop(guard);
                return Ok(connection);
            }
            Err(PublisherSocketError::SpawnBarrierBusy) => {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

fn remove_owned_socket(path: &Path, identity: SocketFileIdentity) {
    let owned = fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_socket()
            && metadata.dev() == identity.device
            && metadata.ino() == identity.inode
    });
    if owned {
        if let Err(error) = fs::remove_file(path) {
            warn!(path = %path.display(), error = %error, "failed to remove publisher socket");
        }
    }
}

const fn ensure_supported_platform() -> Result<(), PublisherSocketError> {
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        Ok(())
    } else {
        Err(PublisherSocketError::UnsupportedPlatform)
    }
}

#[cfg(test)]
fn test_spawn_barrier() -> Option<Arc<dyn PublisherSpawnBarrier>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(TestSpawnBarrier))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(all(test, target_os = "macos"))]
#[derive(Debug)]
struct TestSpawnBarrier;

#[cfg(all(test, target_os = "macos"))]
struct TestSpawnBarrierGuard;

#[cfg(all(test, target_os = "macos"))]
impl PublisherSpawnBarrierGuard for TestSpawnBarrierGuard {}

#[cfg(all(test, target_os = "macos"))]
impl PublisherSpawnBarrier for TestSpawnBarrier {
    fn enter_descriptor_receipt(
        &self,
    ) -> Result<Box<dyn PublisherSpawnBarrierGuard + '_>, PublisherSocketError> {
        Ok(Box::new(TestSpawnBarrierGuard))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locald_publisher_protocol::{
        AcquireArguments, AcquisitionAttemptHandle, DaemonEpoch, PublisherRequest,
        ReleaseArguments, encode_request_frame,
    };
    use nix::sys::socket::{ControlMessage, sendmsg};
    use std::io::{IoSlice, Read as _};
    #[cfg(target_os = "linux")]
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Debug)]
    struct PeerCloseTestHandler {
        entered: Arc<Notify>,
        canceled: Arc<Notify>,
    }

    #[derive(Debug)]
    struct RejectionTestHandler {
        epoch: DaemonEpoch,
        dispatches: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl PublisherRequestHandler for RejectionTestHandler {
        async fn daemon_epoch(&self) -> DaemonEpoch {
            self.epoch.clone()
        }

        async fn handle(
            &self,
            _request: RequestEnvelope,
            _context: PublisherRequestContext,
        ) -> Result<Vec<u8>, PublisherSocketError> {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            encode_response_frame(&ResponseEnvelope::success(
                self.epoch.clone(),
                serde_json::json!({}),
            ))
            .map_err(PublisherSocketError::InvalidFrame)
        }
    }

    #[async_trait]
    impl PublisherRequestHandler for PeerCloseTestHandler {
        async fn daemon_epoch(&self) -> DaemonEpoch {
            DaemonEpoch::from_bytes([1; 16])
        }

        async fn handle(
            &self,
            _request: RequestEnvelope,
            mut context: PublisherRequestContext,
        ) -> Result<Vec<u8>, PublisherSocketError> {
            self.entered.notify_one();
            context.wait_for_peer_close().await;
            self.canceled.notify_one();
            Ok(vec![0, 0, 0, 2, b'{', b'}'])
        }
    }

    fn release_request() -> RequestEnvelope {
        RequestEnvelope::v1(
            DaemonEpoch::from_bytes([1; 16]),
            PublisherRequest::Release(ReleaseArguments {
                lease_handle: locald_publisher_protocol::LeaseHandle::from_bytes([2; 32]),
            }),
        )
    }

    fn acquire_request() -> RequestEnvelope {
        RequestEnvelope::v1(
            DaemonEpoch::from_bytes([1; 16]),
            PublisherRequest::Acquire(AcquireArguments {
                acquisition_attempt_handle: AcquisitionAttemptHandle::from_bytes([2; 32]),
                acknowledged_origin: locald_publisher_protocol::SemanticOrigin::parse(
                    "https://workbench.example.localhost",
                )
                .expect("test origin is valid"),
            }),
        )
    }

    fn write_frame(stream: &mut UnixStream, request: &RequestEnvelope, descriptor: Option<RawFd>) {
        let frame = encode_request_frame(request).expect("test request encodes");
        let first = [IoSlice::new(&frame.as_bytes()[..1])];
        let sent = match descriptor {
            Some(descriptor) => sendmsg::<nix::sys::socket::UnixAddr>(
                stream.as_raw_fd(),
                &first,
                &[ControlMessage::ScmRights(&[descriptor])],
                MsgFlags::empty(),
                None,
            )
            .expect("descriptor send succeeds"),
            None => sendmsg::<nix::sys::socket::UnixAddr>(
                stream.as_raw_fd(),
                &first,
                &[],
                MsgFlags::empty(),
                None,
            )
            .expect("frame send succeeds"),
        };
        assert_eq!(sent, 1);
        stream
            .write_all(&frame.as_bytes()[1..])
            .expect("frame body writes");
    }

    fn send_frame(stream: &mut UnixStream, request: &RequestEnvelope, descriptor: Option<RawFd>) {
        write_frame(stream, request, descriptor);
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("finish publisher request");
    }

    fn send_raw_body(socket_path: &Path, prelude: DescriptorPrelude, body: &[u8]) -> UnixStream {
        let mut publisher = UnixStream::connect(socket_path).expect("connect publisher");
        let length = u32::try_from(body.len())
            .expect("test frame length fits")
            .to_be_bytes();
        publisher
            .write_all(&[prelude as u8])
            .expect("write descriptor prelude");
        publisher.write_all(&length).expect("write body length");
        publisher.write_all(body).expect("write request body");
        publisher
            .shutdown(std::net::Shutdown::Write)
            .expect("finish publisher request");
        publisher
    }

    fn read_rejection(mut publisher: UnixStream, expected_epoch: &DaemonEpoch) -> StableErrorCode {
        publisher
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("bound rejection read");
        let mut frame = Vec::new();
        publisher
            .read_to_end(&mut frame)
            .expect("read framed publisher rejection");
        assert!(!frame.is_empty(), "semantic rejection must be framed");
        let response =
            locald_publisher_protocol::decode_response_frame::<serde_json::Value>(&frame)
                .expect("decode publisher rejection");
        assert_eq!(response.daemon_epoch(), expected_epoch);
        response
            .into_result()
            .expect_err("semantic rejection is not a success")
            .code()
    }

    fn assert_silent_close(mut publisher: UnixStream) {
        publisher
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("bound silent close read");
        let mut response = Vec::new();
        match publisher.read_to_end(&mut response) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
            Err(error) => panic!("read silent transport close: {error}"),
        }
        assert!(
            response.is_empty(),
            "transport, framing, and ancillary failures close without a response"
        );
    }

    fn request_json(request: &RequestEnvelope) -> serde_json::Value {
        serde_json::to_value(request).expect("serialize request as test JSON")
    }

    #[test]
    fn peer_close_probe_distinguishes_request_eof_from_full_close() {
        let (sender, receiver) = UnixStream::pair().expect("socket pair");
        sender
            .shutdown(std::net::Shutdown::Write)
            .expect("finish publisher request");
        assert!(
            !peer_connection_closed(&receiver).expect("inspect cooperative request EOF"),
            "request write-half EOF is framing, not cancellation"
        );
        drop(sender);
        assert!(
            peer_connection_closed(&receiver).expect("inspect full publisher close"),
            "closing the complete publisher socket is cancellation"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn socket_server_delivers_only_full_peer_close_as_cancellation() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let socket_path = temporary.path().join("run/publisher-v1.sock");
        let entered = Arc::new(Notify::new());
        let canceled = Arc::new(Notify::new());
        let server = PublisherSocketServer::bind(
            PublisherSocketConfig::for_test(socket_path.clone(), []),
            Arc::new(PeerCloseTestHandler {
                entered: Arc::clone(&entered),
                canceled: Arc::clone(&canceled),
            }),
        )
        .await
        .expect("bind publisher socket");

        let mut publisher = UnixStream::connect(&socket_path).expect("connect publisher");
        send_frame(&mut publisher, &release_request(), None);
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("dispatch starts after request EOF");
        assert!(
            tokio::time::timeout(Duration::from_millis(75), canceled.notified())
                .await
                .is_err(),
            "cooperative request EOF keeps the waiter active"
        );

        drop(publisher);
        tokio::time::timeout(Duration::from_secs(1), canceled.notified())
            .await
            .expect("full publisher close cancels the waiter");
        tokio::time::timeout(Duration::from_secs(1), server.shutdown())
            .await
            .expect("server joins the canceled connection task")
            .expect("publisher server shuts down");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_socket_frames_only_authenticated_semantic_and_listener_rejections() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let socket_path = temporary.path().join("run/publisher-v1.sock");
        let epoch = DaemonEpoch::from_bytes([1; 16]);
        let dispatches = Arc::new(AtomicUsize::new(0));
        let server = PublisherSocketServer::bind(
            PublisherSocketConfig::for_test(socket_path.clone(), []),
            Arc::new(RejectionTestHandler {
                epoch: epoch.clone(),
                dispatches: Arc::clone(&dispatches),
            }),
        )
        .await
        .expect("bind publisher socket");

        let mut unknown_field = request_json(&release_request());
        unknown_field
            .as_object_mut()
            .expect("request is an object")
            .insert("unexpected".to_owned(), serde_json::Value::Bool(true));
        let unknown_field = serde_json::to_vec(&unknown_field).expect("encode unknown field");
        assert_eq!(
            read_rejection(
                send_raw_body(&socket_path, DescriptorPrelude::None, &unknown_field),
                &epoch,
            ),
            StableErrorCode::InvalidRequest
        );

        let mut bad_scalar = request_json(&release_request());
        bad_scalar["arguments"]["lease_handle"] = serde_json::Value::String("bad".to_owned());
        let bad_scalar = serde_json::to_vec(&bad_scalar).expect("encode bad scalar");
        assert_eq!(
            read_rejection(
                send_raw_body(&socket_path, DescriptorPrelude::None, &bad_scalar),
                &epoch,
            ),
            StableErrorCode::InvalidRequest
        );

        let mut protocol_mismatch = request_json(&release_request());
        protocol_mismatch["protocol_version"] = serde_json::Value::from(2);
        let protocol_mismatch =
            serde_json::to_vec(&protocol_mismatch).expect("encode protocol mismatch");
        assert_eq!(
            read_rejection(
                send_raw_body(&socket_path, DescriptorPrelude::None, &protocol_mismatch,),
                &epoch,
            ),
            StableErrorCode::ProtocolMismatch
        );

        let invalid_listener = std::fs::File::open("/dev/null").expect("open non-socket FD");
        let mut publisher = UnixStream::connect(&socket_path).expect("connect publisher");
        send_frame(
            &mut publisher,
            &acquire_request(),
            Some(invalid_listener.as_raw_fd()),
        );
        assert_eq!(
            read_rejection(publisher, &epoch),
            StableErrorCode::ListenerInvalid
        );

        assert_silent_close(send_raw_body(
            &socket_path,
            DescriptorPrelude::None,
            br#"{"protocol_version":1,"#,
        ));

        let late_descriptor = std::fs::File::open("/dev/null").expect("open late FD");
        let frame = encode_request_frame(&release_request()).expect("encode release frame");
        let publisher = UnixStream::connect(&socket_path).expect("connect publisher");
        let first = [IoSlice::new(&frame.as_bytes()[..1])];
        assert_eq!(
            sendmsg::<nix::sys::socket::UnixAddr>(
                publisher.as_raw_fd(),
                &first,
                &[],
                MsgFlags::empty(),
                None,
            )
            .expect("send descriptor-free prelude"),
            1
        );
        let late = [IoSlice::new(&frame.as_bytes()[1..])];
        assert!(
            sendmsg::<nix::sys::socket::UnixAddr>(
                publisher.as_raw_fd(),
                &late,
                &[ControlMessage::ScmRights(&[late_descriptor.as_raw_fd()])],
                MsgFlags::empty(),
                None,
            )
            .expect("send late descriptor")
                > 0
        );
        let _ = publisher.shutdown(std::net::Shutdown::Write);
        assert_silent_close(publisher);

        assert_eq!(
            dispatches.load(Ordering::SeqCst),
            0,
            "rejected requests never cross the mutation-dispatch boundary"
        );
        server.shutdown().await.expect("stop publisher socket");
    }

    #[test]
    fn real_socket_frame_authenticates_peer_without_a_descriptor() {
        let (mut sender, receiver) = UnixStream::pair().expect("socket pair");
        send_frame(&mut sender, &release_request(), None);
        let config = PublisherSocketConfig::for_test(PathBuf::from("/tmp/unused"), []);
        let received = receive_request(receiver, &config).expect("request is accepted");
        let ReceivedRequest::Dispatch {
            request, context, ..
        } = received
        else {
            panic!("valid request must reach dispatch");
        };
        assert_eq!(request, release_request());
        assert_eq!(context.principal.uid(), config.expected_uid);
        assert!(context.listener.is_none());
    }

    #[test]
    fn real_socket_frame_receives_and_validates_exact_listener_capability() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback listener");
        let port = listener.local_addr().expect("listener address").port();
        let (mut sender, receiver) = UnixStream::pair().expect("socket pair");
        send_frame(&mut sender, &acquire_request(), Some(listener.as_raw_fd()));
        let config = PublisherSocketConfig::for_test(PathBuf::from("/tmp/unused"), []);
        let received = receive_request(receiver, &config).expect("request is accepted");
        let ReceivedRequest::Dispatch {
            request, context, ..
        } = received
        else {
            panic!("valid listener request must reach dispatch");
        };
        assert_eq!(request, acquire_request());
        let capability = context.listener.expect("listener capability");
        assert_eq!(
            capability.identity().address(),
            Ipv4Addr::LOCALHOST.octets()
        );
        assert_eq!(capability.identity().port(), port);
        drop(listener);
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err());
        drop(capability);
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok());
    }

    #[test]
    fn descriptor_contract_rejects_missing_and_surplus_descriptors() {
        let (mut sender, receiver) = UnixStream::pair().expect("socket pair");
        send_frame(&mut sender, &acquire_request(), None);
        let config = PublisherSocketConfig::for_test(PathBuf::from("/tmp/unused"), []);
        assert!(matches!(
            receive_request(receiver, &config),
            Err(PublisherSocketError::ListenerMissing)
        ));

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback listener");
        let (mut sender, receiver) = UnixStream::pair().expect("socket pair");
        send_frame(&mut sender, &release_request(), Some(listener.as_raw_fd()));
        assert!(matches!(
            receive_request(receiver, &config),
            Err(PublisherSocketError::InvalidDescriptorTransfer)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn truncated_descriptor_control_is_owned_before_rejection() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback listener");
        let duplicates = (0..=MAX_RECEIVED_DESCRIPTORS)
            .map(|_| nix::unistd::dup(&listener).expect("duplicate listener"))
            .collect::<Vec<_>>();
        let raw_descriptors = duplicates
            .iter()
            .map(AsRawFd::as_raw_fd)
            .collect::<Vec<_>>();
        let (sender, receiver) = UnixStream::pair().expect("socket pair");
        let sent = sendmsg::<nix::sys::socket::UnixAddr>(
            sender.as_raw_fd(),
            &[IoSlice::new(&[DescriptorPrelude::Listener as u8])],
            &[ControlMessage::ScmRights(&raw_descriptors)],
            MsgFlags::empty(),
            None,
        )
        .expect("send oversized descriptor control");
        assert_eq!(sent, 1);

        let mut byte = [0_u8; 1];
        let received = receive_owned_chunk(&receiver, &mut byte, MsgFlags::MSG_CMSG_CLOEXEC)
            .expect("receive truncated control while retaining installed descriptors");
        assert!(received.flags.contains(MsgFlags::MSG_CTRUNC));
        assert_eq!(received.descriptors.len(), MAX_RECEIVED_DESCRIPTORS);
        let installed = received
            .descriptors
            .iter()
            .map(AsRawFd::as_raw_fd)
            .collect::<Vec<_>>();
        for descriptor in &received.descriptors {
            let flags = nix::fcntl::fcntl(descriptor, FcntlArg::F_GETFD)
                .expect("installed descriptor remains owned");
            assert!(FdFlag::from_bits_truncate(flags).contains(FdFlag::FD_CLOEXEC));
        }
        drop(received);
        for descriptor in installed {
            // SAFETY: querying an invalid raw descriptor is defined to fail
            // with EBADF and does not dereference user memory.
            #[allow(unsafe_code)]
            let result = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
            assert_eq!(
                result, -1,
                "installed descriptor unexpectedly remained open"
            );
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF),
                "every installed descriptor closes on rejection"
            );
        }
    }

    #[test]
    fn listener_front_door_collision_is_rejected() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback listener");
        let port = listener.local_addr().expect("listener address").port();
        let duplicated = nix::unistd::dup(&listener).expect("duplicate listener");
        assert!(matches!(
            validate_listener(duplicated, &BTreeSet::from([port])),
            Err(PublisherSocketError::ListenerFrontDoorConflict(actual)) if actual == port
        ));
    }

    #[tokio::test]
    async fn stale_socket_cleanup_rejects_active_and_removes_inactive_occupants() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let run = temporary.path().join("run");
        let socket = run.join("publisher-v1.sock");
        prepare_run_directory(&run, nix::unistd::geteuid().as_raw()).expect("secure run dir");
        let listener = StdUnixListener::bind(&socket).expect("active socket");
        let spawn_barrier = test_spawn_barrier();
        assert!(matches!(
            remove_safe_stale_socket(
                &socket,
                nix::unistd::geteuid().as_raw(),
                spawn_barrier.as_ref(),
            )
            .await,
            Err(PublisherSocketError::UnsafeSocketOccupant(_))
        ));
        drop(listener);
        remove_safe_stale_socket(
            &socket,
            nix::unistd::geteuid().as_raw(),
            spawn_barrier.as_ref(),
        )
        .await
        .expect("stale socket removed");
        assert!(!socket.exists());
    }

    #[test]
    fn response_validation_is_exact_and_bounded() {
        assert!(validate_response_frame(&[0, 0, 0, 2, b'{', b'}']).is_ok());
        assert!(matches!(
            validate_response_frame(&[0, 0, 0, 1, b'{', b'}']),
            Err(PublisherSocketError::InvalidResponseFrame)
        ));
    }

    #[test]
    fn trailing_buffered_bytes_are_rejected() {
        let (mut sender, receiver) = UnixStream::pair().expect("socket pair");
        write_frame(&mut sender, &release_request(), None);
        sender.write_all(b"late").expect("trailing bytes write");
        sender
            .shutdown(std::net::Shutdown::Write)
            .expect("finish malformed publisher request");
        let config = PublisherSocketConfig::for_test(PathBuf::from("/tmp/unused"), []);
        assert!(matches!(
            receive_request(receiver, &config),
            Err(PublisherSocketError::InvalidDescriptorTransfer)
        ));
        let mut ignored = [0_u8; 1];
        let _ = sender.read(&mut ignored);
    }
}
