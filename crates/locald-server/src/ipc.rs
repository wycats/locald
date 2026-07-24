use crate::ShutdownReason;
use crate::container::ContainerManager;
use crate::manager::{InstanceLogEntry, ProcessManager};
use anyhow::{Context, Result};
use locald_core::attachments::{AttachmentSource, ManualCliSession};
use locald_core::config::LocaldConfig;
use locald_core::ipc::{DaemonIdentity, LogEntry, MAX_IPC_REQUEST_BYTES};
use locald_core::{DemandKey, IpcRequest, IpcResponse, ProjectInstanceId};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc::Sender};
use tracing::{error, info};

pub async fn run_ipc_server(
    manager: ProcessManager,
    container_manager: Arc<ContainerManager>,
    shutdown_tx: Sender<ShutdownReason>,
    version: String,
) -> Result<()> {
    let socket_path = locald_utils::ipc::socket_path()?;

    if tokio::fs::metadata(&socket_path).await.is_ok() {
        // Try to connect to see if it's alive
        if UnixStream::connect(&socket_path).await.is_ok() {
            anyhow::bail!(
                "Socket {} is already in use. Is locald-server already running?",
                socket_path.display()
            );
        }
        // If we can't connect, it's likely a stale socket
        tokio::fs::remove_file(&socket_path).await?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    info!("IPC server listening on {:?}", socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let manager = manager.clone();
                let container_manager = container_manager.clone();
                let shutdown_tx = shutdown_tx.clone();
                let version = version.clone();
                tokio::spawn(handle_connection_task(
                    stream,
                    manager,
                    container_manager,
                    shutdown_tx,
                    version,
                ));
            }
            Err(e) => {
                error!("Error accepting connection: {}", e);
            }
        }
    }
}

// Keep the spawned connection boundary named so the accept loop and its
// per-connection error reporting remain explicit.
#[allow(clippy::large_futures)]
async fn handle_connection_task(
    stream: UnixStream,
    manager: ProcessManager,
    container_manager: Arc<ContainerManager>,
    shutdown_tx: Sender<ShutdownReason>,
    version: String,
) {
    if let Err(error) =
        handle_connection(stream, manager, container_manager, shutdown_tx, version).await
    {
        error!("Error handling connection: {}", error);
    }
}

fn authenticated_peer_pid(stream: &UnixStream) -> Result<u32> {
    let credentials = stream
        .peer_cred()
        .context("failed to read kernel-authenticated IPC peer credentials")?;
    let pid = credentials
        .pid()
        .context("kernel-authenticated IPC peer credentials did not include a process ID")?;
    let pid = u32::try_from(pid).context("kernel-authenticated IPC peer process ID was invalid")?;
    anyhow::ensure!(pid > 0, "kernel-authenticated IPC peer process ID was zero");
    Ok(pid)
}

fn validate_manual_cli_session(stream: &UnixStream, session: ManualCliSession) -> Result<()> {
    let peer_pid = authenticated_peer_pid(stream)?;
    anyhow::ensure!(
        session.pid() == peer_pid,
        "Manual CLI session PID {} does not match kernel-authenticated IPC peer PID {peer_pid}",
        session.pid()
    );
    Ok(())
}

fn project_logs_resolution_error(path: &Path, error: &anyhow::Error) -> String {
    format!(
        "project-scoped logs are unavailable for `{}`: {error:#}; run `locald up` from that project first, or run `locald logs` outside a locald project to inspect all daemon logs",
        path.display()
    )
}

enum LogSubscription {
    Global(broadcast::Receiver<LogEntry>),
    Project {
        instance_id: ProjectInstanceId,
        receiver: broadcast::Receiver<InstanceLogEntry>,
    },
}

impl LogSubscription {
    async fn recv(&mut self) -> Result<LogEntry, broadcast::error::RecvError> {
        match self {
            Self::Global(receiver) => receiver.recv().await,
            Self::Project {
                instance_id,
                receiver,
            } => loop {
                match receiver.recv().await {
                    Ok(scoped_entry) if scoped_entry.instance_id == *instance_id => {
                        return Ok(scoped_entry.entry);
                    }
                    Ok(_) => {}
                    Err(error) => return Err(error),
                }
            },
        }
    }
}

