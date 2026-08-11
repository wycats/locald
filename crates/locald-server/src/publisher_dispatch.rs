//! Typed dispatch from the authenticated publisher socket into daemon-owned
//! publication authority.

#![allow(clippy::redundant_pub_crate)] // This sibling-only adapter is intentionally crate-internal.

use crate::manager::ProcessManager;
use crate::publication::{
    ListenerIdentity, PublisherPrincipal, PublisherProcessBirth, RetainedListenerCapability,
};
use crate::publisher_transport::{
    PublisherListenerIdentity, PublisherProcessBirthEvidence, PublisherRequestContext,
    PublisherRequestHandler, PublisherSocketError,
};
use async_trait::async_trait;
use locald_publisher_protocol as protocol;
use std::fmt;

#[derive(Clone)]
pub(crate) struct PublisherDispatcher {
    manager: ProcessManager,
}

impl PublisherDispatcher {
    pub(crate) const fn new(manager: ProcessManager) -> Self {
        Self { manager }
    }

    fn principal(context: &PublisherRequestContext) -> PublisherPrincipal {
        let observed = context.principal();
        let birth = match observed.birth() {
            #[cfg(target_os = "macos")]
            PublisherProcessBirthEvidence::MacOs { process_id_version } => {
                PublisherProcessBirth::MacOs {
                    process_id_version: *process_id_version,
                }
            }
            #[cfg(target_os = "linux")]
            PublisherProcessBirthEvidence::Linux {
                boot_id,
                start_ticks,
            } => PublisherProcessBirth::Linux {
                boot_id: boot_id.clone(),
                start_ticks: *start_ticks,
            },
        };
        PublisherPrincipal::new(observed.uid(), observed.pid(), birth)
    }

    fn take_listener(
        context: &mut PublisherRequestContext,
    ) -> Result<RetainedListenerCapability, protocol::ProtocolError> {
        let listener = context.take_listener().ok_or_else(|| {
            protocol::ProtocolError::new(
                protocol::StableErrorCode::ListenerMissing,
                "this publication operation requires exactly one listener capability",
                None,
            )
        })?;
        let (identity, guard) = listener.into_parts();
        let identity = match identity {
            #[cfg(target_os = "macos")]
            PublisherListenerIdentity::MacOsIpv4 {
                address,
                port,
                pcb_generation,
            } => ListenerIdentity::MacOsIpv4 {
                address,
                port,
                pcb_generation,
            },
            #[cfg(target_os = "linux")]
            PublisherListenerIdentity::LinuxIpv4 {
                address,
                port,
                socket_cookie,
                network_namespace_cookie,
            } => ListenerIdentity::LinuxIpv4 {
                address,
                port,
                socket_cookie,
                network_namespace_cookie,
            },
        };
        Ok(RetainedListenerCapability::new(identity, guard))
    }

    fn success<T: serde::Serialize>(
        epoch: protocol::DaemonEpoch,
        value: T,
    ) -> Result<Vec<u8>, PublisherSocketError> {
        protocol::encode_response_frame(&protocol::ResponseEnvelope::success(epoch, value))
            .map_err(|_| PublisherSocketError::InvalidResponseFrame)
    }

    fn failure(
        epoch: protocol::DaemonEpoch,
        error: protocol::ProtocolError,
    ) -> Result<Vec<u8>, PublisherSocketError> {
        protocol::encode_response_frame(&protocol::ResponseEnvelope::<serde_json::Value>::error(
            epoch, error,
        ))
        .map_err(|_| PublisherSocketError::InvalidResponseFrame)
    }

    fn wait_connection_closed() -> protocol::ProtocolError {
        protocol::ProtocolError::new(
            protocol::StableErrorCode::OperationCanceled,
            "the readiness wait was canceled because its publisher connection closed",
            None,
        )
    }
}

