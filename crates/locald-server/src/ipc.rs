use crate::ShutdownReason;
use crate::container::ContainerManager;
use crate::manager::ProcessManager;
use anyhow::Result;
use locald_core::attachments::AttachmentSource;
use locald_core::config::LocaldConfig;
use locald_core::ipc::DaemonIdentity;
use locald_core::{IpcRequest, IpcResponse};
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

// Tokio already owns this spawned future in its task allocation. Keeping the
// large connection future inline avoids a second per-connection allocation.
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
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let manager = manager.clone();

        let handle = tokio::spawn(async move {
            manager
                .start_with_manual_cli_session(project_path, Some(tx), verbose, manual_cli_session)
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
        } => {
            if matches!(source, AttachmentSource::ManualCLI(_)) {
                IpcResponse::Error(
                    "ManualCLI owners are created only by their paired Start request".to_owned(),
                )
            } else {
                match manager.project_attach(project_path, source).await {
                    Ok(()) => IpcResponse::Ok,
                    Err(e) => IpcResponse::Error(e.to_string()),
                }
            }
        }
        IpcRequest::ProjectDetach {
            project_path,
            source,
        } => match manager.project_detach(project_path, source).await {
            Ok(()) => IpcResponse::Ok,
            Err(e) => IpcResponse::Error(e.to_string()),
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
