use crate::ShutdownReason;
use crate::container::ContainerManager;
use crate::manager::ProcessManager;
use anyhow::{Context, Result};
use locald_core::attachments::{AttachmentSource, ManualCliSession};
use locald_core::config::LocaldConfig;
use locald_core::ipc::DaemonIdentity;
use locald_core::{DemandKey, IpcRequest, IpcResponse};
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

async fn handle_connection(
    mut stream: UnixStream,
    manager: ProcessManager,
    container_manager: Arc<ContainerManager>,
    shutdown_tx: Sender<ShutdownReason>,
    version: String,
) -> Result<()> {
    let mut buf = [0; 4096];
    let n = stream.read(&mut buf).await?;

    if n == 0 {
        return Ok(());
    }

    let request: IpcRequest = serde_json::from_slice(&buf[..n])?;
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

    if let IpcRequest::Logs { service, mode } = request {
        let mut rx = manager.log_sender.subscribe();
        let recent = manager.get_recent_logs();

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
            match rx.recv().await {
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

    if let IpcRequest::Start {
        project_path,
        verbose,
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
        } => match validate_generic_ensure_demand(&demand) {
            Ok(()) => match manager.ensure_project(project_path, demand).await {
                Ok(result) => IpcResponse::ProjectEnsured(result),
                Err(error) => IpcResponse::Error(format!("{error:#}")),
            },
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
}