impl fmt::Debug for PublisherDispatcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublisherDispatcher")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PublisherRequestHandler for PublisherDispatcher {
    async fn daemon_epoch(&self) -> protocol::DaemonEpoch {
        self.manager.publisher_authority().epoch().await
    }

    async fn handle(
        &self,
        request: protocol::RequestEnvelope,
        mut context: PublisherRequestContext,
    ) -> Result<Vec<u8>, PublisherSocketError> {
        let authority = self.manager.publisher_authority();
        let epoch = authority.epoch().await;
        if request.daemon_epoch() != &epoch {
            return Self::failure(
                epoch,
                protocol::ProtocolError::new(
                    protocol::StableErrorCode::DaemonEpochChanged,
                    "the request names a previous locald daemon lifetime",
                    None,
                ),
            );
        }
        let principal = Self::principal(&context);
        let response = match request.into_request() {
            protocol::PublisherRequest::BeginAcquisition(arguments) => self
                .manager
                .begin_published_endpoint_acquisition(arguments, principal)
                .await
                .and_then(|result| {
                    Self::success(epoch.clone(), result).map_err(|error| socket_error(&error))
                }),
            protocol::PublisherRequest::Acquire(arguments) => {
                match Self::take_listener(&mut context) {
                    Ok(listener) => self
                        .manager
                        .acquire_published_endpoint(
                            &arguments.acquisition_attempt_handle,
                            &principal,
                            &arguments.acknowledged_origin,
                            listener,
                        )
                        .await
                        .and_then(|result| {
                            Self::success(epoch.clone(), result)
                                .map_err(|error| socket_error(&error))
                        }),
                    Err(error) => Err(error),
                }
            }
            protocol::PublisherRequest::Renew(arguments) => self
                .manager
                .renew_published_endpoint(&arguments.lease_handle, &principal)
                .await
                .and_then(|result| {
                    Self::success(epoch.clone(), result).map_err(|error| socket_error(&error))
                }),
            protocol::PublisherRequest::BeginRebind(arguments) => self
                .manager
                .begin_published_endpoint_rebind(
                    &arguments.lease_handle,
                    &principal,
                    arguments.expected_binding_revision,
                    arguments.replace_terminal_attempt_handle.as_ref(),
                )
                .await
                .and_then(|result| {
                    Self::success(epoch.clone(), result).map_err(|error| socket_error(&error))
                }),
            protocol::PublisherRequest::Rebind(arguments) => {
                match Self::take_listener(&mut context) {
                    Ok(listener) => self
                        .manager
                        .rebind_published_endpoint(
                            &arguments.rebind_attempt_handle,
                            &principal,
                            &arguments.acknowledged_origin,
                            listener,
                        )
                        .await
                        .and_then(|result| {
                            Self::success(epoch.clone(), result)
                                .map_err(|error| socket_error(&error))
                        }),
                    Err(error) => Err(error),
                }
            }
            protocol::PublisherRequest::WaitReady(arguments) => {
                let wait_ready = authority.wait_ready(
                    &arguments.lease_handle,
                    &principal,
                    arguments.expected_binding_revision,
                );
                tokio::pin!(wait_ready);
                let result = tokio::select! {
                    biased;
                    () = context.wait_for_peer_close() => Err(Self::wait_connection_closed()),
                    result = &mut wait_ready => result,
                };
                result.and_then(|result| {
                    Self::success(epoch.clone(), result).map_err(|error| socket_error(&error))
                })
            }
            protocol::PublisherRequest::Release(arguments) => self
                .manager
                .release_published_endpoint(&arguments.lease_handle, &principal)
                .await
                .and_then(|result| {
                    Self::success(epoch.clone(), result).map_err(|error| socket_error(&error))
                }),
        };
        match response {
            Ok(frame) => Ok(frame),
            Err(error) => Self::failure(epoch, error),
        }
    }
}