fn subscribe_to_logs(
    project_instance_id: Option<ProjectInstanceId>,
    global_sender: &broadcast::Sender<LogEntry>,
    instance_sender: &broadcast::Sender<InstanceLogEntry>,
) -> LogSubscription {
    project_instance_id.map_or_else(
        || LogSubscription::Global(global_sender.subscribe()),
        |instance_id| LogSubscription::Project {
            instance_id,
            receiver: instance_sender.subscribe(),
        },
    )
}

fn authenticate_process_bound_attachment_source(
    stream: &UnixStream,
    source: AttachmentSource,
) -> Result<AttachmentSource> {
    match source {
        AttachmentSource::CLI { .. } => Ok(AttachmentSource::CLI {
            pid: authenticated_peer_pid(stream)?,
        }),
        AttachmentSource::ManualCLI(session) => {
            validate_manual_cli_session(stream, session)?;
            Ok(AttachmentSource::ManualCLI(session))
        }
        source @ (AttachmentSource::Editor { .. }
        | AttachmentSource::Runtime
        | AttachmentSource::Pin) => Ok(source),
    }
}

fn validate_generic_ensure_demand(demand: &DemandKey) -> Result<()> {
    demand
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid EnsureProject demand: {error}"))?;
    anyhow::ensure!(
        !demand.has_owner(),
        "generic EnsureProject IPC accepts only ownerless demands; owner-bearing demands must be derived by an authenticated host adapter"
    );
    Ok(())
}

fn authenticate_ensure_launch_path(
    stream: &UnixStream,
    demand: &DemandKey,
    launch_path: Option<String>,
) -> Result<Option<String>> {
    launch_path
        .map(|path| {
            authenticated_peer_pid(stream)?;
            anyhow::ensure!(
                demand.kind() == locald_core::DemandKind::ManualCli,
                "trusted launch PATH is accepted only for an explicit Manual CLI ensure"
            );
            Ok(path)
        })
        .transpose()
}

async fn read_request(stream: &mut UnixStream) -> Result<Option<IpcRequest>> {
    let mut request_bytes = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];

    loop {
        let remaining = MAX_IPC_REQUEST_BYTES + 1 - request_bytes.len();
        let read_limit = remaining.min(chunk.len());
        let bytes_read = stream.read(&mut chunk[..read_limit]).await?;
        if bytes_read == 0 {
            if request_bytes.is_empty() {
                return Ok(None);
            }
            return serde_json::from_slice(&request_bytes)
                .context("daemon IPC request ended before a complete JSON value")
                .map(Some);
        }

        request_bytes.extend_from_slice(&chunk[..bytes_read]);
        anyhow::ensure!(
            request_bytes.len() <= MAX_IPC_REQUEST_BYTES,
            "daemon IPC request exceeds the {MAX_IPC_REQUEST_BYTES}-byte limit"
        );

        match serde_json::from_slice(&request_bytes) {
            Ok(request) => return Ok(Some(request)),
            Err(error) if error.is_eof() => {}
            Err(error) => return Err(error).context("invalid daemon IPC request"),
        }
    }
}

