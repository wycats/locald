use crate::runtime::process::ProcessRuntime;
use anyhow::{Context, Result};
use async_stream::stream;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use locald_core::config::{ServiceConfig, TypedServiceConfig};
use locald_core::ipc::{LogEntry, LogStream, ServiceMetrics};
use locald_core::service::{
    RuntimeState, ServiceCommand, ServiceContext, ServiceController, ServiceFactory,
};
use locald_core::state::{HealthStatus, PersistedProcessIdentity, ServiceState};
use nix::sys::signal::Signal;
use portable_pty::{Child, MasterPty, PtySize};
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::warn;

type SpawnedProcess = (
    Box<dyn Child + Send>,
    Box<dyn MasterPty + Send>,
    Box<dyn std::io::Write + Send>,
    String,
    mpsc::Receiver<LogEntry>,
    broadcast::Sender<Vec<u8>>,
);

pub struct ExecController {
    id: String,
    resource_id: String,
    runtime: ProcessRuntime,
    config: ServiceConfig,
    project_root: PathBuf,
    // Runtime state
    child: Option<StdMutex<Box<dyn Child + Send>>>,
    pty_master: Option<StdMutex<Box<dyn MasterPty + Send>>>,
    pty_writer: Option<StdMutex<Box<dyn std::io::Write + Send>>>,
    container_id: Option<String>,
    owned_process_id: Option<u32>,
    process_identity: Option<PersistedProcessIdentity>,
    cgroup_path: Option<String>,
    port: Option<u16>,
    log_tx: broadcast::Sender<LogEntry>,
    pty_tx: Option<broadcast::Sender<Vec<u8>>>,
    bundle_dir: Option<PathBuf>,
    env: std::collections::HashMap<String, String>,
    system: StdMutex<System>,
}

