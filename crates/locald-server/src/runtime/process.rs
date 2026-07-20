use anyhow::{Context, Result};
use locald_builder::{
    BuilderImage, BundleSource, ContainerImage, Lifecycle, LocalLayoutBundleSource, ShimRuntime,
};
use locald_core::ipc::{LogEntry, LogStream};
use locald_core::state::{PersistedProcessBirth, PersistedProcessIdentity};
use locald_oci::{oci_layout, runtime_spec};
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, getpgrp};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Pid as SysinfoPid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

type ProcessHandle = (
    Box<dyn Child + Send>,
    Box<dyn MasterPty + Send>,
    Box<dyn std::io::Write + Send>,
    String,
    mpsc::Receiver<LogEntry>,
    broadcast::Sender<Vec<u8>>,
);

type PtyReader = Box<dyn std::io::Read + Send>;

struct LogReaderHandoff {
    reader: Option<PtyReader>,
    cancelled: bool,
}

struct PreparedLogStreamer {
    handoff: Arc<(StdMutex<LogReaderHandoff>, Condvar)>,
    log_rx: Option<mpsc::Receiver<LogEntry>>,
    pty_tx: Option<broadcast::Sender<Vec<u8>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    activated: bool,
}

impl PreparedLogStreamer {
    fn activate(
        mut self,
        reader: PtyReader,
    ) -> (mpsc::Receiver<LogEntry>, broadcast::Sender<Vec<u8>>) {
        let log_rx = self
            .log_rx
            .take()
            .expect("prepared log streamer retains its log receiver until activation");
        let pty_tx = self
            .pty_tx
            .take()
            .expect("prepared log streamer retains its PTY sender until activation");
        let (handoff, wake) = &*self.handoff;
        let mut handoff = handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        handoff.reader = Some(reader);
        drop(handoff);
        self.activated = true;
        wake.notify_one();
        (log_rx, pty_tx)
    }
}

impl Drop for PreparedLogStreamer {
    fn drop(&mut self) {
        if self.activated {
            return;
        }

        let (handoff, wake) = &*self.handoff;
        let mut handoff = handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        handoff.cancelled = true;
        drop(handoff);
        wake.notify_one();

        if self
            .thread
            .take()
            .is_some_and(|thread| thread.join().is_err())
        {
            warn!("Cancelled PTY log-reader thread panicked before activation");
        }
    }
}

#[derive(Clone)]
pub struct ProcessRuntime {
    notify_socket_path: PathBuf,
    process_system: Arc<StdMutex<System>>,
}

impl fmt::Debug for ProcessRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessRuntime")
            .field("notify_socket_path", &self.notify_socket_path)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedStaleProcess {
    pid: i32,
    identity: PersistedProcessIdentity,
}

impl ProcessRuntime {
    #[must_use]
    pub fn new(notify_socket_path: PathBuf) -> Self {
        Self {
            notify_socket_path,
            process_system: Arc::new(StdMutex::new(System::new())),
        }
    }

    pub fn kill_pid(&self, pid: i32, signal: Signal) -> Result<()> {
        locald_utils::process::kill_pid(pid, signal)
    }

    fn validated_stale_pid(pid: u32) -> Result<i32> {
        anyhow::ensure!(pid > 1, "refusing to operate on reserved process ID {pid}");
        anyhow::ensure!(
            pid != std::process::id(),
            "refusing to operate on the current locald process ({pid})"
        );
        let pid = i32::try_from(pid).context("recorded process ID exceeds the platform range")?;
        anyhow::ensure!(
            pid != getpgrp().as_raw(),
            "refusing to operate on the current locald process group ({pid})"
        );
        Ok(pid)
    }

    fn observed_executable(&self, pid: i32) -> Option<PathBuf> {
        let sysinfo_pid =
            SysinfoPid::from_u32(u32::try_from(pid).expect("validated process ID is positive"));
        let mut system = self
            .process_system
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sysinfo_pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        system
            .process(sysinfo_pid)
            .and_then(|process| process.exe().map(Path::to_path_buf))
    }