fn socket_error(error: &PublisherSocketError) -> protocol::ProtocolError {
    protocol::ProtocolError::new(error.stable_code(), error.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publisher_transport::{
        PublisherSocketConfig, PublisherSocketServer, publisher_spawn_barrier,
    };
    use crate::state::StateManager;
    use locald_core::attachments::AttachmentStore;
    use locald_core::ipc::PublicationState as CorePublicationState;
    use locald_core::registry::Registry;
    use locald_publisher_client::{PublisherTransport as _, UnixPublisherTransport};
    use serde::de::DeserializeOwned;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::os::fd::{AsFd as _, BorrowedFd};
    use std::os::unix::net::UnixStream;
    use std::process::Command;
    use std::sync::{Arc, Mutex as StdMutex};
    use tempfile::tempdir;
    use tokio::sync::{Mutex, Notify};

    #[derive(Debug)]
    struct WaitObservingDispatcher {
        dispatcher: PublisherDispatcher,
        wait_started: Arc<Notify>,
        wait_finished: Arc<Notify>,
        wait_error: Arc<StdMutex<Option<protocol::StableErrorCode>>>,
    }

    #[async_trait]
    impl PublisherRequestHandler for WaitObservingDispatcher {
        async fn daemon_epoch(&self) -> protocol::DaemonEpoch {
            self.dispatcher.daemon_epoch().await
        }

        async fn handle(
            &self,
            request: protocol::RequestEnvelope,
            context: PublisherRequestContext,
        ) -> Result<Vec<u8>, PublisherSocketError> {
            let observes_wait =
                matches!(request.request(), protocol::PublisherRequest::WaitReady(_));
            if observes_wait {
                self.wait_started.notify_one();
            }
            let response = self.dispatcher.handle(request, context).await;
            if observes_wait {
                if let Ok(frame) = &response {
                    let response = protocol::decode_response_frame::<serde_json::Value>(frame)
                        .expect("decode observed wait response");
                    let error = response
                        .into_result()
                        .expect_err("routeless wait cannot become ready");
                    *self.wait_error.lock().expect("lock observed wait result") =
                        Some(error.code());
                }
                self.wait_finished.notify_one();
            }
            response
        }
    }

    fn exchange<R: DeserializeOwned>(
        socket: &protocol::AbsolutePath,
        epoch: &protocol::DaemonEpoch,
        request: protocol::PublisherRequest,
        listener: Option<BorrowedFd<'_>>,
    ) -> Result<R, protocol::ProtocolError> {
        let envelope = protocol::RequestEnvelope::v1(epoch.clone(), request);
        let frame = protocol::encode_request_frame(&envelope).expect("encode publisher request");
        let reply = UnixPublisherTransport
            .exchange(socket, &frame, listener)
            .expect("exchange publisher request over the real Unix socket");
        let response = protocol::decode_response_frame::<R>(&reply.response_frame)
            .expect("decode publisher response");
        assert_eq!(response.daemon_epoch(), epoch);
        response.into_result()
    }

    fn begin_unanswered_exchange(
        socket: &protocol::AbsolutePath,
        epoch: &protocol::DaemonEpoch,
        request: protocol::PublisherRequest,
    ) -> UnixStream {
        let envelope = protocol::RequestEnvelope::v1(epoch.clone(), request);
        let frame = protocol::encode_request_frame(&envelope).expect("encode publisher request");
        let mut stream = UnixStream::connect(socket.as_path()).expect("connect publisher socket");
        let mut transport_frame = Vec::with_capacity(
            frame.as_bytes().len()
                + if cfg!(target_os = "macos") {
                    protocol::MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES
                } else {
                    0
                },
        );
        transport_frame.push(frame.as_bytes()[0]);
        #[cfg(target_os = "macos")]
        transport_frame.extend_from_slice(&current_macos_audit_proof());
        transport_frame.extend_from_slice(&frame.as_bytes()[1..]);
        stream
            .write_all(&transport_frame)
            .expect("write publisher request");
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("finish publisher request");
        stream
    }

    #[cfg(target_os = "macos")]
    fn current_macos_audit_proof() -> [u8; protocol::MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES] {
        const TASK_AUDIT_TOKEN: libc::task_flavor_t = 15;
        const TASK_AUDIT_TOKEN_WORDS: usize =
            protocol::MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES / std::mem::size_of::<u32>();

        let mut words = [0_u32; TASK_AUDIT_TOKEN_WORDS];
        let mut word_count = libc::mach_msg_type_number_t::try_from(words.len())
            .expect("audit-token word count fits the Mach ABI");
        // SAFETY: `words` is writable storage for exactly `word_count` Mach
        // natural words, and the current task port remains valid for the call.
        #[allow(unsafe_code, deprecated)]
        let result = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                TASK_AUDIT_TOKEN,
                words.as_mut_ptr().cast(),
                &raw mut word_count,
            )
        };
        assert_eq!(result, libc::KERN_SUCCESS, "obtain current audit token");
        assert_eq!(
            usize::try_from(word_count).ok(),
            Some(words.len()),
            "current audit token has the expected length"
        );

        let mut proof = [0_u8; protocol::MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES];
        for (bytes, word) in proof
            .chunks_exact_mut(std::mem::size_of::<u32>())
            .zip(words)
        {
            bytes.copy_from_slice(&word.to_ne_bytes());
        }
        proof
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_socket_drives_the_complete_routeless_publication_lifecycle() {
        let directory = tempdir().expect("create publisher integration fixture");
        let project_path = directory.path().join("project");
        std::fs::create_dir(&project_path).expect("create published project");
        std::fs::write(
            project_path.join("locald.toml"),
            r#"
[project]
name = "publisher-integration"
domain = "publisher-integration.localhost"

[services.workbench]
type = "published"
domains = ["workbench"]
health_check = { type = "http", path = "/api/health" }
"#,
        )
        .expect("write published configuration");
        let mut git = Command::new("git");
        git.args(["init", "--quiet"])
            .arg(&project_path)
            .current_dir(&project_path);
        // Git exports repository-local environment to hooks. The full test
        // suite runs from the pre-push hook, so this fixture must not inherit
        // the enclosing locald checkout as its repository.
        for variable in [
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_OBJECT_DIRECTORY",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_IMPLICIT_WORK_TREE",
            "GIT_GRAFT_FILE",
            "GIT_INDEX_FILE",
            "GIT_NO_REPLACE_OBJECTS",
            "GIT_REPLACE_REF_BASE",
            "GIT_PREFIX",
            "GIT_SHALLOW_FILE",
            "GIT_COMMON_DIR",
        ] {
            git.env_remove(variable);
        }
        let git_status = locald_utils::process_spawn::ProcessSpawnBarrier::global()
            .spawn_std_command(&mut git)
            .and_then(|mut child| child.wait())
            .expect("initialize Git worktree");
        assert!(git_status.success(), "Git worktree initialization succeeds");

        let registry = Arc::new(Mutex::new(Registry::with_path(
            directory.path().join("catalog.json"),
        )));
        let mut manager = ProcessManager::new(
            directory.path().join("notify.sock"),
            Arc::new(StateManager::with_path(directory.path().join("state.json"))),
            registry.clone(),
            Arc::new(Mutex::new(AttachmentStore::new(
                directory.path().join("attachments.json"),
            ))),
            None,
        )
        .expect("create publication manager");
        manager.use_sandbox_host_set_writer();
        manager.set_https_port(Some(4443)).await;
        let instance_id = manager
            .resolve_published_endpoint_project_instance(&project_path)
            .await
            .expect("resolve physical worktree identity before registration");
        assert!(
            registry.lock().await.instances.is_empty(),
            "identity resolution alone does not register the project"
        );
        let wire_instance = protocol::ProjectInstanceId::parse(&instance_id.to_string())
            .expect("convert project instance to wire identity");
        let wire_locator = protocol::AbsolutePath::try_from(project_path)
            .expect("convert project path to wire locator");
        let service_name = protocol::ServiceName::parse("workbench").expect("parse service name");

        let run_parent = directory.path().join("data");
        std::fs::create_dir(&run_parent).expect("create publisher data directory");
        let socket_path = run_parent.join("run/publisher-v1.sock");
        let socket = protocol::AbsolutePath::try_from(socket_path.clone())
            .expect("convert publisher socket path");
        let config =
            PublisherSocketConfig::for_current_user(socket_path, [4443], publisher_spawn_barrier());
        let wait_started = Arc::new(Notify::new());
        let wait_finished = Arc::new(Notify::new());
        let wait_error = Arc::new(StdMutex::new(None));
        let dispatcher = Arc::new(WaitObservingDispatcher {
            dispatcher: PublisherDispatcher::new(manager.clone()),
            wait_started: Arc::clone(&wait_started),
            wait_finished: Arc::clone(&wait_finished),
            wait_error: Arc::clone(&wait_error),
        });
        let server = PublisherSocketServer::bind(config, dispatcher)
            .await
            .expect("bind publisher socket");
        let authority = manager.publisher_authority();
        let protocol_info = authority.protocol_info(socket.clone()).await;
        let epoch = protocol_info.daemon_epoch().clone();

        let begin = exchange::<protocol::BeginAcquisitionResult>(
            &socket,
            &epoch,
            protocol::PublisherRequest::BeginAcquisition(protocol::BeginAcquisitionArguments {
                expected_project_instance_id: wire_instance,
                project_locator: wire_locator,
                service_name,
                replace_terminal_attempt_handle: None,
            }),
            None,
        )
        .expect("prepare acquisition");
        assert!(
            registry.lock().await.instances.contains_key(&instance_id),
            "first publication preparation admits the declaration without locald up"
        );

        let first_listener = TcpListener::bind("127.0.0.1:0").expect("bind first listener");
        let acquired = exchange::<protocol::AcquireResult>(
            &socket,
            &epoch,
            protocol::PublisherRequest::Acquire(protocol::AcquireArguments {
                acquisition_attempt_handle: begin.acquisition_attempt_handle().clone(),
                acknowledged_origin: begin.origin().clone(),
            }),
            Some(first_listener.as_fd()),
        )
        .expect("acquire first published binding");
        assert_eq!(acquired.binding_revision().get(), 1);
        assert_eq!(
            acquired.publication_state(),
            protocol::PublicationState::CheckingEndpoint
        );

        let status = manager
            .list()
            .await
            .into_iter()
            .find(|status| status.name == "publisher-integration:workbench")
            .expect("published service remains visible");
        assert_eq!(
            status.publication.map(|publication| publication.state),
            Some(CorePublicationState::CheckingEndpoint)
        );
        let resolution = manager
            .resolve_service_by_domain("workbench.publisher-integration.localhost")
            .await
            .expect("resolve stable published origin");
        let locald_core::resolver::DomainResolution::PublishedUnavailable { publication, .. } =
            resolution
        else {
            panic!("L1 must keep canonical HTTPS unroutable");
        };
        assert_eq!(publication.state, CorePublicationState::CheckingEndpoint);

        let wait_connection = begin_unanswered_exchange(
            &socket,
            &epoch,
            protocol::PublisherRequest::WaitReady(protocol::WaitReadyArguments {
                lease_handle: acquired.lease_handle().clone(),
                expected_binding_revision: acquired.binding_revision(),
            }),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), wait_started.notified())
            .await
            .expect("readiness wait reaches the dispatcher");
        drop(wait_connection);
        tokio::time::timeout(std::time::Duration::from_secs(1), wait_finished.notified())
            .await
            .expect("closing only the wait connection cancels that waiter");
        assert_eq!(
            *wait_error.lock().expect("lock observed wait result"),
            Some(protocol::StableErrorCode::OperationCanceled)
        );

        let renewed = exchange::<protocol::RenewResult>(
            &socket,
            &epoch,
            protocol::PublisherRequest::Renew(protocol::RenewArguments {
                lease_handle: acquired.lease_handle().clone(),
            }),
            None,
        )
        .expect("renew live lease after its canceled waiter");
        assert_eq!(renewed.binding_revision().get(), 1);

        let begin_rebind = exchange::<protocol::BeginRebindResult>(
            &socket,
            &epoch,
            protocol::PublisherRequest::BeginRebind(protocol::BeginRebindArguments {
                lease_handle: acquired.lease_handle().clone(),
                expected_binding_revision: renewed.binding_revision(),
                replace_terminal_attempt_handle: None,
            }),
            None,
        )
        .expect("prepare rebind");
        let second_listener = TcpListener::bind("127.0.0.1:0").expect("bind second listener");
        let rebound = exchange::<protocol::RebindResult>(
            &socket,
            &epoch,
            protocol::PublisherRequest::Rebind(protocol::RebindArguments {
                rebind_attempt_handle: begin_rebind.rebind_attempt_handle().clone(),
                acknowledged_origin: begin_rebind.origin().clone(),
            }),
            Some(second_listener.as_fd()),
        )
        .expect("install replacement binding");
        assert_eq!(rebound.binding_revision().get(), 2);

        let released = exchange::<protocol::ReleaseResult>(
            &socket,
            &epoch,
            protocol::PublisherRequest::Release(protocol::ReleaseArguments {
                lease_handle: rebound.lease_handle().clone(),
            }),
            None,
        )
        .expect("release live lease");
        assert!(released.is_released());

        let wait_error = exchange::<protocol::WaitReadyResult>(
            &socket,
            &epoch,
            protocol::PublisherRequest::WaitReady(protocol::WaitReadyArguments {
                lease_handle: rebound.lease_handle().clone(),
                expected_binding_revision: rebound.binding_revision(),
            }),
            None,
        )
        .expect_err("released lease cannot become ready");
        assert_eq!(wait_error.code(), protocol::StableErrorCode::LeaseLost);

        server.shutdown().await.expect("stop publisher socket");
        authority.shutdown().await;
        manager.shutdown().await.expect("stop publication manager");
    }
}