// The compatibility dispatcher still owns the full legacy IPC enum while it
// routes each request. Keep that known boundary explicit until the legacy
// protocol is retired rather than spreading the match across public helpers.
#[allow(clippy::large_futures, clippy::large_stack_frames)]
async fn handle_connection(
    mut stream: UnixStream,
    manager: ProcessManager,
    container_manager: Arc<ContainerManager>,
    shutdown_tx: Sender<ShutdownReason>,
    version: String,
) -> Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };
    tracing::debug!("Received request: {:?}", request);

    if let IpcRequest::RunContainer {
        image,
        command,
        interactive,
        detached,
    } = request
    {
        info!("Handling RunContainer: image={}", image);

        let (tx, mut rx) = tokio::sync::mpsc::channel(100);

        // Spawn the container run in a separate task so we can stream logs
        let container_manager = container_manager.clone();
        let handle = tokio::spawn(async move {
            container_manager
                .run(&image, command, interactive, detached, Some(tx))
                .await
        });

        // Stream logs back to client
        while let Some((line, is_stderr)) = rx.recv().await {
            let stream_type = if is_stderr {
                locald_core::ipc::LogStream::Stderr
            } else {
                locald_core::ipc::LogStream::Stdout
            };
            let event = locald_core::ipc::Event::Log(locald_core::ipc::LogEntry {
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
                service: "container".to_string(),
                stream: stream_type,
                message: line,
            });
            let mut bytes = serde_json::to_vec(&event)?;
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
        }

        match handle.await? {
            Ok(()) => {
                info!("RunContainer succeeded");
                let response = IpcResponse::Ok;
                let bytes = serde_json::to_vec(&response)?;
                stream.write_all(&bytes).await?;
            }
            Err(e) => {
                error!("RunContainer failed: {:?}", e);
                let response = IpcResponse::Error(format!("{e:#}"));
                let bytes = serde_json::to_vec(&response)?;
                stream.write_all(&bytes).await?;
            }
        }
        return Ok(());
    }

    if let IpcRequest::Logs {
        service,
        project_path,
        mode,
    } = request
    {
        let project_instance_id = match project_path {
            Some(project_path) => match manager.project_instance_for_logs(&project_path).await {
                Ok(instance_id) => Some(instance_id),
                Err(error) => {
                    let message = project_logs_resolution_error(&project_path, &error);
                    let mut bytes = serde_json::to_vec(&IpcResponse::Error(message))?;
                    bytes.push(b'\n');
                    stream.write_all(&bytes).await?;
                    return Ok(());
                }
            },
            None => None,
        };
        let mut subscription = subscribe_to_logs(
            project_instance_id,
            &manager.log_sender,
            &manager.instance_log_sender,
        );
        let recent = manager.get_recent_logs_for_instance(project_instance_id);

        for entry in recent {
            if let Some(ref s) = service
                && &entry.service != s
                && entry.service != format!("{}:build", s)
            {
                continue;
            }
            let mut bytes = serde_json::to_vec(&entry)?;
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
        }

        if matches!(mode, locald_core::ipc::LogMode::Snapshot) {
            return Ok(());
        }

        loop {
            match subscription.recv().await {
                Ok(entry) => {
                    if let Some(ref s) = service
                        && &entry.service != s
                        && entry.service != format!("{}:build", s)
                    {
                        continue;
                    }
                    let mut bytes = serde_json::to_vec(&entry)?;
                    bytes.push(b'\n');
                    if stream.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        return Ok(());
    }

    if let IpcRequest::EnsureProject {
        project_path,
        demand,
        verbose: true,
        launch_path,
    } = &request
    {
        let authenticated_launch_path = validate_generic_ensure_demand(demand)
            .and_then(|()| authenticate_ensure_launch_path(&stream, demand, launch_path.clone()));
        let launch_path = match authenticated_launch_path {
            Ok(launch_path) => launch_path,
            Err(error) => {
                let mut bytes = serde_json::to_vec(&IpcResponse::Error(format!("{error:#}")))?;
                bytes.push(b'\n');
                stream.write_all(&bytes).await?;
                return Ok(());
            }
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let manager = manager.clone();
        let project_path = project_path.clone();
        let demand = demand.clone();
        let handle = tokio::spawn(async move {
            manager
                .ensure_project_from_ipc_with_events(project_path, demand, launch_path, tx)
                .await
        });

        while let Some(event) = rx.recv().await {
            let mut bytes = serde_json::to_vec(&event)?;
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
        }

        let response = match handle.await? {
            Ok(result) => IpcResponse::ProjectEnsured(result),
            Err(error) => IpcResponse::Error(format!("{error:#}")),
        };
        let mut bytes = serde_json::to_vec(&response)?;
        bytes.push(b'\n');
        stream.write_all(&bytes).await?;
        return Ok(());
    }

    if let IpcRequest::Start {
        project_path,
        verbose,
        launch_path,
        manual_cli_session,
    } = request
    {
        let legacy_cli_peer_pid = if let Some(session) = manual_cli_session {
            if let Err(error) = validate_manual_cli_session(&stream, session) {
                let mut bytes = serde_json::to_vec(&IpcResponse::Error(format!("{error:#}")))?;
                bytes.push(b'\n');
                stream.write_all(&bytes).await?;
                return Ok(());
            }
            None
        } else {
            match authenticated_peer_pid(&stream) {
                Ok(peer_pid) => Some(peer_pid),
                Err(error) => {
                    tracing::debug!(
                        "Legacy Start request has no authenticated peer PID for attachment pairing: {error:#}"
                    );
                    None
                }
            }
        };
        if launch_path.is_some() && manual_cli_session.is_none() && legacy_cli_peer_pid.is_none() {
            let mut bytes = serde_json::to_vec(&IpcResponse::Error(
                "trusted launch PATH requires kernel-authenticated local IPC peer credentials"
                    .to_owned(),
            ))?;
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
            return Ok(());
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let manager = manager.clone();

        let handle = tokio::spawn(async move {
            manager
                .start_from_ipc(
                    project_path,
                    Some(tx),
                    verbose,
                    manual_cli_session,
                    legacy_cli_peer_pid,
                    launch_path,
                )
                .await
        });

        while let Some(event) = rx.recv().await {
            let mut bytes = serde_json::to_vec(&event)?;
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
        }

        let result = handle.await?;
        let response = match result {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(format!("{e:#}")),
        };

        let mut bytes = serde_json::to_vec(&response)?;
        bytes.push(b'\n');
        stream.write_all(&bytes).await?;

        return Ok(());
    }

    let response = match request {
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::GetVersion => IpcResponse::Version(version),
        IpcRequest::GetDaemonIdentity => match std::env::current_exe() {
            Ok(executable) => IpcResponse::DaemonIdentity(DaemonIdentity {
                version,
                pid: std::process::id(),
                executable,
            }),
            Err(e) => IpcResponse::Error(format!("failed to resolve daemon executable: {e}")),
        },
        IpcRequest::Start { .. } => unreachable!(),
        IpcRequest::Stop { name } => match manager.stop(&name).await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::Restart { name } => match manager.restart(&name).await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(format!("{e:#}")),
        },
        IpcRequest::Reset { name } => match manager.reset(&name).await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::StopAll => match manager.stop_all().await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::RestartAll => match manager.restart_all().await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::Status => {
            let status = manager.list().await;
            IpcResponse::Status(status)
        }
        IpcRequest::SyncHosts => match manager.sync_hosts().await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::GetHostsDomains => IpcResponse::HostsDomains(manager.hosts_domain_names()),
        IpcRequest::Shutdown => {
            let _ = shutdown_tx.send(ShutdownReason::Stop).await;
            IpcResponse::Ok
        }
        IpcRequest::AiSchema => {
            let schema = schemars::schema_for!(LocaldConfig);
            let schema_json = serde_json::to_string_pretty(&schema)?;
            IpcResponse::AiSchema(schema_json)
        }
        IpcRequest::AiContext => {
            let status = manager.list().await;
            let context = serde_json::to_string_pretty(&status)?;
            IpcResponse::AiContext(context)
        }
        IpcRequest::RegistryList => match manager.registry_list().await {
            Ok(projects) => IpcResponse::RegistryList(projects),
            Err(error) => IpcResponse::Error(error.to_string()),
        },
        IpcRequest::RegistryPin { project_path } => match manager.registry_pin(&project_path).await
        {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::RegistryUnpin { project_path } => {
            match manager.registry_unpin(&project_path).await {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e.to_string()),
            }
        }
        IpcRequest::RegistryClean => match manager.registry_clean().await {
            Ok(count) => IpcResponse::RegistryCleaned(count),
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::ProjectAttach {
            project_path,
            source,
            standalone,
        } => {
            if matches!(source, AttachmentSource::ManualCLI(_)) {
                IpcResponse::Error(
                    "ManualCLI owners are created only by their paired Start request".to_owned(),
                )
            } else {
                match authenticate_process_bound_attachment_source(&stream, source) {
                    Ok(source) => match manager
                        .project_attach_from_ipc(project_path, source, standalone)
                        .await
                    {
                        Ok(()) => IpcResponse::Ok,
                        Err(e) => IpcResponse::Error(e.to_string()),
                    },
                    Err(error) => IpcResponse::Error(format!("{error:#}")),
                }
            }
        }
        IpcRequest::ProjectDetach {
            project_path,
            source,
        } => match source
            .map(|source| authenticate_process_bound_attachment_source(&stream, source))
            .transpose()
        {
            Ok(source) => match manager.project_detach(project_path, source).await {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e.to_string()),
            },
            Err(error) => IpcResponse::Error(format!("{error:#}")),
        },
        IpcRequest::ProjectStatus { project_path } => {
            match manager.project_status(&project_path).await {
                Ok(info) => IpcResponse::ProjectStatus(info),
                Err(e) => IpcResponse::Error(e.to_string()),
            }
        }
        IpcRequest::ProjectList { filter } => match manager.project_list(filter).await {
            Ok(entries) => IpcResponse::ProjectList(entries),
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::EnsureProject {
            project_path,
            demand,
            verbose: _,
            launch_path,
        } => match validate_generic_ensure_demand(&demand) {
            Ok(()) => {
                let authenticated_launch_path =
                    authenticate_ensure_launch_path(&stream, &demand, launch_path);
                match authenticated_launch_path {
                    Ok(launch_path) => match manager
                        .ensure_project_from_ipc(project_path, demand, launch_path)
                        .await
                    {
                        Ok(result) => IpcResponse::ProjectEnsured(result),
                        Err(error) => IpcResponse::Error(format!("{error:#}")),
                    },
                    Err(error) => IpcResponse::Error(format!("{error:#}")),
                }
            }
            Err(error) => IpcResponse::Error(format!("{error:#}")),
        },
        IpcRequest::RenewProjectDemand {
            project_path,
            demand,
        } => match validate_generic_ensure_demand(&demand) {
            Ok(()) => match manager
                .project_renew_availability(&project_path, &demand)
                .await
            {
                Ok(true) => IpcResponse::Ok,
                Ok(false) => IpcResponse::Error(
                    "the project demand is no longer live; run `locald up` again".to_owned(),
                ),
                Err(error) => IpcResponse::Error(format!("{error:#}")),
            },
            Err(error) => IpcResponse::Error(format!("{error:#}")),
        },
        IpcRequest::PauseProject { project_path } => {
            match manager.project_pause_availability(&project_path).await {
                Ok(_) => IpcResponse::Ok,
                Err(error) => IpcResponse::Error(format!("{error:#}")),
            }
        }
        IpcRequest::SetAlwaysOn {
            project_path,
            enabled,
        } => match manager.project_set_always_on(&project_path, enabled).await {
            Ok(()) => IpcResponse::Ok,
            Err(error) => IpcResponse::Error(format!("{error:#}")),
        },
        IpcRequest::ProjectForceStart { project_path } => {
            match manager.project_force_start(project_path).await {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e.to_string()),
            }
        }
        IpcRequest::ProjectForceStop { project_path } => {
            match manager.project_force_stop(project_path).await {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e.to_string()),
            }
        }
        IpcRequest::GetServiceEnv { name } => match manager.get_service_env(&name).await {
            Ok(env) => IpcResponse::ServiceEnv(env),
            Err(e) => IpcResponse::Error(e.to_string()),
        },
        IpcRequest::Logs { .. } => unreachable!(),
        IpcRequest::RunContainer { .. } => unreachable!(),
    };

    let response_bytes = serde_json::to_vec(&response)?;
    stream.write_all(&response_bytes).await?;

    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn different_pid(pid: u32) -> u32 {
        pid.wrapping_add(1)
    }

    #[tokio::test]
    async fn unix_stream_peer_pid_is_kernel_authenticated() {
        let (_client, server) = UnixStream::pair().expect("create connected IPC pair");

        assert_eq!(
            authenticated_peer_pid(&server).expect("authenticate IPC peer PID"),
            std::process::id()
        );
    }

    #[tokio::test]
    async fn manual_cli_session_rejects_a_spoofed_pid() {
        let (_client, server) = UnixStream::pair().expect("create connected IPC pair");
        let peer_pid = authenticated_peer_pid(&server).expect("authenticate IPC peer PID");
        let session = ManualCliSession::new(different_pid(peer_pid));

        let error = validate_manual_cli_session(&server, session)
            .expect_err("spoofed Manual CLI PID must be rejected");

        assert!(
            error
                .to_string()
                .contains("does not match kernel-authenticated IPC peer PID")
        );
    }

    #[tokio::test]
    async fn cli_attachment_uses_the_authenticated_peer_pid() {
        let (_client, server) = UnixStream::pair().expect("create connected IPC pair");
        let peer_pid = authenticated_peer_pid(&server).expect("authenticate IPC peer PID");

        let source = authenticate_process_bound_attachment_source(
            &server,
            AttachmentSource::CLI {
                pid: different_pid(peer_pid),
            },
        )
        .expect("authenticate CLI attachment source");

        assert_eq!(source, AttachmentSource::CLI { pid: peer_pid });
    }

    #[test]
    fn generic_ensure_rejects_caller_supplied_owner_identity() {
        let demand = DemandKey::manual_cli_session("caller-selected-session")
            .expect("construct owner-bearing Manual CLI demand");

        let error = validate_generic_ensure_demand(&demand)
            .expect_err("generic IPC must reject caller-supplied demand owners");

        assert!(error.to_string().contains("accepts only ownerless demands"));
        validate_generic_ensure_demand(&DemandKey::manual_cli())
            .expect("generic IPC accepts ownerless Manual CLI demand");
    }

    #[test]
    fn scoped_log_resolution_failure_explains_both_recovery_paths() {
        let error = anyhow::anyhow!("project is not registered in the identity catalog");
        let message = project_logs_resolution_error(Path::new("/tmp/example"), &error);

        assert!(message.contains("project-scoped logs are unavailable for `/tmp/example`"));
        assert!(message.contains("run `locald up` from that project first"));
        assert!(message.contains("run `locald logs` outside a locald project"));
        assert!(message.contains("not registered in the identity catalog"));
    }

    #[tokio::test]
    async fn unscoped_logs_subscribe_to_the_global_daemon_stream() {
        let (global_sender, _) = broadcast::channel(4);
        let (instance_sender, _) = broadcast::channel(4);
        let mut subscription = subscribe_to_logs(None, &global_sender, &instance_sender);
        let entry = LogEntry {
            timestamp: 0,
            service: "locald".to_owned(),
            stream: locald_core::ipc::LogStream::Stdout,
            message: "daemon diagnostic".to_owned(),
        };

        global_sender
            .send(entry.clone())
            .expect("publish daemon log");

        assert_eq!(
            subscription.recv().await.expect("receive daemon log"),
            entry
        );
    }

    #[tokio::test]
    async fn trusted_launch_path_requires_an_explicit_authenticated_cli_ensure() {
        let (_client, server) = UnixStream::pair().expect("create connected IPC pair");
        let path = "/opt/homebrew/bin:/usr/bin".to_owned();

        assert_eq!(
            authenticate_ensure_launch_path(&server, &DemandKey::manual_cli(), Some(path.clone()),)
                .expect("authenticate explicit CLI launch context"),
            Some(path)
        );

        let error = authenticate_ensure_launch_path(
            &server,
            &DemandKey::stopped_page_resume(),
            Some("/usr/bin".to_owned()),
        )
        .expect_err("non-CLI ensure must not replace trusted launch context");
        assert!(
            error
                .to_string()
                .contains("accepted only for an explicit Manual CLI ensure")
        );
    }

    #[tokio::test]
    async fn ipc_reader_accepts_a_request_larger_than_one_read_buffer() {
        let (mut client, mut server) = UnixStream::pair().expect("create connected IPC pair");
        let request = IpcRequest::Start {
            project_path: "/tmp/project".into(),
            verbose: false,
            launch_path: Some("x".repeat(8192)),
            manual_cli_session: None,
        };
        let request_bytes = serde_json::to_vec(&request).expect("encode large request");
        assert!(request_bytes.len() > 4096);

        let writer = tokio::spawn(async move {
            client
                .write_all(&request_bytes)
                .await
                .expect("write complete large request");
        });
        let decoded = read_request(&mut server)
            .await
            .expect("read large request")
            .expect("request is present");
        writer.await.expect("writer joins");

        assert_eq!(decoded, request);
    }
}