    #[cfg(target_os = "macos")]
    #[allow(
        unsafe_code,
        reason = "proc_pidinfo is the macOS kernel API for an atomic birth and process-group snapshot"
    )]
    fn observed_process_authority(pid: i32) -> Result<Option<PersistedProcessIdentity>> {
        let expected_size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
            .context("proc_bsdinfo exceeds the platform buffer-size range")?;
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        // SAFETY: `info` points to a correctly sized writable proc_bsdinfo
        // buffer. It is initialized only when proc_pidinfo reports a full read.
        let bytes_read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                expected_size,
            )
        };
        if bytes_read != expected_size {
            let inspection_error = std::io::Error::last_os_error();
            if bytes_read <= 0 && inspection_error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(None);
            }
            return match kill(Pid::from_raw(pid), None) {
                Err(Errno::ESRCH) => Ok(None),
                Ok(()) | Err(Errno::EPERM) => Err(anyhow::anyhow!(
                    "failed to read a complete process identity for PID {pid}: read {bytes_read} of {expected_size} bytes ({inspection_error})"
                )),
                Err(error) => Err(anyhow::anyhow!(
                    "failed to confirm PID {pid} after proc_pidinfo read {bytes_read} of {expected_size} bytes ({inspection_error}): {error}"
                )),
            };
        }
        // SAFETY: the exact-size check above proves proc_pidinfo initialized the
        // entire proc_bsdinfo value.
        let info = unsafe { info.assume_init() };
        let expected_pid = u32::try_from(pid).context("validated process ID became negative")?;
        anyhow::ensure!(
            info.pbi_pid == expected_pid,
            "process identity snapshot for PID {pid} reported PID {}",
            info.pbi_pid
        );
        let process_group_id =
            i32::try_from(info.pbi_pgid).context("process group ID exceeds the platform range")?;
        anyhow::ensure!(
            process_group_id > 0,
            "process identity snapshot for PID {pid} reported invalid process group {process_group_id}"
        );

        Ok(Some(PersistedProcessIdentity {
            birth: Some(PersistedProcessBirth::Macos {
                start_seconds: info.pbi_start_tvsec,
                start_microseconds: info.pbi_start_tvusec,
            }),
            process_group_id,
            executable: None,
        }))
    }

    #[cfg(any(target_os = "linux", test))]
    fn parse_linux_proc_stat(stat: &str) -> Result<(i32, i32, u64)> {
        let command_start = stat
            .find('(')
            .context("Linux process stat is missing its command start")?;
        let fields_start = stat
            .rfind(") ")
            .map(|index| index + 2)
            .context("Linux process stat is missing its command terminator")?;
        anyhow::ensure!(
            command_start < fields_start,
            "Linux process stat has malformed command boundaries"
        );
        let observed_pid = stat[..command_start]
            .trim()
            .parse::<i32>()
            .context("Linux process stat contains an invalid PID")?;
        let fields = stat[fields_start..]
            .split_ascii_whitespace()
            .collect::<Vec<_>>();
        let process_group_id = fields
            .get(2)
            .context("Linux process stat is missing its process group")?
            .parse::<i32>()
            .context("Linux process stat contains an invalid process group")?;
        let start_ticks = fields
            .get(19)
            .context("Linux process stat is missing its start ticks")?
            .parse::<u64>()
            .context("Linux process stat contains invalid start ticks")?;
        anyhow::ensure!(
            process_group_id > 0,
            "Linux process stat reported invalid process group {process_group_id}"
        );
        Ok((observed_pid, process_group_id, start_ticks))
    }

    #[cfg(target_os = "linux")]
    #[allow(
        clippy::disallowed_methods,
        reason = "small /proc pseudo-files provide the synchronous kernel identity snapshot used immediately before process authorization"
    )]
    fn observed_process_authority(pid: i32) -> Result<Option<PersistedProcessIdentity>> {
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ESRCH) =>
            {
                return Ok(None);
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read identity for PID {pid}"));
            }
        };
        let (observed_pid, process_group_id, start_ticks) = Self::parse_linux_proc_stat(&stat)?;
        anyhow::ensure!(
            observed_pid == pid,
            "process identity snapshot for PID {pid} reported PID {observed_pid}"
        );
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .context("failed to read the Linux kernel boot identity")?;
        let boot_id = boot_id.trim();
        anyhow::ensure!(!boot_id.is_empty(), "Linux kernel boot identity is empty");

        Ok(Some(PersistedProcessIdentity {
            birth: Some(PersistedProcessBirth::Linux {
                boot_id: boot_id.to_owned(),
                start_ticks,
            }),
            process_group_id,
            executable: None,
        }))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn observed_process_authority(pid: i32) -> Result<Option<PersistedProcessIdentity>> {
        anyhow::bail!(
            "durable process birth identity for PID {pid} is unsupported on {}",
            std::env::consts::OS
        )
    }

    fn ensure_same_process_identity(
        pid: i32,
        observed: &PersistedProcessIdentity,
        expected: &PersistedProcessIdentity,
    ) -> Result<()> {
        let expected_birth = expected.birth.as_ref().with_context(|| {
            format!(
                "recorded PID {pid} has no high-resolution process birth identity; stop it manually"
            )
        })?;
        let observed_birth = observed.birth.as_ref().with_context(|| {
            format!("platform process observation for PID {pid} has no birth identity")
        })?;
        anyhow::ensure!(
            observed_birth == expected_birth,
            "recorded PID {pid} was reused: expected birth {expected_birth:?}, observed {observed_birth:?}"
        );
        anyhow::ensure!(
            observed.process_group_id == expected.process_group_id,
            "recorded PID {pid} changed process group: expected {}, observed {}",
            expected.process_group_id,
            observed.process_group_id
        );
        Ok(())
    }

    /// Capture an ownership fingerprint while locald still owns the process.
    pub fn capture_process_identity(&self, pid: u32) -> Result<Option<PersistedProcessIdentity>> {
        let pid = Self::validated_stale_pid(pid)?;
        // Executable metadata is diagnostic only. Observe it before the
        // platform-native authority snapshot so it cannot widen the window
        // between the final ownership check and a caller's next action.
        let executable = self.observed_executable(pid);
        let Some(mut identity) = Self::observed_process_authority(pid)? else {
            return Ok(None);
        };
        identity.executable = executable;
        Ok(Some(identity))
    }

    /// Signal a process or process group that a live controller identified at
    /// spawn time. The process leader must still match its captured identity
    /// before locald can authorize either target. Once the leader disappears,
    /// a surviving group cannot be distinguished from a later PGID reuse and
    /// cleanup therefore fails closed while retaining ownership evidence.
    pub fn signal_owned_process(
        &self,
        pid: u32,
        expected: &PersistedProcessIdentity,
        signal: Signal,
    ) -> Result<()> {
        let Some(verified) = self.verify_stale_process(pid, expected)? else {
            return Ok(());
        };
        let target = if Self::can_signal_verified_group(&verified) {
            Pid::from_raw(-verified.identity.process_group_id)
        } else {
            Pid::from_raw(verified.pid)
        };

        match kill(target, signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(anyhow::anyhow!(
                "failed to signal owned process {pid} (target {}) with {signal:?}: {error}",
                target.as_raw()
            )),
        }
    }

    /// Return whether a live controller's captured process or authorized group
    /// still exists. Identity mismatches fail closed.
    pub fn owned_process_or_group_exists(
        &self,
        pid: u32,
        expected: &PersistedProcessIdentity,
    ) -> Result<bool> {
        let pid = Self::validated_stale_pid(pid)?;
        self.verified_stale_process_exists(&VerifiedStaleProcess {
            pid,
            identity: expected.clone(),
        })
    }

    /// Verify that a persisted PID still names the process locald recorded.
    /// A missing process is safe to forget; any mismatch fails closed.
    pub fn verify_stale_process(
        &self,
        pid: u32,
        expected: &PersistedProcessIdentity,
    ) -> Result<Option<VerifiedStaleProcess>> {
        let pid = Self::validated_stale_pid(pid)?;
        let Some(observed) = Self::observed_process_authority(pid)? else {
            if expected.process_group_id == pid
                && expected.process_group_id > 1
                && expected.process_group_id != getpgrp().as_raw()
            {
                match kill(Pid::from_raw(-expected.process_group_id), None) {
                    Ok(()) | Err(Errno::EPERM) => {
                        anyhow::bail!(
                            "recorded leader PID {pid} is gone but its verified process group {} remains live; ownership cannot be revalidated",
                            expected.process_group_id
                        );
                    }
                    Err(Errno::ESRCH) => {}
                    Err(error) => {
                        return Err(anyhow::anyhow!(
                            "failed to inspect recorded process group {} after leader PID {pid} exited: {error}",
                            expected.process_group_id
                        ));
                    }
                }
            }
            return Ok(None);
        };
        Self::ensure_same_process_identity(pid, &observed, expected)?;
        Ok(Some(VerifiedStaleProcess {
            pid,
            identity: expected.clone(),
        }))
    }

    fn can_signal_verified_group(process: &VerifiedStaleProcess) -> bool {
        process.identity.process_group_id > 1
            && process.identity.process_group_id == process.pid
            && process.identity.process_group_id != getpgrp().as_raw()
    }

    /// Signal a process whose persisted ownership fingerprint was verified in
    /// this reconciliation pass. Group signaling is used only for a distinct,
    /// recorded process group; otherwise locald signals the verified PID.
    pub fn signal_verified_stale_process(
        &self,
        process: &VerifiedStaleProcess,
        signal: Signal,
    ) -> Result<()> {
        let pid = u32::try_from(process.pid).context("verified process ID became negative")?;
        let Some(current) = self.verify_stale_process(pid, &process.identity)? else {
            return Ok(());
        };
        let target = if Self::can_signal_verified_group(&current) {
            Pid::from_raw(-current.identity.process_group_id)
        } else {
            Pid::from_raw(current.pid)
        };
        match kill(target, signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(anyhow::anyhow!(
                "failed to signal verified stale process {} (target {}): {error}",
                current.pid,
                target.as_raw()
            )),
        }
    }

    /// Return whether a previously verified process or its owned group remains
    /// live. If the PID is reused while cleanup is in flight, fail closed.
    pub fn verified_stale_process_exists(&self, process: &VerifiedStaleProcess) -> Result<bool> {
        if let Some(observed) = Self::observed_process_authority(process.pid)? {
            Self::ensure_same_process_identity(process.pid, &observed, &process.identity)
                .context("recorded process changed identity while stale cleanup was in flight")?;
        }

        let group_exists = if Self::can_signal_verified_group(process) {
            match kill(Pid::from_raw(-process.identity.process_group_id), None) {
                Ok(()) | Err(Errno::EPERM) => true,
                Err(Errno::ESRCH) => false,
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "failed to inspect verified stale process group {}: {error}",
                        process.identity.process_group_id
                    ));
                }
            }
        } else {
            false
        };
        let process_exists = match kill(Pid::from_raw(process.pid), None) {
            Ok(()) | Err(Errno::EPERM) => true,
            Err(Errno::ESRCH) => false,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "failed to inspect verified stale process {}: {error}",
                    process.pid
                ));
            }
        };

        let exists = group_exists || process_exists;
        if exists {
            anyhow::ensure!(
                process.identity.birth.is_some(),
                "recorded PID {} has no high-resolution process birth identity; stop it manually",
                process.pid
            );
        }

        Ok(exists)
    }

    /// Check a legacy bare PID without treating it as authorization to signal.
    pub fn unverified_stale_process_exists(&self, pid: u32) -> Result<bool> {
        let pid = Self::validated_stale_pid(pid)?;
        let process_exists = match kill(Pid::from_raw(pid), None) {
            Ok(()) | Err(Errno::EPERM) => true,
            Err(Errno::ESRCH) => false,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "failed to inspect unverified stale process {pid}: {error}"
                ));
            }
        };
        let group_exists = match kill(Pid::from_raw(-pid), None) {
            Ok(()) | Err(Errno::EPERM) => true,
            Err(Errno::ESRCH) => false,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "failed to inspect unverified stale process group {pid}: {error}"
                ));
            }
        };
        Ok(process_exists || group_exists)
    }

    pub fn stop_shim_container(&self, id: &str) -> Result<()> {
        // Option A: The shim runs the container as its foreground child.
        // Stopping is handled by terminating the shim process; container state
        // is cleaned up by the shim on exit.
        info!("Stopping Shim container {}", id);
        Ok(())
    }

    fn create_pty() -> Result<portable_pty::PtyPair> {
        let pty_system = NativePtySystem::default();
        pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to create PTY")
    }

    fn prepare_log_streamer(service_name: String) -> Result<PreparedLogStreamer> {
        let (tx, rx) = mpsc::channel(100);
        let (pty_tx, _) = broadcast::channel(100);
        let pty_tx_clone = pty_tx.clone();
        let handoff = Arc::new((
            StdMutex::new(LogReaderHandoff {
                reader: None,
                cancelled: false,
            }),
            Condvar::new(),
        ));
        let thread_handoff = handoff.clone();

        let thread = std::thread::Builder::new()
            .spawn(move || {
                let mut reader = {
                    let (handoff, wake) = &*thread_handoff;
                    let mut handoff = handoff
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    while handoff.reader.is_none() && !handoff.cancelled {
                        handoff = wake
                            .wait(handoff)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    let Some(reader) = handoff.reader.take() else {
                        return;
                    };
                    reader
                };
                let mut buffer = Vec::new();
                let mut buf = [0u8; 4096];

                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = buf[0..n].to_vec();
                            let _ = pty_tx_clone.send(data.clone());

                            buffer.extend_from_slice(&data);
                            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                                let line_bytes: Vec<u8> = buffer.drain(0..=pos).collect();
                                let line_len = line_bytes.len();
                                let line_content =
                                    if line_len > 0 && line_bytes[line_len - 1] == b'\n' {
                                        &line_bytes[..line_len - 1]
                                    } else {
                                        &line_bytes[..]
                                    };

                                let line = String::from_utf8_lossy(line_content).to_string();

                                let timestamp = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs();
                                let timestamp = i64::try_from(timestamp).unwrap_or(i64::MAX);

                                let entry = LogEntry {
                                    timestamp,
                                    service: service_name.clone(),
                                    stream: LogStream::Stdout,
                                    message: line,
                                };
                                if tx.blocking_send(entry).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("Failed to start PTY log reader")?;
        Ok(PreparedLogStreamer {
            handoff,
            log_rx: Some(rx),
            pty_tx: Some(pty_tx),
            thread: Some(thread),
            activated: false,
        })
    }

    fn spawn_bundle_process(name: String, bundle_dir: &Path) -> Result<ProcessHandle> {
        let container_id = format!("locald-{}", uuid::Uuid::new_v4());
        let shim_path = ShimRuntime::find_shim()?;

        info!("Spawning shim for {} (container {})", name, container_id);

        let pair = Self::create_pty()?;

        let mut cmd = CommandBuilder::new(shim_path);
        cmd.arg("bundle");
        cmd.arg("run");
        cmd.arg("--bundle");
        cmd.arg(bundle_dir);
        cmd.arg("--id");
        cmd.arg(&container_id);

        // Finish every fallible PTY setup step before spawning. Once the OS
        // child exists, the returned handle can move directly into controller
        // ownership without another error path that could strand it.
        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;
        // The prepared thread owns no PTY reader until the child exists. A
        // failed spawn drops this guard, cancels the waiter, and closes every
        // local PTY handle without leaving a blocking reader thread behind.
        let log_streamer = Self::prepare_log_streamer(name)?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn process")?;
        let (rx, pty_tx) = log_streamer.activate(reader);

        let master = pair.master;

        Ok((child, master, writer, container_id, rx, pty_tx))
    }

    pub fn start_host_process(
        &self,
        name: String,
        path: &Path,
        command: &str,
        env: &HashMap<String, String>,
        port: Option<u16>,
    ) -> Result<ProcessHandle> {
        info!("Starting host process for service {}", name);

        let pair = Self::create_pty()?;

        // Use sh -c to allow shell expansion and features
        let mut cmd = CommandBuilder::new("sh");
        cmd.arg("-c");
        cmd.arg(command);
        cmd.cwd(path);

        // Set environment variables
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(p) = port {
            cmd.env("PORT", p.to_string());
        }
        cmd.env(
            "NOTIFY_SOCKET",
            self.notify_socket_path.display().to_string(),
        );

        // Prepare all fallible PTY I/O before the process exists. The caller
        // can therefore adopt every successfully spawned child atomically.
        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;
        // Prepare the thread before spawning, but transfer the blocking PTY
        // reader only after the child exists. Dropping an unactivated guard
        // cancels and joins the waiting thread.
        let log_streamer = Self::prepare_log_streamer(name)?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn process")?;
        let (rx, pty_tx) = log_streamer.activate(reader);

        let master = pair.master;

        // Generate a pseudo-container ID for tracking
        let container_id = format!("host-{}", uuid::Uuid::new_v4());

        Ok((child, master, writer, container_id, rx, pty_tx))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_cnb_container(
        &self,
        name: String,
        path: &Path,
        command: Option<&String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        verbose: bool,
        log_callback: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
        cgroup_path: Option<&str>,
    ) -> Result<PathBuf> {
        info!("Preparing containerized service {}", name);

        // 1. Setup directories
        let home = directories::UserDirs::new()
            .ok_or_else(|| anyhow::anyhow!("Failed to get user dirs"))?
            .home_dir()
            .to_path_buf();

        // Use BuilderImage to prepare the environment
        let cache_root = home.join(".local/share/locald/builders");
        let builder_image_name = "heroku/builder:22"; // TODO: Make configurable
        let builder_dir_name = builder_image_name.replace(['/', ':'], "_");
        let builder_cache_dir = cache_root.join(&builder_dir_name);

        let builder = BuilderImage::new(builder_image_name, &builder_cache_dir, vec![]);
        let cnb_dir = builder
            .ensure_available()
            .await
            .context("Failed to prepare builder image")?;

        let lifecycle = Lifecycle::new(&cnb_dir);

        let state_dir = locald_utils::project::get_state_dir(path);
        let build_dir = state_dir.join("build");
        let cache_dir = state_dir.join("cache");

        // Clean up previous build artifacts
        // We don't check exists() first because it might return false for permission errors
        if let Err(e) = tokio::fs::remove_dir_all(&build_dir).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    "Failed to clean build dir: {}. Attempting privileged cleanup...",
                    e
                );
                ShimRuntime::cleanup_path(&build_dir)
                    .await
                    .context("Failed to clean build dir (privileged)")?;
            }
        }
        tokio::fs::create_dir_all(&build_dir).await?;

        let rootfs = build_dir.join("rootfs");

        // 2. Build
        info!("Building service {}...", name);

        lifecycle
            .run_creator(
                path,
                &format!("locald-{}", name.replace(':', "-")),
                &cache_dir,
                &build_dir,
                verbose,
                log_callback,
            )
            .await
            .context("Failed to build service")?;

        // Fetch run image environment (e.g. PATH)
        let run_image_env = lifecycle
            .get_run_image_env(&build_dir)
            .await
            .unwrap_or_default();

        // 3. Unpack
        info!("Unpacking service {}...", name);
        let image_name = format!("locald-{}", name.replace(':', "-"));
        let layout_path = build_dir
            .join("oci-layout")
            .join("index.docker.io")
            .join("library")
            .join(&image_name)
            .join("latest");

        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let bundle_source = LocalLayoutBundleSource {
            layout_path: layout_path.clone(),
            image_ref: "latest".to_string(),
            app_dir: abs_path,
        };

        let bundle_info = bundle_source.prepare_rootfs(&build_dir).await?;

        // Fetch labels
        let labels = oci_layout::get_image_labels("latest", &layout_path)
            .await
            .unwrap_or_default();

        tracing::info!("Available labels: {:?}", labels.keys());

        // 4. Generate Config
        let cmd_args = if let Some(command_str) = command {
            vec![
                "/cnb/lifecycle/launcher".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                (*command_str).clone(),
            ]
        } else {
            // Ensure metadata is available for the launcher
            let metadata_path = rootfs.join("layers/config/metadata.toml");
            if !metadata_path.exists() {
                // Try to restore from label
                let metadata_json = labels
                    .get("io.buildpacks.build.metadata")
                    .or_else(|| labels.get("io.buildpacks.lifecycle.metadata"));

                if let Some(json_str) = metadata_json {
                    tracing::info!("Restoring metadata.toml from image label");
                    if let Ok(val) = serde_json::from_str::<toml::Value>(json_str) {
                        if let Some(parent) = metadata_path.parent() {
                            let _ = tokio::fs::create_dir_all(parent).await;
                        }
                        if let Ok(toml_str) = toml::to_string_pretty(&val) {
                            let _ = tokio::fs::write(&metadata_path, toml_str).await;
                        }
                    } else {
                        tracing::warn!(
                            "Failed to parse metadata label as TOML compatible structure"
                        );
                    }
                }
            }

            vec!["/cnb/lifecycle/launcher".to_string()]
        };

        let mut env_vec = vec![
            "CNB_PLATFORM_API=0.12".to_string(),
            "CNB_EXPERIMENTAL_MODE=warn".to_string(),
            "CNB_LAYERS_DIR=/layers".to_string(),
            "CNB_APP_DIR=/workspace".to_string(),
        ];

        // Add run image environment (base)
        env_vec.extend(run_image_env);

        // Add image environment (overrides)
        env_vec.extend(bundle_info.env);

        for (k, v) in env {
            env_vec.push(format!("{k}={v}"));
        }
        if let Some(p) = port {
            env_vec.push(format!("PORT={p}"));
        }
        env_vec.push(format!(
            "NOTIFY_SOCKET={}",
            self.notify_socket_path.display()
        ));

        let uid = nix::unistd::getuid().as_raw();
        let gid = nix::unistd::getgid().as_raw();

        let spec = runtime_spec::generate_config(
            std::path::Path::new("rootfs"),
            &cmd_args,
            &env_vec,
            &bundle_info.bind_mounts,
            uid,
            gid,
            0, // Run as root inside container for now
            0,
            None,
            cgroup_path,
        )?;

        let bundle_dir = build_dir.clone();
        let config_path = bundle_dir.join("config.json");
        let json_str = serde_json::to_string_pretty(&spec)?;
        tokio::fs::write(&config_path, json_str).await?;

        Ok(bundle_dir)
    }

    pub fn start_container_process(
        &self,
        name: String,
        bundle_dir: &Path,
    ) -> Result<ProcessHandle> {
        Self::spawn_bundle_process(name, bundle_dir)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_cnb_container(
        &self,
        name: String,
        path: &Path,
        command: Option<&String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        verbose: bool,
        event_tx: Option<mpsc::Sender<locald_core::ipc::BootEvent>>,
    ) -> Result<ProcessHandle> {
        #[allow(clippy::option_if_let_else)]
        let log_callback = if let Some(tx) = event_tx {
            let name = name.clone();
            Some(std::sync::Arc::new(move |line: String| {
                let tx = tx.clone();
                let name = name.clone();
                tokio::spawn(async move {
                    let _ = tx
                        .send(locald_core::ipc::BootEvent::Log {
                            id: name,
                            line,
                            stream: locald_core::ipc::LogStream::Stdout,
                        })
                        .await;
                });
            })
                as std::sync::Arc<dyn Fn(String) + Send + Sync>)
        } else {
            None
        };

        let bundle_dir = self
            .prepare_cnb_container(
                name.clone(),
                path,
                command,
                env,
                port,
                verbose,
                log_callback,
                None,
            )
            .await?;

        // 5. Run via Shim
        Self::spawn_bundle_process(name, &bundle_dir)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_container(
        &self,
        name: String,
        image: String,
        command: Option<String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        path: &Path,
        cgroup_path: Option<&str>,
    ) -> Result<PathBuf> {
        info!("Preparing container service {} from image {}", name, image);

        // 1. Setup directories
        let home = directories::UserDirs::new()
            .ok_or_else(|| anyhow::anyhow!("Failed to get user dirs"))?
            .home_dir()
            .to_path_buf();

        let cache_root = home.join(".local/share/locald/images");
        let image_dir_name = image.replace(['/', ':'], "_");
        let image_cache_dir = cache_root.join(&image_dir_name);

        let state_dir = locald_utils::project::get_state_dir(path);
        let bundle_dir = state_dir.join("containers").join(&name);

        // 2. Prepare Bundle
        let container_image = ContainerImage::new(&image, &image_cache_dir);
        let bundle_info = container_image.prepare_rootfs(&bundle_dir).await?;

        // 3. Generate Config
        let cmd_args = command.map_or_else(
            || {
                bundle_info
                    .command
                    .unwrap_or_else(|| vec!["/bin/sh".to_string()])
            },
            |cmd_str| vec!["/bin/sh".to_string(), "-c".to_string(), cmd_str],
        );

        let mut env_vec = Vec::new();
        env_vec.extend(bundle_info.env);

        for (k, v) in env {
            env_vec.push(format!("{k}={v}"));
        }
        if let Some(p) = port {
            env_vec.push(format!("PORT={p}"));
        }
        env_vec.push(format!(
            "NOTIFY_SOCKET={}",
            self.notify_socket_path.display()
        ));

        let uid = nix::unistd::getuid().as_raw();
        let gid = nix::unistd::getgid().as_raw();

        let spec = runtime_spec::generate_config(
            std::path::Path::new("rootfs"),
            &cmd_args,
            &env_vec,
            &bundle_info.bind_mounts,
            uid,
            gid,
            0,
            0,
            bundle_info.workdir.as_deref(),
            cgroup_path,
        )?;

        let config_path = bundle_dir.join("config.json");
        let json_str = serde_json::to_string_pretty(&spec)?;
        tokio::fs::write(&config_path, json_str).await?;

        Ok(bundle_dir)
    }

    pub async fn start_container(
        &self,
        name: String,
        image: String,
        command: Option<String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        path: &Path,
    ) -> Result<ProcessHandle> {
        let bundle_dir = self
            .prepare_container(name.clone(), image, command, env, port, path, None)
            .await?;
        // 4. Run via Shim
        Self::spawn_bundle_process(name, &bundle_dir)
    }

    pub async fn terminate_process(child: &mut Box<dyn Child + Send>, name: &str, signal: Signal) {
        locald_utils::process::terminate_gracefully(child, name, signal).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Child as StdChild, Command};

    struct ChildCleanup(Option<StdChild>);

    impl ChildCleanup {
        fn new(child: StdChild) -> Self {
            Self(Some(child))
        }

        fn id(&self) -> u32 {
            self.0.as_ref().expect("test child is present").id()
        }

        fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
            self.0.as_mut().expect("test child is present").try_wait()
        }

        fn terminate_and_reap(&mut self) {
            let child = self.0.as_mut().expect("test child is present");
            child.kill().expect("terminate test child");
            child.wait().expect("reap test child");
            self.0 = None;
        }
    }

    impl Drop for ChildCleanup {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    struct ProcessGroupCleanup {
        group: i32,
        armed: bool,
    }

    impl ProcessGroupCleanup {
        fn new(pid: u32) -> Self {
            Self {
                group: i32::try_from(pid).expect("test process-group ID fits i32"),
                armed: true,
            }
        }

        fn group(&self) -> i32 {
            self.group
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for ProcessGroupCleanup {
        fn drop(&mut self) {
            if self.armed {
                let _ = kill(Pid::from_raw(-self.group), Signal::SIGKILL);
            }
        }
    }

    fn assert_test_thread_completes(action: impl FnOnce() + Send + 'static) {
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(0);
        std::thread::spawn(move || {
            action();
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("test thread completed before timeout");
    }

    #[test]
    fn cloned_runtime_reuses_process_observation_state() {
        let runtime = ProcessRuntime::new(PathBuf::from("notify.sock"));
        let cloned = runtime.clone();

        assert!(Arc::ptr_eq(&runtime.process_system, &cloned.process_system));
    }

    #[test]
    fn prepared_log_streamer_cancels_before_reader_handoff() {
        assert_test_thread_completes(|| {
            let streamer = ProcessRuntime::prepare_log_streamer("cancelled:test".to_owned())
                .expect("prepare log streamer");

            drop(streamer);
        });
    }

    #[test]
    fn failed_child_spawn_cancels_the_waiting_log_streamer() {
        assert_test_thread_completes(|| {
            let pair = ProcessRuntime::create_pty().expect("create test PTY");
            let _reader = pair
                .master
                .try_clone_reader()
                .expect("clone test PTY reader");
            let _writer = pair.master.take_writer().expect("take test PTY writer");
            let _streamer = ProcessRuntime::prepare_log_streamer("failed-spawn:test".to_owned())
                .expect("prepare log streamer");
            let command = CommandBuilder::new(format!(
                "/locald-test-missing-executable-{}",
                uuid::Uuid::new_v4()
            ));

            pair.slave
                .spawn_command(command)
                .expect_err("missing executable must fail before reader handoff");
        });
    }

    #[tokio::test]
    async fn prepared_log_streamer_reads_after_child_side_activation() {
        let streamer = ProcessRuntime::prepare_log_streamer("activated:test".to_owned())
            .expect("prepare log streamer");
        let reader: PtyReader = Box::new(std::io::Cursor::new(b"ready\n".to_vec()));
        let (mut log_rx, _pty_tx) = streamer.activate(reader);

        let entry = tokio::time::timeout(std::time::Duration::from_secs(1), log_rx.recv())
            .await
            .expect("log streamer produced output before timeout")
            .expect("log streamer kept its output channel open");
        assert_eq!(entry.service, "activated:test");
        assert_eq!(entry.message, "ready");
    }

    #[test]
    fn live_process_without_birth_authority_cannot_be_signaled() {
        let runtime = ProcessRuntime::new(PathBuf::from("notify.sock"));
        let mut child = ChildCleanup::new(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn legacy-identity test process"),
        );
        let pid = child.id();
        let mut identity = runtime
            .capture_process_identity(pid)
            .expect("capture process identity")
            .expect("test process is live");
        assert!(identity.birth.is_some());
        identity.birth = None;

        let error = runtime
            .signal_owned_process(pid, &identity, Signal::SIGTERM)
            .expect_err("legacy identity must not authorize a live process");
        assert!(format!("{error:#}").contains("no high-resolution process birth identity"));
        assert!(
            child
                .try_wait()
                .expect("inspect legacy-identity test process")
                .is_none()
        );

        child.terminate_and_reap();
    }

    #[test]
    fn verified_signal_accepts_a_process_that_vanished_before_the_signal() {
        let runtime = ProcessRuntime::new(PathBuf::from("notify.sock"));
        let mut child = ChildCleanup::new(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn stale-signal race process"),
        );
        let pid = child.id();
        let identity = runtime
            .capture_process_identity(pid)
            .expect("capture stale-signal race identity")
            .expect("stale-signal race process is live");
        let verified = runtime
            .verify_stale_process(pid, &identity)
            .expect("verify stale-signal race process")
            .expect("stale-signal race process remains live");

        child.terminate_and_reap();

        runtime
            .signal_verified_stale_process(&verified, Signal::SIGTERM)
            .expect("a fully vanished verified process requires no signal");
    }

    #[test]
    fn linux_proc_stat_parser_handles_spaces_and_parentheses_in_the_command() {
        let stat = "4242 (worker name ) tricky) S 1 4242 1 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 123456 0";

        let parsed =
            ProcessRuntime::parse_linux_proc_stat(stat).expect("parse Linux process stat fixture");

        assert_eq!(parsed, (4242, 4242, 123_456));
    }

    #[test]
    fn owned_signal_refuses_a_group_after_its_leader_cannot_be_revalidated() {
        let runtime = ProcessRuntime::new(PathBuf::from("notify.sock"));
        let mut leader = Command::new("sh");
        leader
            .arg("-c")
            .arg("sleep 30 & exec sleep 0.2")
            .process_group(0);
        let mut leader = leader.spawn().expect("spawn process-group leader");
        let pid = leader.id();
        let mut cleanup = ProcessGroupCleanup::new(pid);
        let identity = runtime
            .capture_process_identity(pid)
            .expect("inspect process-group leader")
            .expect("process-group leader is live");
        leader.wait().expect("reap process-group leader");

        let group = cleanup.group();
        assert!(matches!(
            kill(Pid::from_raw(-group), None),
            Ok(()) | Err(Errno::EPERM)
        ));
        let error = runtime
            .signal_owned_process(pid, &identity, Signal::SIGTERM)
            .expect_err("leaderless group must not be signaled");
        assert!(format!("{error:#}").contains("ownership cannot be revalidated"));
        assert!(matches!(
            kill(Pid::from_raw(-group), None),
            Ok(()) | Err(Errno::EPERM)
        ));

        match kill(Pid::from_raw(-group), Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => panic!("terminate test process group: {error}"),
        }
        for _ in 0..100 {
            if matches!(kill(Pid::from_raw(-group), None), Err(Errno::ESRCH)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        runtime
            .signal_owned_process(pid, &identity, Signal::SIGTERM)
            .expect("a fully exited owned process requires no signal");
        cleanup.disarm();
    }
}