impl fmt::Debug for ExecController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecController")
            .field("id", &self.id)
            .field("resource_id", &self.resource_id)
            .field("config", &self.config)
            .field("project_root", &self.project_root)
            .field("container_id", &self.container_id)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl ExecController {
    #[must_use]
    pub fn new(
        id: String,
        runtime: ProcessRuntime,
        config: ServiceConfig,
        project_root: PathBuf,
        port: Option<u16>,
        env: std::collections::HashMap<String, String>,
    ) -> Self {
        Self::new_with_resource_id(id.clone(), id, runtime, config, project_root, port, env)
    }

    fn new_with_resource_id(
        id: String,
        resource_id: String,
        runtime: ProcessRuntime,
        config: ServiceConfig,
        project_root: PathBuf,
        port: Option<u16>,
        env: std::collections::HashMap<String, String>,
    ) -> Self {
        let (log_tx, _) = broadcast::channel(100);
        Self {
            id,
            resource_id,
            runtime,
            config,
            project_root,
            child: None,
            pty_master: None,
            pty_writer: None,
            container_id: None,
            owned_process_id: None,
            process_identity: None,
            cgroup_path: None,
            port,
            log_tx,
            pty_tx: None,
            bundle_dir: None,
            env,
            system: StdMutex::new(System::new()),
        }
    }

    fn resolve_env(&self) -> std::collections::HashMap<String, String> {
        self.env.clone()
    }

    fn escalation_signal_after_timeout(initial_signal: Signal) -> Option<Signal> {
        (initial_signal != Signal::SIGKILL).then_some(Signal::SIGKILL)
    }

    async fn wait_for_owned_cleanup(
        &self,
        child: &mut Option<Box<dyn Child + Send>>,
        pid: u32,
        identity: &PersistedProcessIdentity,
        timeout: std::time::Duration,
    ) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(child) = child.as_mut() {
                child.try_wait().with_context(|| {
                    format!(
                        "failed to inspect retained child for service {} PID {pid} during owned cleanup",
                        self.id
                    )
                })?;
            }
            if !self.runtime.owned_process_or_group_exists(pid, identity)? {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn stop_through_retained_child(
        child: &mut Box<dyn Child + Send>,
        timeout: std::time::Duration,
    ) -> Result<()> {
        // Keep the child unreaped until immediately before signaling it. A
        // live or unreaped child cannot have its PID reused; once `try_wait`
        // reaps an exited child, returning here avoids signaling that PID.
        if child
            .try_wait()
            .context("failed to inspect retained child before cleanup")?
            .is_some()
        {
            return Ok(());
        }

        child
            .kill()
            .context("failed to terminate process through its retained child handle")?;

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if child
                .try_wait()
                .context("failed to confirm exit through the retained child handle")?
                .is_some()
            {
                return Ok(());
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "process remained live after its retained child handle was killed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn adopt_spawned_process<F>(
        &mut self,
        spawned: SpawnedProcess,
        capture_identity: F,
    ) -> Result<()>
    where
        F: FnOnce(&ProcessRuntime, u32) -> Result<Option<PersistedProcessIdentity>>,
    {
        let (child, master, writer, container_id, mut log_rx, pty_tx) = spawned;
        let owned_process_id = child.process_id();

        // Adopt every successfully spawned handle before the first fallible
        // ownership step. A failed identity capture can then use the same
        // controller cleanup path and retain the handles if cleanup itself
        // cannot be confirmed.
        self.child = Some(StdMutex::new(child));
        self.pty_master = Some(StdMutex::new(master));
        self.pty_writer = Some(StdMutex::new(writer));
        self.container_id = Some(container_id);
        self.owned_process_id = owned_process_id;
        self.process_identity = None;
        self.pty_tx = Some(pty_tx);

        let identity_result = owned_process_id
            .context("spawned child did not expose a process ID")
            .and_then(|pid| {
                capture_identity(&self.runtime, pid)?.with_context(|| {
                    format!("spawned process {pid} exited before its identity could be captured")
                })
            });

        match identity_result {
            Ok(identity) => self.process_identity = Some(identity),
            Err(capture_error) => {
                // `stop` terminates through the retained child handle and, for
                // a known PID, verifies that neither the PID nor its process
                // group remains before clearing controller ownership.
                if let Err(cleanup_error) = self.stop().await {
                    return Err(cleanup_error).context(format!(
                        "failed to establish durable process ownership for service `{}`: {capture_error:#}; synchronous cleanup also failed and controller ownership was retained for retry",
                        self.id
                    ));
                }
                self.pty_tx = None;
                return Err(capture_error).context(format!(
                    "failed to establish durable process ownership for service `{}`; the spawned process was synchronously stopped",
                    self.id
                ));
            }
        }

        // Spawn log forwarder only after durable ownership can be published.
        let log_tx = self.log_tx.clone();
        let id = self.id.clone();
        tokio::spawn(async move {
            while let Some(entry) = log_rx.recv().await {
                if let Err(_e) = log_tx.send(entry) {
                    tracing::trace!("Failed to broadcast log for {}: no subscribers", id);
                }
            }
        });

        Ok(())
    }
}

#[async_trait]
impl ServiceController for ExecController {
    fn id(&self) -> &str {
        &self.id
    }

    async fn prepare(&mut self) -> Result<()> {
        // Phase 99: only set cgroupsPath once the cgroup root exists (admin setup).
        #[cfg(target_os = "linux")]
        {
            self.cgroup_path =
                locald_utils::cgroup::maybe_cgroup_path_for_service(&self.resource_id);
        }

        match &self.config {
            ServiceConfig::Typed(TypedServiceConfig::Exec(c)) | ServiceConfig::Legacy(c) => {
                if let Some(_build_config) = &c.build {
                    let service_path = c.workdir.as_ref().map_or_else(
                        || self.project_root.clone(),
                        |wd| self.project_root.join(wd),
                    );

                    let env = self.resolve_env();

                    // Setup log callback
                    let log_tx = self.log_tx.clone();
                    let id = self.id.clone();
                    let log_callback = std::sync::Arc::new(move |line: String| {
                        let tx = log_tx.clone();
                        let id = id.clone();
                        tokio::spawn(async move {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let timestamp = i64::try_from(timestamp).unwrap_or(i64::MAX);

                            let _ = tx.send(LogEntry {
                                timestamp,
                                service: id,
                                instance_id: None,
                                service_name: None,
                                service_domain: None,
                                stream: LogStream::Stdout,
                                message: line,
                            });
                        });
                    });

                    let bundle_dir = self
                        .runtime
                        .prepare_cnb_container(
                            self.resource_id.clone(),
                            &service_path,
                            c.command.as_ref(),
                            &env,
                            self.port,
                            false, // TODO: Pass verbose flag?
                            Some(log_callback),
                            self.cgroup_path.as_deref(),
                        )
                        .await?;

                    self.bundle_dir = Some(bundle_dir);
                }
            }
            ServiceConfig::Typed(TypedServiceConfig::Container(c)) => {
                let env = self.resolve_env();
                let bundle_dir = self
                    .runtime
                    .prepare_container(
                        self.resource_id.clone(),
                        c.image.clone(),
                        c.command.clone(),
                        &env,
                        self.port,
                        &self.project_root,
                        self.cgroup_path.as_deref(),
                    )
                    .await?;
                self.bundle_dir = Some(bundle_dir);
            }
            ServiceConfig::Typed(
                TypedServiceConfig::Postgres(_)
                | TypedServiceConfig::Worker(_)
                | TypedServiceConfig::Site(_)
                | TypedServiceConfig::Published(_),
            ) => {}
        }

        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let spawned = if let Some(bundle_dir) = &self.bundle_dir {
            self.runtime
                .start_container_process(self.id.clone(), bundle_dir)?
        } else {
            let (command, workdir) = match &self.config {
                ServiceConfig::Typed(TypedServiceConfig::Exec(c)) | ServiceConfig::Legacy(c) => {
                    (c.command.clone(), c.workdir.clone())
                }
                ServiceConfig::Typed(TypedServiceConfig::Worker(c)) => {
                    (Some(c.command.clone()), c.workdir.clone())
                }
                ServiceConfig::Typed(
                    TypedServiceConfig::Container(_)
                    | TypedServiceConfig::Postgres(_)
                    | TypedServiceConfig::Site(_)
                    | TypedServiceConfig::Published(_),
                ) => anyhow::bail!("Invalid config for ExecController (Host Process)"),
            };

            let service_path = workdir.map_or_else(
                || self.project_root.clone(),
                |wd| self.project_root.join(wd),
            );

            let env = self.resolve_env();

            let cmd_str = command.ok_or_else(|| anyhow::anyhow!("Command is required"))?;
            self.runtime.start_host_process(
                self.id.clone(),
                &service_path,
                &cmd_str,
                &env,
                self.port,
            )?
        };

        self.adopt_spawned_process(spawned, ProcessRuntime::capture_process_identity)
            .await
    }

    async fn stop(&mut self) -> Result<()> {
        let signal =
            self.config
                .common()
                .stop_signal
                .as_deref()
                .map_or(Signal::SIGTERM, |s| match s.to_uppercase().as_str() {
                    "SIGINT" | "INT" => Signal::SIGINT,
                    "SIGQUIT" | "QUIT" => Signal::SIGQUIT,
                    "SIGKILL" | "KILL" => Signal::SIGKILL,
                    _ => Signal::SIGTERM,
                });
        let mut child = self.child.take().map(|child_mutex| {
            child_mutex
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });

        let cleanup_result: Result<()> = async {
            match (self.owned_process_id, self.process_identity.clone()) {
                (Some(pid), Some(identity)) => {
                    self.runtime.signal_owned_process(pid, &identity, signal)?;
                    let exited = self
                        .wait_for_owned_cleanup(
                            &mut child,
                            pid,
                            &identity,
                            std::time::Duration::from_secs(5),
                        )
                        .await?;
                    if !exited {
                        let Some(escalation_signal) =
                            Self::escalation_signal_after_timeout(signal)
                        else {
                            anyhow::bail!(
                                "owned process or process group for service {} remained live after SIGKILL",
                                self.id
                            );
                        };
                        warn!(
                            "Service {} did not fully exit after {signal:?}; sending {escalation_signal:?}",
                            self.id,
                        );
                        self.runtime
                            .signal_owned_process(pid, &identity, escalation_signal)?;
                        anyhow::ensure!(
                            self.wait_for_owned_cleanup(
                                &mut child,
                                pid,
                                &identity,
                                std::time::Duration::from_secs(2),
                            )
                            .await?,
                            "owned process or process group for service {} remained live after SIGKILL",
                            self.id
                        );
                    }
                }
                (Some(pid), None) => {
                    if let Some(child) = child.as_mut() {
                        Self::stop_through_retained_child(
                            child,
                            std::time::Duration::from_secs(5),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "failed to stop service {} after its process identity could not be captured",
                                self.id
                            )
                        })?;
                    }
                    anyhow::ensure!(
                        !self.runtime.unverified_stale_process_exists(pid)?,
                        "service {} still has a live process or process group after retained-child cleanup, but locald could not capture an ownership identity; stop it manually",
                        self.id
                    );
                }
                (None, Some(_)) => {
                    anyhow::bail!(
                        "service {} retained a process identity without its owned process ID",
                        self.id
                    );
                }
                (None, None) => {
                    if let Some(child) = child.as_mut() {
                        Self::stop_through_retained_child(
                            child,
                            std::time::Duration::from_secs(5),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "failed to stop service {} whose child exposed no process ID",
                                self.id
                            )
                        })?;
                    }
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = cleanup_result {
            self.child = child.map(StdMutex::new);
            return Err(error);
        }

        // Phase 99: after graceful termination, ensure no leaked subprocesses remain by
        // killing the whole cgroup subtree.
        if let Some(cgroup_path) = self.cgroup_path.as_deref() {
            match locald_utils::shim::find_privileged() {
                Ok(Some(shim_path)) => {
                    let mut command = locald_utils::shim::tokio_command(&shim_path);
                    command
                        .arg("admin")
                        .arg("cgroup")
                        .arg("kill")
                        .arg("--path")
                        .arg(cgroup_path);
                    let status = match locald_utils::process_spawn::ProcessSpawnBarrier::global()
                        .spawn_tokio_command(&mut command)
                    {
                        Ok(mut child) => child.wait().await,
                        Err(error) => Err(error),
                    };

                    match status {
                        Ok(status) if status.success() => {}
                        Ok(status) => {
                            warn!(
                                "Cgroup cleanup failed for {}: locald-shim exited with status {status}",
                                self.id
                            );
                        }
                        Err(e) => {
                            warn!("Cgroup cleanup failed for {}: {e}", self.id);
                        }
                    }
                }
                Ok(None) => {
                    warn!(
                        "Skipping cgroup cleanup for {}: privileged locald-shim not configured. Run sudo locald admin setup",
                        self.id
                    );
                }
                Err(e) => {
                    warn!("Skipping cgroup cleanup for {}: {e}", self.id);
                }
            }
        }

        if let Some(container_id) = &self.container_id {
            if let Err(e) = self.runtime.stop_shim_container(container_id) {
                warn!("Cleanup warning: {:#}", e);
            }
        }

        self.pty_master = None;
        self.pty_writer = None;
        self.pty_tx = None;
        self.container_id = None;
        self.owned_process_id = None;
        self.process_identity = None;

        Ok(())
    }

    async fn write_stdin(&self, data: &[u8]) -> Result<()> {
        if let Some(writer) = &self.pty_writer {
            let mut writer = writer.lock().unwrap();
            writer.write_all(data)?;
        }
        Ok(())
    }

    async fn resize_pty(&self, rows: u16, cols: u16) -> Result<()> {
        if let Some(master) = &self.pty_master {
            let master = master.lock().unwrap();
            master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
        }
        Ok(())
    }

    async fn read_state(&self) -> RuntimeState {
        let (is_running, pid) = self.child.as_ref().map_or((false, None), |child_mutex| {
            child_mutex.lock().map_or((false, None), |mut child| {
                let running = matches!(child.try_wait(), Ok(None));
                let pid = running.then(|| child.process_id()).flatten();
                (running, pid)
            })
        });

        RuntimeState {
            pid,
            port: self.port,
            status: if is_running {
                ServiceState::Running
            } else {
                ServiceState::Stopped
            },
            health_status: if is_running {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unknown
            }, // Simplified
        }
    }

    fn owned_process_id(&self) -> Option<u32> {
        self.owned_process_id
    }

    fn process_identity(&self) -> Option<PersistedProcessIdentity> {
        self.process_identity.clone()
    }

    async fn logs(&self) -> BoxStream<'static, LogEntry> {
        let mut rx = self.log_tx.subscribe();
        Box::pin(stream! {
            while let Ok(entry) = rx.recv().await {
                yield entry;
            }
        })
    }

    fn get_metadata(&self, key: &str) -> Option<String> {
        match key {
            "port" => self.port.map(|p| p.to_string()),
            "cgroup_path" => self.cgroup_path.clone(),
            "container_id" => self.container_id.clone(),
            _ => None,
        }
    }

    async fn execute_command(&mut self, _cmd: ServiceCommand) -> Result<()> {
        Ok(())
    }

    fn subscribe_pty(&self) -> Option<broadcast::Receiver<Vec<u8>>> {
        self.pty_tx
            .as_ref()
            .map(tokio::sync::broadcast::Sender::subscribe)
    }

    fn snapshot(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    async fn restore(&mut self, _state: serde_json::Value) -> Result<()> {
        Ok(())
    }

    async fn metrics(&self) -> Result<Option<ServiceMetrics>> {
        let pid = match self.child.as_ref() {
            Some(child_mutex) => {
                let guard = child_mutex.lock().unwrap();
                guard.process_id()
            }
            None => return Ok(None),
        };

        let Some(pid) = pid else {
            return Ok(None);
        };

        let mut sys = self.system.lock().unwrap();
        let pid = Pid::from_u32(pid);

        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

        if let Some(process) = sys.process(pid) {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            return Ok(Some(ServiceMetrics {
                name: self.id.clone(),
                instance_id: None,
                service_name: None,
                cpu_percent: process.cpu_usage(),
                memory_bytes: process.memory(),
                timestamp: i64::try_from(timestamp).unwrap_or(0),
            }));
        }

        Ok(None)
    }
}

#[derive(Debug)]
pub struct ExecFactory {
    runtime: ProcessRuntime,
}

impl ExecFactory {
    #[must_use]
    pub fn new(runtime: ProcessRuntime) -> Self {
        Self { runtime }
    }
}

impl ServiceFactory for ExecFactory {
    fn can_handle(&self, config: &ServiceConfig) -> bool {
        match config {
            ServiceConfig::Typed(TypedServiceConfig::Exec(_)) => true,
            ServiceConfig::Legacy(_) => true,
            ServiceConfig::Typed(TypedServiceConfig::Worker(_)) => true,
            ServiceConfig::Typed(TypedServiceConfig::Container(_)) => true,
            ServiceConfig::Typed(TypedServiceConfig::Postgres(_)) => false,
            ServiceConfig::Typed(TypedServiceConfig::Site(_)) => false,
            ServiceConfig::Typed(TypedServiceConfig::Published(_)) => false,
        }
    }

    fn create(
        &self,
        name: String,
        config: &ServiceConfig,
        ctx: &ServiceContext,
    ) -> Arc<Mutex<dyn ServiceController>> {
        Arc::new(Mutex::new(ExecController::new_with_resource_id(
            name,
            ctx.key.resource_id(),
            self.runtime.clone(),
            config.clone(),
            ctx.project_root.clone(),
            ctx.bindings.primary_port(),
            ctx.env.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locald_core::config::{CommonServiceConfig, WorkerServiceConfig};
    use locald_core::state::PersistedProcessBirth;
    use portable_pty::{ChildKiller, ExitStatus};
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::tempdir;

    #[test]
    fn sigkill_is_a_terminal_stop_signal() {
        assert_eq!(
            ExecController::escalation_signal_after_timeout(Signal::SIGKILL),
            None
        );
        assert_eq!(
            ExecController::escalation_signal_after_timeout(Signal::SIGTERM),
            Some(Signal::SIGKILL)
        );
    }

    fn different_process_birth(birth: &PersistedProcessBirth) -> PersistedProcessBirth {
        match birth {
            PersistedProcessBirth::Macos {
                start_seconds,
                start_microseconds,
            } => PersistedProcessBirth::Macos {
                start_seconds: *start_seconds,
                start_microseconds: start_microseconds.saturating_add(1),
            },
            PersistedProcessBirth::Linux {
                boot_id,
                start_ticks,
            } => PersistedProcessBirth::Linux {
                boot_id: boot_id.clone(),
                start_ticks: start_ticks.saturating_add(1),
            },
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum PidlessChildBehavior {
        AlreadyExited,
        ExitOnKill,
        KillFails,
        ConfirmationFails,
        OwnedCleanupObservationFails,
        NeverExits,
    }

    #[derive(Debug)]
    struct PidlessChildState {
        behavior: PidlessChildBehavior,
        kill_calls: usize,
        wait_calls: usize,
        killed: bool,
    }

    #[derive(Debug)]
    struct PidlessChild {
        state: Arc<StdMutex<PidlessChildState>>,
        process_id: Option<u32>,
    }

    #[derive(Debug)]
    struct PidlessChildKiller {
        state: Arc<StdMutex<PidlessChildState>>,
    }

    fn kill_pidless_child(state: &Arc<StdMutex<PidlessChildState>>) -> std::io::Result<()> {
        let mut state = state.lock().expect("pidless child state is not poisoned");
        state.kill_calls += 1;
        if matches!(state.behavior, PidlessChildBehavior::KillFails) {
            return Err(std::io::Error::other("injected pidless child kill failure"));
        }
        state.killed = true;
        Ok(())
    }

    impl ChildKiller for PidlessChild {
        fn kill(&mut self) -> std::io::Result<()> {
            kill_pidless_child(&self.state)
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(PidlessChildKiller {
                state: self.state.clone(),
            })
        }
    }

    impl ChildKiller for PidlessChildKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            kill_pidless_child(&self.state)
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self {
                state: self.state.clone(),
            })
        }
    }

    impl Child for PidlessChild {
        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            let mut state = self
                .state
                .lock()
                .expect("pidless child state is not poisoned");
            state.wait_calls += 1;
            if matches!(
                state.behavior,
                PidlessChildBehavior::OwnedCleanupObservationFails
            ) {
                return Err(std::io::Error::other(
                    "injected owned-child observation failure",
                ));
            }
            if matches!(state.behavior, PidlessChildBehavior::AlreadyExited) {
                return Ok(Some(ExitStatus::with_exit_code(0)));
            }
            if !state.killed {
                return Ok(None);
            }
            if matches!(state.behavior, PidlessChildBehavior::ConfirmationFails) {
                return Err(std::io::Error::other(
                    "injected pidless child confirmation failure",
                ));
            }
            if matches!(state.behavior, PidlessChildBehavior::NeverExits) {
                return Ok(None);
            }
            Ok(Some(ExitStatus::with_exit_code(0)))
        }

        fn wait(&mut self) -> std::io::Result<ExitStatus> {
            self.try_wait()?.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "pidless test child is still running",
                )
            })
        }

        fn process_id(&self) -> Option<u32> {
            self.process_id
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    #[derive(Debug)]
    struct TestMasterPty;

    impl MasterPty for TestMasterPty {
        fn resize(&self, _size: PtySize) -> std::result::Result<(), anyhow::Error> {
            Ok(())
        }

        fn get_size(&self) -> std::result::Result<PtySize, anyhow::Error> {
            Ok(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
        }

        fn try_clone_reader(
            &self,
        ) -> std::result::Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(
            &self,
        ) -> std::result::Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
            Ok(Box::new(std::io::sink()))
        }

        #[cfg(unix)]
        fn process_group_leader(&self) -> Option<libc::pid_t> {
            None
        }

        #[cfg(unix)]
        fn as_raw_fd(&self) -> Option<portable_pty::unix::RawFd> {
            None
        }

        #[cfg(unix)]
        fn tty_name(&self) -> Option<PathBuf> {
            None
        }
    }

    fn spawned_test_process(
        behavior: PidlessChildBehavior,
        process_id: Option<u32>,
    ) -> (SpawnedProcess, Arc<StdMutex<PidlessChildState>>) {
        let state = Arc::new(StdMutex::new(PidlessChildState {
            behavior,
            kill_calls: 0,
            wait_calls: 0,
            killed: false,
        }));
        let child: Box<dyn Child + Send> = Box::new(PidlessChild {
            state: state.clone(),
            process_id,
        });
        let (_log_tx, log_rx) = mpsc::channel(1);
        let (pty_tx, _pty_rx) = broadcast::channel(1);
        (
            (
                child,
                Box::new(TestMasterPty),
                Box::new(std::io::sink()),
                "pidless-test".to_owned(),
                log_rx,
                pty_tx,
            ),
            state,
        )
    }

    fn pidless_spawned_process(
        behavior: PidlessChildBehavior,
    ) -> (SpawnedProcess, Arc<StdMutex<PidlessChildState>>) {
        spawned_test_process(behavior, None)
    }

    #[cfg(unix)]
    struct SentinelProcess(std::process::Child);

    #[cfg(unix)]
    impl SentinelProcess {
        fn spawn_group_leader() -> Self {
            use std::os::unix::process::CommandExt;

            let mut command = std::process::Command::new("sleep");
            command.arg("30").process_group(0);
            Self(command.spawn().expect("spawn sentinel process group"))
        }

        fn id(&self) -> u32 {
            self.0.id()
        }

        fn is_running(&mut self) -> bool {
            self.0
                .try_wait()
                .expect("inspect sentinel process")
                .is_none()
        }
    }

    #[cfg(unix)]
    impl Drop for SentinelProcess {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    async fn assert_identity_capture_failure_cleanup(
        capture_result: Result<Option<PersistedProcessIdentity>>,
        expected_error: &str,
    ) {
        let dir = tempdir().expect("create exec-controller test directory");
        let mut controller = ExecController::new(
            "capture-failure:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                command: "sleep 30".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::new(),
        );
        let captured_pid = Arc::new(AtomicU32::new(0));
        let recorded_pid = captured_pid.clone();

        let spawned = controller
            .runtime
            .start_host_process(
                controller.id.clone(),
                dir.path(),
                "sleep 30",
                &std::collections::HashMap::new(),
                None,
            )
            .expect("spawn process for identity-capture failure");
        let error = controller
            .adopt_spawned_process(spawned, move |_runtime, pid| {
                recorded_pid.store(pid, Ordering::SeqCst);
                capture_result
            })
            .await
            .expect_err("identity capture failure must fail the start");
        let error = format!("{error:#}");
        assert!(error.contains(expected_error), "unexpected error: {error}");
        assert!(error.contains("spawned process was synchronously stopped"));

        let pid = captured_pid.load(Ordering::SeqCst);
        assert!(pid > 1, "capture hook did not observe the spawned PID");
        assert!(
            !controller
                .runtime
                .unverified_stale_process_exists(pid)
                .expect("confirm spawned PID and process group are absent"),
            "failed start left PID {pid} or its process group alive"
        );
        assert!(controller.child.is_none());
        assert!(controller.pty_master.is_none());
        assert!(controller.pty_writer.is_none());
        assert!(controller.container_id.is_none());
        assert!(controller.get_metadata("container_id").is_none());
        assert!(controller.owned_process_id().is_none());
        assert!(controller.process_identity().is_none());
        assert!(controller.pty_tx.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn host_worker_resolves_commands_from_its_injected_path() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("create host PATH test directory");
        let bin = dir.path().join("toolchain-bin");
        std::fs::create_dir(&bin).expect("create injected PATH directory");
        let tool = bin.join("agent-lab-tool");
        std::fs::write(&tool, "#!/bin/sh\nexec /bin/sleep 30\n").expect("write injected PATH tool");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
            .expect("make injected PATH tool executable");

        let mut controller = ExecController::new(
            "trusted-path:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                command: "agent-lab-tool".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::from([(
                "PATH".to_owned(),
                format!("{}:/usr/bin:/bin", bin.display()),
            )]),
        );

        controller
            .start()
            .await
            .expect("resolve host command through injected PATH");
        assert_eq!(controller.read_state().await.status, ServiceState::Running);
        controller.stop().await.expect("stop injected PATH worker");
    }

    #[tokio::test]
    async fn identity_capture_failure_stops_and_confirms_the_spawned_process_is_gone() {
        assert_identity_capture_failure_cleanup(
            Err(anyhow::anyhow!("injected identity capture failure")),
            "injected identity capture failure",
        )
        .await;
    }

    #[tokio::test]
    async fn vanished_identity_capture_stops_and_confirms_the_spawned_process_is_gone() {
        assert_identity_capture_failure_cleanup(
            Ok(None),
            "exited before its identity could be captured",
        )
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn identity_capture_cleanup_never_signals_an_unverified_reused_process_group() {
        let dir = tempdir().expect("create exec-controller test directory");
        let mut controller = ExecController::new(
            "capture-reused-group:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                common: CommonServiceConfig {
                    stop_signal: Some("SIGKILL".to_owned()),
                    ..Default::default()
                },
                command: "unused".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::new(),
        );
        let mut sentinel = SentinelProcess::spawn_group_leader();
        let sentinel_pid = sentinel.id();
        let (spawned, child_state) =
            spawned_test_process(PidlessChildBehavior::AlreadyExited, Some(sentinel_pid));

        let error = controller
            .adopt_spawned_process(spawned, |_runtime, _pid| {
                Err(anyhow::anyhow!("injected identity capture failure"))
            })
            .await
            .expect_err("unverified reused process group must fail closed");
        let error = format!("{error:#}");
        assert!(error.contains("injected identity capture failure"));
        assert!(error.contains("still has a live process or process group"));
        assert!(error.contains("cleanup also failed"));

        let child_state = child_state
            .lock()
            .expect("test child state is not poisoned");
        assert_eq!(child_state.kill_calls, 0);
        assert_eq!(child_state.wait_calls, 1);
        drop(child_state);
        for _ in 0..25 {
            assert!(
                sentinel.is_running(),
                "identity-less cleanup signaled an unrelated reused process group"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(controller.child.is_some());
        assert_eq!(controller.owned_process_id(), Some(sentinel_pid));
        assert!(controller.process_identity().is_none());
    }

    #[tokio::test]
    async fn missing_pid_child_is_killed_and_confirmed_before_handles_are_cleared() {
        let dir = tempdir().expect("create exec-controller test directory");
        let mut controller = ExecController::new(
            "pidless-success:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                command: "unused".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::new(),
        );
        let (spawned, child_state) = pidless_spawned_process(PidlessChildBehavior::ExitOnKill);

        let error = controller
            .adopt_spawned_process(spawned, |_runtime, _pid| {
                panic!("identity capture must not run without a process ID")
            })
            .await
            .expect_err("missing process ID must fail the start");
        let error = format!("{error:#}");
        assert!(error.contains("did not expose a process ID"));
        assert!(error.contains("spawned process was synchronously stopped"));

        let child_state = child_state
            .lock()
            .expect("pidless child state is not poisoned");
        assert_eq!(child_state.kill_calls, 1);
        assert_eq!(child_state.wait_calls, 2);
        drop(child_state);
        assert!(controller.child.is_none());
        assert!(controller.pty_master.is_none());
        assert!(controller.pty_writer.is_none());
        assert!(controller.container_id.is_none());
        assert!(controller.get_metadata("container_id").is_none());
        assert!(controller.owned_process_id().is_none());
        assert!(controller.process_identity().is_none());
        assert!(controller.pty_tx.is_none());
    }

    #[tokio::test]
    async fn missing_pid_cleanup_failure_retains_every_controller_handle_for_retry() {
        for (behavior, expected_error) in [
            (
                PidlessChildBehavior::KillFails,
                "injected pidless child kill failure",
            ),
            (
                PidlessChildBehavior::ConfirmationFails,
                "injected pidless child confirmation failure",
            ),
        ] {
            let dir = tempdir().expect("create exec-controller test directory");
            let mut controller = ExecController::new(
                format!("pidless-failure-{behavior:?}:web"),
                ProcessRuntime::new(dir.path().join("notify.sock")),
                ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                    command: "unused".to_owned(),
                    ..Default::default()
                })),
                dir.path().to_path_buf(),
                None,
                std::collections::HashMap::new(),
            );
            let (spawned, child_state) = pidless_spawned_process(behavior);

            let error = controller
                .adopt_spawned_process(spawned, |_runtime, _pid| {
                    panic!("identity capture must not run without a process ID")
                })
                .await
                .expect_err("pidless cleanup failure must fail the start");
            let error = format!("{error:#}");
            assert!(error.contains(expected_error), "unexpected error: {error}");
            assert!(error.contains("cleanup also failed"));

            let child_state = child_state
                .lock()
                .expect("pidless child state is not poisoned");
            assert_eq!(child_state.kill_calls, 1);
            assert!(child_state.wait_calls >= 1);
            drop(child_state);
            assert!(controller.child.is_some());
            assert!(controller.pty_master.is_some());
            assert!(controller.pty_writer.is_some());
            assert!(controller.container_id.is_some());
            assert_eq!(
                controller.get_metadata("container_id").as_deref(),
                Some("pidless-test")
            );
            assert!(controller.owned_process_id().is_none());
            assert!(controller.process_identity().is_none());
            assert!(controller.pty_tx.is_some());
        }
    }

    #[tokio::test]
    async fn successful_stop_closes_the_current_pty_generation() {
        let dir = tempdir().expect("create exec-controller test directory");
        let mut controller = ExecController::new(
            "pty-teardown:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                command: "unused".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::new(),
        );
        let (spawned, _child_state) = pidless_spawned_process(PidlessChildBehavior::ExitOnKill);
        let (child, master, writer, container_id, _log_rx, pty_tx) = spawned;
        let mut old_generation = pty_tx.subscribe();
        controller.child = Some(StdMutex::new(child));
        controller.pty_master = Some(StdMutex::new(master));
        controller.pty_writer = Some(StdMutex::new(writer));
        controller.container_id = Some(container_id);
        controller.pty_tx = Some(pty_tx);

        controller.stop().await.expect("stop test controller");

        assert!(controller.subscribe_pty().is_none());
        assert!(matches!(
            old_generation.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn missing_pid_exit_confirmation_has_a_bounded_deadline() {
        let (spawned, child_state) = pidless_spawned_process(PidlessChildBehavior::NeverExits);
        let (mut child, _master, _writer, _container_id, _log_rx, _pty_tx) = spawned;

        let error =
            ExecController::stop_through_retained_child(&mut child, std::time::Duration::ZERO)
                .await
                .expect_err("a pidless child that remains live must time out");
        assert!(
            format!("{error:#}")
                .contains("remained live after its retained child handle was killed"),
            "unexpected error: {error:#}"
        );

        let child_state = child_state
            .lock()
            .expect("pidless child state is not poisoned");
        assert_eq!(child_state.kill_calls, 1);
        assert_eq!(child_state.wait_calls, 2);
    }

    #[tokio::test]
    async fn owned_cleanup_observation_failure_is_reported_without_releasing_the_child() {
        let dir = tempdir().expect("create exec-controller test directory");
        let controller = ExecController::new(
            "owned-cleanup-observation:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                command: "unused".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::new(),
        );
        let pid = u32::MAX;
        let (spawned, child_state) = spawned_test_process(
            PidlessChildBehavior::OwnedCleanupObservationFails,
            Some(pid),
        );
        let (child, _master, _writer, _container_id, _log_rx, _pty_tx) = spawned;
        let mut child = Some(child);
        let identity = PersistedProcessIdentity {
            birth: None,
            process_group_id: i32::MAX,
            executable: None,
        };

        let error = controller
            .wait_for_owned_cleanup(&mut child, pid, &identity, std::time::Duration::ZERO)
            .await
            .expect_err("child observation failure must fail owned cleanup");
        let error = format!("{error:#}");
        assert!(
            error.contains(&format!(
                "failed to inspect retained child for service {} PID {pid} during owned cleanup",
                controller.id
            )),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("injected owned-child observation failure"),
            "unexpected error: {error}"
        );
        assert!(child.is_some());
        assert_eq!(
            child_state
                .lock()
                .expect("pidless child state is not poisoned")
                .wait_calls,
            1
        );
    }

    #[tokio::test]
    async fn exited_child_never_reports_a_reusable_pid_or_loses_its_spawn_identity() {
        let dir = tempdir().expect("create exec-controller test directory");
        let mut controller = ExecController::new(
            "identity:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                command: "sleep 0.1".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::new(),
        );

        controller.start().await.expect("start short-lived child");
        let spawn_pid = controller
            .owned_process_id()
            .expect("capture cleanup PID before the child can be reaped");
        let spawn_identity = controller
            .process_identity()
            .expect("capture identity before the child can be reaped");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let stopped = loop {
            let state = controller.read_state().await;
            if state.status == ServiceState::Stopped {
                break state;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "short-lived child did not exit before the deadline"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        assert_eq!(stopped.status, ServiceState::Stopped);
        assert_eq!(stopped.pid, None);
        assert_eq!(controller.owned_process_id(), Some(spawn_pid));
        assert_eq!(controller.process_identity(), Some(spawn_identity));

        controller.stop().await.expect("release stopped child");
        assert_eq!(controller.owned_process_id(), None);
        assert_eq!(controller.process_identity(), None);
    }

    #[tokio::test]
    async fn failed_cleanup_retains_the_child_handle_and_ownership_for_retry() {
        let dir = tempdir().expect("create exec-controller test directory");
        let mut controller = ExecController::new(
            "retry-cleanup:web".to_owned(),
            ProcessRuntime::new(dir.path().join("notify.sock")),
            ServiceConfig::Typed(TypedServiceConfig::Worker(WorkerServiceConfig {
                command: "sleep 30".to_owned(),
                ..Default::default()
            })),
            dir.path().to_path_buf(),
            None,
            std::collections::HashMap::new(),
        );

        controller.start().await.expect("start owned child");
        let spawn_pid = controller
            .owned_process_id()
            .expect("capture owned process ID");
        let spawn_identity = controller
            .process_identity()
            .expect("capture owned process identity");
        let mismatched_birth = different_process_birth(
            spawn_identity
                .birth
                .as_ref()
                .expect("spawned process has birth authority"),
        );
        controller
            .process_identity
            .as_mut()
            .expect("retain owned identity")
            .birth = Some(mismatched_birth);

        let error = controller
            .stop()
            .await
            .expect_err("mismatched ownership fails closed");
        assert!(format!("{error:#}").contains("was reused"));
        assert!(controller.child.is_some());
        assert_eq!(controller.owned_process_id(), Some(spawn_pid));
        assert!(controller.process_identity().is_some());

        controller.process_identity = Some(spawn_identity);
        controller
            .stop()
            .await
            .expect("retry reaps the retained child");
        assert!(controller.child.is_none());
        assert_eq!(controller.owned_process_id(), None);
        assert_eq!(controller.process_identity(), None);
    }
}
