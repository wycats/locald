//! # locald-server
//!
//! `locald-server` is the core daemon for the `locald` system. It manages the lifecycle of services,
//! handles IPC requests from the CLI, and proxies HTTP/HTTPS traffic.
//!
//! ## Lifecycle
//!
//! 1.  **Startup**: The `run` function initializes the `ProcessManager`, `ProxyManager`, and `NotifyServer`.
//! 2.  **Restoration**: It attempts to restore the state of previously running services.
//! 3.  **Event Loop**: It listens for IPC requests and process exit notifications.
//! 4.  **Shutdown**: It gracefully stops all services and cleans up resources.
//!
//! ## Entry Points
//!
//! *   **Main Loop**: [`run`](crate::run)
//! *   **Configuration**: [`config_loader::ConfigLoader`]
//!
//! ## Warning
//!
//! This crate is primarily intended for internal use by the `locald` binary.
//! The API is not guaranteed to be stable.

// =========================================================================
//  Strict Lints: Safety, Hygiene, and Documentation
// =========================================================================

// 1. Logic & Safety
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/wycats/dotlocal/phase-23-advanced-service-config/locald-docs/public/favicon.svg"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/wycats/dotlocal/phase-23-advanced-service-config/locald-docs/public/favicon.svg"
)]
#![allow(clippy::let_underscore_must_use)] // Don't swallow errors with `let _`
#![warn(clippy::await_holding_lock)] // Prevent Async Deadlocks (Critical)
#![allow(clippy::manual_let_else)] // Enforces clean "Guard Clause" style
#![allow(clippy::unwrap_used)] // Allowed for legacy invariants while we audit and replace unwraps
#![allow(clippy::expect_used)] // Force error propagation
#![warn(clippy::wildcard_enum_match_arm)] // Force explicit enum matching
#![warn(clippy::redundant_pattern_matching)] // Catch redundant matches
#![warn(unreachable_pub)] // Warn if an item is pub but not reachable from crate root
#![warn(clippy::match_wildcard_for_single_variants)] // Catch `_ =>` when only one variant remains
#![warn(clippy::must_use_candidate)] // Suggest `#[must_use]` for pure functions
#![warn(clippy::unused_async)] // Catch async functions that don't await

// 2. Numeric Safety (Critical for PIDs/Ports)
#![warn(clippy::cast_possible_truncation)] // Warn on u64 -> u32 (potential data loss)
#![allow(clippy::cast_possible_wrap)] // Warn on u32 -> i32 (potential overflow)

// 3. Observability
#![allow(clippy::print_stdout)] // Ban println! (Use tracing::info!)
#![allow(clippy::print_stderr)] // Ban eprintln! (Use tracing::error!)

// 4. Import Hygiene
#![warn(clippy::wildcard_imports)] // Ban `use crate::*` (Explicit imports only)
#![allow(clippy::shadow_unrelated)] // Ban accidental variable shadowing

// 5. Documentation
#![allow(missing_docs)] // TODO: Enable later
#![allow(clippy::missing_errors_doc)] // TODO: Enable later

// 6. Other
#![allow(deprecated)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::significant_drop_tightening)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::unnecessary_debug_formatting)]
#![allow(clippy::uninlined_format_args)]

mod agent_context;
#[doc(hidden)]
pub mod api;
#[doc(hidden)]
pub mod assets;
pub(crate) mod catalog_publication;
#[doc(hidden)]
// pub mod cert; // Moved to locald-utils
pub mod config_loader;
#[doc(hidden)]
pub mod container;
// This module is an internal sibling API. Keep its surface explicitly
// crate-scoped instead of exporting generated runtime-file internals.
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod generated_files;
#[doc(hidden)]
pub mod health;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub mod helper_client;
#[doc(hidden)]
pub mod ipc;
pub(crate) mod lifecycle_migration;
pub(crate) mod lifecycle_transaction;
#[doc(hidden)]
pub mod logging;
#[doc(hidden)]
pub mod manager;
#[doc(hidden)]
pub mod plugins;
#[doc(hidden)]
pub mod port_allocator;
#[doc(hidden)]
// pub mod notify; // Moved to locald-utils
#[doc(hidden)]
pub mod proxy;
mod publication;
mod publisher_dispatch;
mod publisher_transport;
#[doc(hidden)]
pub mod runtime;
#[doc(hidden)]
pub mod service;
#[doc(hidden)]
pub mod shim_client;
#[doc(hidden)]
pub mod state;
#[doc(hidden)]
pub mod static_server;
mod tls;
#[doc(hidden)]
pub mod toolbar;

#[cfg(test)]
mod proxy_test;
#[cfg(test)]
mod test_create;

use crate::manager::ProcessManager;
use crate::proxy::ProxyManager;
use anyhow::{Context, Result};
use daemonize::Daemonize;
use nix::unistd::execve;
use std::ffi::{CString, OsStr};
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;

#[derive(Debug, Clone, Copy)]
pub enum ShutdownReason {
    Stop,
    Restart,
}

#[derive(Debug)]
struct CatalogWriterLock {
    file: File,
}

const INHERITED_CATALOG_WRITER_LOCK_FD: &str = "LOCALD_INTERNAL_CATALOG_WRITER_LOCK_FD";

impl CatalogWriterLock {
    fn acquire(inherited_fd: Option<RawFd>) -> Result<Self> {
        let path = locald_core::storage::data_dir().join("catalog.writer.lock");
        inherited_fd.map_or_else(|| Self::acquire_at(&path), |fd| Self::adopt_at(&path, fd))
    }

    #[allow(unsafe_code)]
    fn acquire_at(path: &std::path::Path) -> Result<Self> {
        let parent = path
            .parent()
            .context("catalog writer lock path has no parent directory")?;
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create catalog writer lock directory `{}`",
                parent.display()
            )
        })?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed to open catalog writer lock `{}`", path.display()))?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let source = std::io::Error::last_os_error();
            if source.kind() == std::io::ErrorKind::WouldBlock {
                anyhow::bail!(
                    "another locald daemon owns the project catalog writer lock `{}`",
                    path.display()
                );
            }
            return Err(source).with_context(|| {
                format!("failed to acquire catalog writer lock `{}`", path.display())
            });
        }
        Self::set_close_on_exec(file.as_raw_fd(), true).with_context(|| {
            format!(
                "failed to set close-on-exec for catalog writer lock `{}`",
                path.display()
            )
        })?;
        Ok(Self { file })
    }

    /// Adopt the open file description preserved by a restarting daemon.
    ///
    /// `flock` ownership follows the open file description, so reopening the
    /// path after `execve` would conflict with the daemon's own inherited lock.
    /// Validate and reuse that descriptor, then restore close-on-exec for all
    /// ordinary child processes launched by the new daemon image.
    #[allow(unsafe_code)]
    fn adopt_at(path: &std::path::Path, fd: RawFd) -> Result<Self> {
        if fd < 0 {
            anyhow::bail!("inherited catalog writer lock descriptor must be non-negative");
        }

        let inherited = Self::descriptor_metadata(fd).with_context(|| {
            format!("failed to inspect inherited catalog writer lock descriptor {fd}")
        })?;
        let expected = Self::path_metadata(path).with_context(|| {
            format!(
                "failed to inspect catalog writer lock `{}` for inherited descriptor {fd}",
                path.display()
            )
        })?;
        if inherited.st_dev != expected.st_dev || inherited.st_ino != expected.st_ino {
            anyhow::bail!(
                "inherited catalog writer lock descriptor {fd} does not match `{}`",
                path.display()
            );
        }

        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let source = std::io::Error::last_os_error();
            return Err(source).with_context(|| {
                format!(
                    "failed to adopt inherited catalog writer lock `{}` from descriptor {fd}",
                    path.display()
                )
            });
        }

        // SAFETY: The private restart contract transfers ownership of this
        // validated, open descriptor to the new process image exactly once.
        let file = unsafe { File::from_raw_fd(fd) };
        Self::set_close_on_exec(file.as_raw_fd(), true).with_context(|| {
            format!(
                "failed to restore close-on-exec for catalog writer lock `{}`",
                path.display()
            )
        })?;
        Ok(Self { file })
    }

    #[allow(unsafe_code)]
    fn descriptor_metadata(fd: RawFd) -> Result<libc::stat> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe { libc::fstat(fd, metadata.as_mut_ptr()) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: A successful `fstat` initialized the complete structure.
        Ok(unsafe { metadata.assume_init() })
    }

    #[allow(unsafe_code)]
    fn path_metadata(path: &std::path::Path) -> Result<libc::stat> {
        let path = CString::new(path.as_os_str().as_bytes())
            .context("catalog writer lock path contained an interior NUL")?;
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe { libc::stat(path.as_ptr(), metadata.as_mut_ptr()) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: A successful `stat` initialized the complete structure.
        Ok(unsafe { metadata.assume_init() })
    }

    #[allow(unsafe_code)]
    fn set_close_on_exec(fd: RawFd, enabled: bool) -> Result<()> {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        let flags = if enabled {
            flags | libc::FD_CLOEXEC
        } else {
            flags & !libc::FD_CLOEXEC
        };
        if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn prepare_for_exec(&self) -> Result<RawFd> {
        let fd = self.file.as_raw_fd();
        Self::set_close_on_exec(fd, false)
            .context("failed to preserve catalog writer lock across restart")?;
        Ok(fd)
    }

    fn cancel_exec(&self) -> Result<()> {
        Self::set_close_on_exec(self.file.as_raw_fd(), true)
            .context("failed to restore catalog writer lock close-on-exec state")
    }
}

/// Consume the private descriptor marker before the daemon creates worker
/// threads. The descriptor itself remains open until it is adopted below.
#[allow(unsafe_code)]
fn take_inherited_catalog_writer_lock_fd() -> Result<Option<RawFd>> {
    let Some(value) = std::env::var_os(INHERITED_CATALOG_WRITER_LOCK_FD) else {
        return Ok(None);
    };

    // SAFETY: `run` calls this at daemon entry, before daemonization, logging,
    // or the Tokio runtime creates any worker threads.
    unsafe { std::env::remove_var(INHERITED_CATALOG_WRITER_LOCK_FD) };

    let value = value
        .to_str()
        .context("inherited catalog writer lock descriptor is not valid UTF-8")?;
    let fd = value
        .parse::<RawFd>()
        .context("inherited catalog writer lock descriptor is not an integer")?;
    if fd < 0 {
        anyhow::bail!("inherited catalog writer lock descriptor must be non-negative");
    }
    Ok(Some(fd))
}

fn restart_environment(inherited_fd: RawFd) -> Result<Vec<CString>> {
    let marker_name = OsStr::new(INHERITED_CATALOG_WRITER_LOCK_FD);
    let mut environment = std::env::vars_os()
        .filter(|(name, _)| name != marker_name)
        .map(|(name, value)| {
            let mut entry = name.as_os_str().as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_os_str().as_bytes());
            CString::new(entry).context("environment variable contained an interior NUL")
        })
        .collect::<Result<Vec<_>>>()?;
    environment.push(
        CString::new(format!("{INHERITED_CATALOG_WRITER_LOCK_FD}={inherited_fd}"))
            .context("catalog writer lock environment marker contained an interior NUL")?,
    );
    Ok(environment)
}

#[allow(clippy::disallowed_methods)]
pub fn run(foreground: bool, version: String) -> Result<()> {
    let inherited_catalog_writer_lock_fd = take_inherited_catalog_writer_lock_fd()?;

    // Idempotency check: if already running, exit successfully
    if is_already_running() {
        println!("locald is already running.");
        return Ok(());
    }

    if !foreground {
        let stdout = File::create("/tmp/locald.out")?;
        let stderr = File::create("/tmp/locald.err")?;

        let daemonize = Daemonize::new()
            .pid_file("/tmp/locald.pid")
            .chown_pid_file(true)
            .working_directory("/tmp")
            .stdout(stdout)
            .stderr(stderr);

        match daemonize.start() {
            Ok(()) => println!("locald-server started in background"),
            Err(e) => {
                eprintln!("Error starting daemon: {e}");
                return Err(e.into());
            }
        }
    }

    let catalog_writer_lock = CatalogWriterLock::acquire(inherited_catalog_writer_lock_fd)?;

    // Initialize logging
    let (log_tx, _) = tokio::sync::broadcast::channel(100);

    let broadcast_layer = logging::BroadcastLayer {
        sender: log_tx.clone(),
    };
    let fmt_layer = tracing_subscriber::fmt::layer();

    let _ = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(broadcast_layer)
        .try_init();

    // Install default crypto provider
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Start Tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(version, log_tx, catalog_writer_lock))
}

fn is_already_running() -> bool {
    // Try to connect to the socket to see if a server is listening
    locald_utils::ipc::socket_path().is_ok_and(|path| UnixStream::connect(path).is_ok())
}

#[allow(clippy::similar_names)]
fn validate_port_override_policy(
    sandbox: bool,
    has_http_override: bool,
    has_https_override: bool,
) -> Result<()> {
    if !sandbox && (has_http_override || has_https_override) {
        anyhow::bail!(
            "LOCALD_HTTP_PORT and LOCALD_HTTPS_PORT are available only in explicit sandbox mode; standard mode always uses HTTP on port 80 and trusted HTTPS on port 443"
        );
    }
    Ok(())
}

fn parse_sandbox_port_override(name: &str, value: Option<&std::ffi::OsStr>) -> Result<Option<u16>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{name} must be a UTF-8 integer from 0 through 65535"))?;
    let port = value.parse::<u16>().with_context(|| {
        format!("{name} must be an integer from 0 through 65535; got `{value}`")
    })?;
    Ok(Some(port))
}

const fn publisher_transport_activation_allowed(sandbox: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = sandbox;
        true
    }
    #[cfg(target_os = "linux")]
    {
        sandbox
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = sandbox;
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublisherWakePolicy {
    System,
    #[cfg(target_os = "linux")]
    ExplicitSandboxNoHostSuspendGuarantee,
}

const fn publisher_wake_policy(
    sandbox: bool,
    explicit_no_host_suspend_guarantee: bool,
) -> PublisherWakePolicy {
    #[cfg(target_os = "linux")]
    {
        if sandbox && explicit_no_host_suspend_guarantee {
            PublisherWakePolicy::ExplicitSandboxNoHostSuspendGuarantee
        } else {
            PublisherWakePolicy::System
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (sandbox, explicit_no_host_suspend_guarantee);
        PublisherWakePolicy::System
    }
}

fn validate_explicit_no_host_suspend_guarantee(
    sandbox: bool,
    value: Option<&std::ffi::OsStr>,
) -> Result<bool> {
    let Some(value) = value else {
        return Ok(false);
    };
    if value != std::ffi::OsStr::new("1") {
        anyhow::bail!("LOCALD_SANDBOX_NO_HOST_SUSPEND must be exactly `1` when set");
    }
    if !sandbox {
        anyhow::bail!(
            "LOCALD_SANDBOX_NO_HOST_SUSPEND requires an effective explicit sandbox; use --sandbox together with --sandbox-no-host-suspend"
        );
    }
    Ok(true)
}

#[cfg(target_os = "macos")]
fn validate_macos_standard_preflight(ca_trusted: bool, helper_probe: Result<()>) -> Result<()> {
    if !ca_trusted {
        anyhow::bail!(
            "locald's Root CA is missing or not trusted by macOS. Run `sudo locald admin setup` to repair the installation."
        );
    }
    helper_probe.context(
        "standard-mode privileged-helper preflight failed; run `sudo locald admin setup` to repair the installation",
    )
}

async fn load_attachment_store_for_lifecycle_recovery(
    store: &mut locald_core::attachments::AttachmentStore,
    preflight: &lifecycle_transaction::LifecycleRecoveryPreflight,
) -> Result<()> {
    if let Some(images) = preflight.pending_legacy_attachment_images() {
        let exact_error = match store
            .load_exact_transaction_image(images.base(), images.target())
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };

        let compatibility_path = store.storage_path();
        let legacy_input = if path_entry_exists(compatibility_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to inspect lifecycle compatibility state `{}` after exact v2 parsing failed",
                    compatibility_path.display()
                )
            })?
        {
            Some(tokio::fs::read(compatibility_path).await.with_context(|| {
                format!(
                    "Failed to read existing lifecycle compatibility state `{}` after exact v2 parsing failed: {exact_error:#}",
                    compatibility_path.display()
                )
            })?)
        } else {
            None
        };
        let declares_exact_v2 = legacy_input.as_ref().is_some_and(|content| {
            serde_json::from_slice::<serde_json::Value>(content).is_ok_and(|value| {
                value
                    .as_object()
                    .is_some_and(|object| object.contains_key("instance_owners"))
            })
        });
        if declares_exact_v2 {
            return Err(exact_error).with_context(|| {
                format!(
                    "Lifecycle compatibility state `{}` declares exact v2 shape and cannot fall back to legacy parsing",
                    compatibility_path.display()
                )
            });
        }

        let mut legacy =
            locald_core::attachments::AttachmentStore::new(store.storage_path().to_path_buf());
        legacy.load().await.with_context(|| {
            format!(
                "Failed to parse legacy lifecycle compatibility state `{}` after exact v2 parsing failed: {exact_error:#}",
                store.storage_path().display()
            )
        })?;
        match legacy_input {
            Some(expected) => {
                let current = tokio::fs::read(store.storage_path()).await.with_context(|| {
                    format!(
                        "Failed to re-read lifecycle compatibility state `{}` after legacy parsing",
                        store.storage_path().display()
                    )
                })?;
                anyhow::ensure!(
                    current == expected,
                    "lifecycle compatibility state `{}` changed while legacy recovery was loading it",
                    store.storage_path().display()
                );
            }
            None => {
                anyhow::ensure!(
                    !path_entry_exists(store.storage_path()).await.with_context(|| {
                        format!(
                            "Failed to re-inspect absent lifecycle compatibility state `{}` after legacy parsing",
                            store.storage_path().display()
                        )
                    })?,
                    "lifecycle compatibility state `{}` appeared while legacy recovery was loading it",
                    store.storage_path().display()
                );
            }
        }
        anyhow::ensure!(
            legacy.snapshot() == *images.base(),
            "compatibility state `{}` is not exact v2 and its permissive legacy projection does not match the journal base; exact v2 parsing failed: {exact_error:#}",
            store.storage_path().display()
        );
        *store = legacy;
        return Ok(());
    }

    if preflight.requires_exact_attachment_authority() {
        store.load_exact().await
    } else {
        store.load().await
    }
}

#[allow(clippy::similar_names)]
async fn async_main(
    version: String,
    log_tx: tokio::sync::broadcast::Sender<locald_core::ipc::LogEntry>,
    catalog_writer_lock: CatalogWriterLock,
) -> Result<()> {
    let executable = std::env::current_exe().ok();
    info!(
        "locald-server starting... (version: {}, pid: {}, executable: {})",
        version,
        std::process::id(),
        executable
            .as_ref()
            .map_or_else(|| "<unknown>".into(), |path| path.display().to_string())
    );

    // Load configuration
    let config = crate::config_loader::ConfigLoader::load()
        .await
        .map(|loader| loader.global)
        .unwrap_or_else(|e| {
            warn!("Failed to load global config: {e}. Using defaults.");
            locald_core::config::GlobalConfig::default()
        });

    let http_port_override = std::env::var_os("LOCALD_HTTP_PORT");
    let https_port_override = std::env::var_os("LOCALD_HTTPS_PORT");
    let sandbox = config.server.is_sandbox();
    let explicit_no_host_suspend_guarantee = validate_explicit_no_host_suspend_guarantee(
        sandbox,
        std::env::var_os("LOCALD_SANDBOX_NO_HOST_SUSPEND").as_deref(),
    )?;
    validate_port_override_policy(
        sandbox,
        http_port_override.is_some(),
        https_port_override.is_some(),
    )?;
    let http_port_override = sandbox
        .then(|| parse_sandbox_port_override("LOCALD_HTTP_PORT", http_port_override.as_deref()))
        .transpose()?
        .flatten();
    let https_port_override = sandbox
        .then(|| parse_sandbox_port_override("LOCALD_HTTPS_PORT", https_port_override.as_deref()))
        .transpose()?
        .flatten();

    #[cfg(target_os = "macos")]
    if !config.server.is_sandbox() {
        let helper_probe = helper_client::probe_helper().await;
        validate_macos_standard_preflight(locald_utils::cert::is_ca_trusted(), helper_probe)?;
    }

    // The notify socket must be sandbox-aware (tests and parallel sandboxes), otherwise
    // multiple daemon instances will contend for the same fixed path.
    let notify_path = locald_utils::ipc::socket_path()
        .map(|p| p.with_file_name("locald-notify.sock"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/locald-notify.sock"));

    let state_manager = std::sync::Arc::new(
        crate::state::StateManager::new().context("Failed to initialize state manager")?,
    );

    let catalog_path = locald_core::registry::Registry::path();
    let catalog_publication_journal =
        catalog_publication::CatalogPublicationJournal::for_catalog_path(&catalog_path)
            .context("Failed to locate catalog publication recovery authority")?;
    let catalog_publication_preflight = catalog_publication_journal
        .load(&catalog_path)
        .await
        .context("Failed to preflight catalog publication recovery authority")?;
    let lifecycle_preflight =
        lifecycle_transaction::LifecycleJournal::at(&locald_core::storage::data_dir())
            .preflight()
            .await
            .context("Failed to preflight lifecycle recovery authority")?;
    let catalog_exists = path_entry_exists(&catalog_path).await.with_context(|| {
        format!(
            "Failed to inspect project identity catalog `{}`",
            catalog_path.display()
        )
    })?;
    let startup_authority = select_startup_catalog_authority(
        &catalog_path,
        catalog_exists,
        catalog_publication_preflight.as_ref(),
        &lifecycle_preflight,
    )?;
    let registry = match startup_authority.recovery_catalog {
        Some(catalog) => catalog,
        None => locald_core::registry::Registry::load_for_lifecycle_recovery(
            startup_authority.allow_legacy_bootstrap,
        )
        .await
        .context("Failed to initialize project identity catalog")?,
    };
    let registry = std::sync::Arc::new(tokio::sync::Mutex::new(registry));

    let mut attachment_store = locald_core::attachments::AttachmentStore::new(
        locald_core::attachments::AttachmentStore::path(),
    );
    load_attachment_store_for_lifecycle_recovery(&mut attachment_store, &lifecycle_preflight)
        .await
        .context("Failed to initialize lifecycle compatibility state")?;
    let attachments = std::sync::Arc::new(tokio::sync::Mutex::new(attachment_store));

    let mut manager = ProcessManager::new(
        notify_path.clone(),
        state_manager,
        registry,
        attachments,
        Some(log_tx),
    )?;
    if config.server.is_sandbox() {
        manager.use_sandbox_host_set_writer();
    }
    manager
        .recover_catalog_publication_state()
        .await
        .context("Failed to recover catalog, domain, and hosts publication")?;
    manager
        .recover_and_migrate_lifecycle_state()
        .await
        .context("Failed to recover or migrate lifecycle authority")?;
    manager
        .migrate_catalog_schema_if_needed()
        .await
        .context("Failed to migrate the project catalog schema")?;
    manager
        .hydrate_publisher_availability_state()
        .await
        .context("Failed to hydrate published endpoint availability policy")?;
    manager.spawn_metrics_collector();

    // Initialize ContainerManager
    let data_dir = directories::ProjectDirs::from("com", "locald", "locald")
        .map(|dirs| dirs.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".locald"));
    let container_manager = std::sync::Arc::new(crate::container::ContainerManager::new(&data_dir));

    // Notify Server (Linux only - uses Unix datagram sockets with peer credentials)
    #[cfg(target_os = "linux")]
    {
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(100);
        // We need to handle potential failure binding the socket
        let notify_server = match locald_utils::notify::NotifyServer::new(notify_path).await {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to bind notify socket: {e}");
                return Err(e);
            }
        };

        tokio::spawn(async move {
            notify_server.run(notify_tx).await;
        });

        let manager_clone = manager.clone();
        tokio::spawn(async move {
            while let Some((pid, _msg)) = notify_rx.recv().await {
                manager_clone.handle_notify(pid).await;
            }
        });
    }
    #[cfg(not(target_os = "linux"))]
    {
        // On non-Linux platforms, the notify server is not available.
        // Services will still work but won't receive systemd-style notifications.
        tracing::debug!("Notify server not available on this platform");
        let _ = notify_path; // suppress unused warning
    }

    // Reconcile stale runtime evidence before accepting lifecycle requests that
    // could launch new processes. Availability-policy convergence happens in
    // the background once IPC is online.
    let restore_plan = manager
        .reconcile_stale_runtime_state()
        .await
        .context("Failed to reconcile daemon runtime state")?;
    // Renew still-live process-owned compatibility demands before the first
    // availability sweep. Revalidation is passive and therefore preserves a
    // user pause while preventing daemon downtime from expiring a live owner.
    manager
        .reconcile_legacy_attachment_owners()
        .await
        .context("Failed to reconcile legacy lifecycle owners")?;

    // Initialize CertManager
    let certificate_domains = manager.domain_index();
    let cert_manager = match locald_utils::cert::CertManager::new(move |server_name| {
        tls::owned_server_name(&certificate_domains, server_name)
    })
    .await
    {
        Ok(cm) => Some(std::sync::Arc::new(cm)),
        Err(e) => {
            if !config.server.is_sandbox() {
                return Err(anyhow::anyhow!(
                    "Failed to initialize HTTPS certificates: {e}\n\
                     Run `sudo locald admin setup` to configure HTTPS trust."
                ));
            }
            warn!("Failed to initialize CertManager: {e}. HTTPS will be disabled.");
            None
        }
    };
    let https_enabled = cert_manager.is_some();

    // Run Proxy server
    let api_router = crate::api::router(manager.clone());
    let proxy = std::sync::Arc::new(ProxyManager::new(
        std::sync::Arc::new(manager.clone()),
        api_router,
        cert_manager,
    ));

    // Bind HTTP
    let listener_http = if let Some(port) = http_port_override {
        info!("Binding HTTP to configured port: {}", port);
        match proxy.bind_http(port).await {
            Ok(l) => Some(l),
            Err(e) => {
                error!("Failed to bind configured port {}: {}", port, e);
                None
            }
        }
    } else if config.server.is_sandbox() {
        // Sandbox mode: use high ports, best-effort
        match proxy.bind_http(8080).await {
            Ok(l) => Some(l),
            Err(e) => {
                warn!("Failed to bind port 8080: {e}. Trying 8081...");
                match proxy.bind_http(8081).await {
                    Ok(l) => Some(l),
                    Err(e) => {
                        error!("Failed to bind port 8081: {e}. Proxy disabled.");
                        None
                    }
                }
            }
        }
    } else {
        // Standard mode: bind privileged port 80.
        // On macOS, the helper binds as root and passes the FD via XPC.
        // On Linux, the shim binds as root and passes the FD via SCM_RIGHTS.
        match proxy.bind_http(80).await {
            Ok(l) => Some(l),
            Err(e) => {
                error!(
                    "Failed to bind port 80: {e}.\n\
                     Run `sudo locald admin setup` to install the privileged helper."
                );
                return Err(e);
            }
        }
    };

    let _has_http = listener_http.is_some();
    let advertised_http_port = listener_http.as_ref().map(|listener| {
        listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or(8080)
    });

    // A bound socket is advertised only when TLS can actually serve it. In
    // sandbox mode certificate initialization may be unavailable; leave HTTPS
    // absent so readiness-aware callers receive a timeout instead of a dead URL.
    let listener_https: Option<std::net::TcpListener> = if !https_enabled {
        None
    } else if let Some(port) = https_port_override {
        info!("Binding HTTPS to configured port: {}", port);
        match proxy.bind_https(port).await {
            Ok(l) => Some(l),
            Err(e) => {
                error!("Failed to bind configured port {}: {}", port, e);
                None
            }
        }
    } else if config.server.is_sandbox() {
        // Sandbox mode: use high ports, best-effort
        match proxy.bind_https(8443).await {
            Ok(l) => Some(l),
            Err(e) => {
                error!("Failed to bind port 8443: {e}. HTTPS disabled.");
                None
            }
        }
    } else {
        // Standard mode: bind privileged port 443.
        match proxy.bind_https(443).await {
            Ok(l) => Some(l),
            Err(e) => {
                error!(
                    "Failed to bind port 443: {e}.\n\
                     Run `sudo locald admin setup` to install the privileged helper."
                );
                return Err(e);
            }
        }
    };

    // A durable published origin is useful only when this daemon can serve the
    // exact HTTPS listener recorded at admission time.
    let advertised_https_port = listener_https.as_ref().map(|listener| {
        listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or(8443)
    });
    manager
        .validate_published_origins_for_https_listener(advertised_https_port)
        .await?;

    // Publish listener identity only after every durable published origin has
    // been validated against the complete bound listener set.
    if let Some(port) = advertised_http_port {
        manager.set_http_port(Some(port)).await;
    }
    if let Some(port) = advertised_https_port {
        manager.set_https_port(Some(port)).await;
    }

    if let Some(l) = listener_https {
        let proxy_clone = proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy_clone.serve_https(l).await {
                error!("HTTPS proxy server error: {e}");
            }
        });
    }

    if let Some(l) = listener_http {
        let proxy_clone = proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy_clone.serve_http(l).await {
                error!("HTTP proxy server error: {e}");
            }
        });
    }

    // The dedicated publisher socket becomes discoverable only after catalog
    // recovery, durable origin validation, and all front-door listeners have
    // completed. A bind failure leaves ordinary discovery explicitly inactive
    // so installed publishers fail visibly rather than deriving a socket.
    let publisher_socket_path = locald_core::storage::data_dir()
        .join(locald_publisher_protocol::PUBLISHER_SOCKET_RELATIVE_PATH);
    let publisher_socket = publisher_socket_path
        .to_str()
        .and_then(|path| locald_publisher_protocol::AbsolutePath::parse(path).ok());
    let mut publisher_server = None;
    let publisher_discovery = if !publisher_transport_activation_allowed(sandbox) {
        info!(
            "publisher transport is inactive until this platform has an atomic installation-record lifecycle"
        );
        ipc::PublisherTransportDiscovery::Inactive
    } else if let Some(publisher_socket) = publisher_socket {
        let publisher_authority = manager.publisher_authority();
        let wake_activation =
            match publisher_wake_policy(sandbox, explicit_no_host_suspend_guarantee) {
                PublisherWakePolicy::System => publisher_authority.activate_system_wake_monitor(),
                #[cfg(target_os = "linux")]
                PublisherWakePolicy::ExplicitSandboxNoHostSuspendGuarantee => {
                    publisher_authority.activate_linux_sandbox_explicit_no_suspend_wake_monitor()
                }
            };
        match wake_activation {
            Ok(()) => {
                let front_door_ports = advertised_http_port
                    .into_iter()
                    .chain(advertised_https_port);
                let config = publisher_transport::PublisherSocketConfig::for_current_user(
                    publisher_socket_path.clone(),
                    front_door_ports,
                    publisher_transport::publisher_spawn_barrier(),
                );
                let dispatcher = std::sync::Arc::new(publisher_dispatch::PublisherDispatcher::new(
                    manager.clone(),
                ));
                match publisher_transport::PublisherSocketServer::bind(config, dispatcher).await {
                    Ok(server) => {
                        let protocol_info =
                            publisher_authority.protocol_info(publisher_socket).await;
                        info!(
                            path = %server.socket_path().display(),
                            "publisher transport is active"
                        );
                        publisher_server = Some(server);
                        ipc::PublisherTransportDiscovery::Active(protocol_info)
                    }
                    Err(error) => {
                        error!(
                            path = %publisher_socket_path.display(),
                            error = %error,
                            "publisher transport is unavailable"
                        );
                        ipc::PublisherTransportDiscovery::Inactive
                    }
                }
            }
            Err(error) => {
                error!(
                    error = %error,
                    "publisher transport wake safety is unavailable"
                );
                ipc::PublisherTransportDiscovery::Inactive
            }
        }
    } else {
        error!(
            path = %publisher_socket_path.display(),
            "publisher transport path is not an absolute UTF-8 path"
        );
        ipc::PublisherTransportDiscovery::Inactive
    };

    let mut publisher_changes = manager.publisher_authority().subscribe_changes();
    let publisher_event_sender = manager.event_sender.clone();
    let publisher_projection_handle = tokio::spawn(async move {
        while publisher_changes.changed().await.is_ok() {
            let _ = publisher_event_sender.send(locald_core::ipc::Event::ServiceListChanged);
        }
    });

    // Only after catalogued origins agree with the bound listeners may any
    // lifecycle request, policy restoration, or background convergence run.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<ShutdownReason>(1);
    let manager_clone = manager.clone();
    let container_manager_clone = container_manager.clone();
    let version_clone = version.clone();
    let shutdown_tx_ipc = shutdown_tx.clone();
    let mut ipc_handle = tokio::spawn(async move {
        ipc::run_ipc_server(
            manager_clone,
            container_manager_clone,
            shutdown_tx_ipc,
            version_clone,
            publisher_discovery,
        )
        .await
    });

    let restore_manager = manager.clone();
    tokio::spawn(async move {
        restore_manager
            .restore_policy_owned_projects(restore_plan)
            .await;
    });

    let manager_reaper = manager.clone();
    tokio::spawn(async move {
        loop {
            manager_reaper.wait_for_availability_maintenance().await;
            if manager_reaper.is_shutting_down() {
                break;
            }
            // The wake may be an advertised retry deadline. Dispatch only due
            // retries before orphan reaping, which can wait behind a
            // foreground attachment transition for an unbounded interval.
            // The global lease sweep follows compatibility-owner revalidation.
            let retry_dispatch = manager_reaper
                .converge_due_project_availability_retries()
                .await;
            if manager_reaper.is_shutting_down() {
                break;
            }
            manager_reaper.reap_and_stop_orphans().await;
            if manager_reaper.is_shutting_down() {
                break;
            }
            // Keep retry claims withheld through the global sweep. This keeps
            // later queued retries from becoming due again while an earlier
            // ordinary lifecycle convergence is still running.
            manager_reaper
                .converge_all_project_availability_after_retry_dispatch(retry_dispatch)
                .await;
        }
    });

    let container_manager_clone = container_manager.clone();
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        watch_for_upgrade(container_manager_clone, shutdown_tx_clone).await;
    });

    let (reason, ipc_joined) = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down");
            (ShutdownReason::Stop, false)
        },
        r = shutdown_rx.recv() => {
            info!("Received shutdown signal");
            (r.unwrap_or(ShutdownReason::Stop), false)
        },
        result = &mut ipc_handle => {
            match result {
                Ok(Err(e)) => error!("IPC server failed: {e}"),
                Ok(Ok(())) => info!("IPC server exited normally"),
                Err(e) => error!("IPC server task panicked: {e}"),
            }
            (ShutdownReason::Stop, true)
        }
    };

    if !ipc_joined {
        ipc_handle.abort();
        let _ = ipc_handle.await;
    }
    // Retire publisher authority before waiting for accepted publisher
    // connections. This wakes readiness/preparation waiters and prevents a
    // request already admitted by the socket from committing after shutdown.
    manager.publisher_authority().shutdown().await;
    if let Some(server) = publisher_server.take()
        && let Err(error) = server.shutdown().await
    {
        warn!(error = %error, "failed to stop publisher transport cleanly");
    }
    publisher_projection_handle.abort();
    let _ = publisher_projection_handle.await;

    info!("Stopping all services...");
    if let Err(e) = manager.shutdown().await {
        warn!("Error shutting down services: {e}");
    }

    if let Ok(path) = locald_utils::ipc::socket_path() {
        let _ = tokio::fs::remove_file(path).await;
    }
    let _ = tokio::fs::remove_file("/tmp/locald.pid").await;

    if matches!(reason, ShutdownReason::Restart) {
        info!("Restarting process...");
        let exe_path = std::env::current_exe()?;
        let exe = CString::new(exe_path.as_os_str().as_bytes())
            .context("Executable path contained an interior NUL")?;

        let mut argv = Vec::new();
        argv.push(exe.clone());
        for arg in std::env::args().skip(1) {
            argv.push(CString::new(arg).context("Argument contained an interior NUL")?);
        }

        let inherited_fd = catalog_writer_lock.file.as_raw_fd();
        let environment = restart_environment(inherited_fd)?;
        let spawn_permit = locald_utils::process_spawn::ProcessSpawnBarrier::global().enter_spawn();
        catalog_writer_lock.prepare_for_exec()?;
        let err = execve(&exe, &argv, &environment)
            .err()
            .context("execve unexpectedly returned Ok")?;
        if let Err(restore_error) = catalog_writer_lock.cancel_exec() {
            error!(
                "Failed to restore catalog writer lock descriptor after exec error: {restore_error}"
            );
        }
        drop(spawn_permit);
        error!("Failed to exec: {}", err);
        return Err(err.into());
    }

    info!("locald-server stopped");
    Ok(())
}

async fn path_entry_exists(path: &std::path::Path) -> std::io::Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(source),
    }
}

#[derive(Debug)]
struct StartupCatalogAuthority {
    allow_legacy_bootstrap: bool,
    recovery_catalog: Option<locald_core::ProjectCatalog>,
}

fn select_startup_catalog_authority(
    catalog_path: &std::path::Path,
    catalog_exists: bool,
    publication: Option<&catalog_publication::CatalogPublicationTransaction>,
    lifecycle: &lifecycle_transaction::LifecycleRecoveryPreflight,
) -> Result<StartupCatalogAuthority> {
    if let (Some(publication), Some(lifecycle_txn)) = (publication, lifecycle.transaction()) {
        anyhow::ensure!(
            lifecycle_txn.phase() == lifecycle_transaction::LifecycleTransactionPhase::Prepared,
            "catalog publication generation {} coexists with lifecycle transaction {} after its prepared phase",
            publication.target_generation(),
            lifecycle_txn.id()
        );
        let images = lifecycle_txn.catalog().with_context(|| {
            format!(
                "catalog publication generation {} coexists with lifecycle transaction {} without catalog images",
                publication.target_generation(),
                lifecycle_txn.id()
            )
        })?;
        let mut lifecycle_base = images.base().clone();
        let mut lifecycle_target = images.target().clone();
        lifecycle_base.set_storage_path(catalog_path.to_path_buf());
        lifecycle_target.set_storage_path(catalog_path.to_path_buf());
        anyhow::ensure!(
            lifecycle_base == *publication.catalog_base()
                && lifecycle_target == *publication.catalog_target(),
            "catalog publication generation {} does not match lifecycle transaction {} catalog images",
            publication.target_generation(),
            lifecycle_txn.id()
        );
    }

    let recovery_catalog = if catalog_exists {
        None
    } else if let Some(publication) = publication {
        Some(
            publication
                .catalog_for_missing_storage()
                .context("Failed to recover the missing catalog from publication authority")?,
        )
    } else {
        lifecycle.prepared_legacy_catalog_base(catalog_path)
    };

    Ok(StartupCatalogAuthority {
        allow_legacy_bootstrap: !lifecycle.has_v2_authority() && publication.is_none(),
        recovery_catalog,
    })
}

async fn watch_for_upgrade(
    container_manager: std::sync::Arc<crate::container::ContainerManager>,
    shutdown_tx: tokio::sync::mpsc::Sender<ShutdownReason>,
) {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to get current exe path: {}", e);
            return;
        }
    };

    let initial_mtime = match std::fs::metadata(&exe_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to get exe metadata: {}", e);
            return;
        }
    };

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));

    loop {
        interval.tick().await;

        let current_mtime = match std::fs::metadata(&exe_path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };

        if current_mtime != initial_mtime {
            info!("Detected binary upgrade.");

            let active = container_manager.active_count();
            if active == 0 {
                info!("No active ephemeral tasks. Initiating restart...");
                let _ = shutdown_tx.send(ShutdownReason::Restart).await;
                break;
            }
            info!("Deferring restart: {} active ephemeral tasks.", active);
        }
    }
}

#[cfg(test)]
mod privileged_startup_tests {
    #[cfg(target_os = "macos")]
    use super::validate_macos_standard_preflight;
    use super::{
        PublisherWakePolicy, parse_sandbox_port_override, publisher_transport_activation_allowed,
        publisher_wake_policy, validate_explicit_no_host_suspend_guarantee,
        validate_port_override_policy,
    };
    use std::ffi::OsStr;

    #[test]
    fn standard_mode_rejects_all_port_overrides() {
        let error = validate_port_override_policy(false, true, false).unwrap_err();
        assert!(error.to_string().contains("HTTP on port 80"));
        assert!(error.to_string().contains("trusted HTTPS on port 443"));
        assert!(validate_port_override_policy(false, false, true).is_err());
        assert!(validate_port_override_policy(false, true, true).is_err());
        validate_port_override_policy(false, false, false)
            .expect("standard mode without overrides is valid");
    }

    #[test]
    fn sandbox_mode_retains_ephemeral_port_overrides() {
        validate_port_override_policy(true, true, true)
            .expect("sandbox mode may configure both proxy ports");
    }

    #[test]
    fn sandbox_port_overrides_are_strictly_parsed() {
        assert_eq!(
            parse_sandbox_port_override("LOCALD_HTTP_PORT", None).unwrap(),
            None
        );
        assert_eq!(
            parse_sandbox_port_override("LOCALD_HTTP_PORT", Some(OsStr::new("0"))).unwrap(),
            Some(0)
        );
        assert!(
            parse_sandbox_port_override("LOCALD_HTTP_PORT", Some(OsStr::new("not-a-port")))
                .unwrap_err()
                .to_string()
                .contains("LOCALD_HTTP_PORT")
        );
        assert!(
            parse_sandbox_port_override("LOCALD_HTTPS_PORT", Some(OsStr::new("65536")))
                .unwrap_err()
                .to_string()
                .contains("LOCALD_HTTPS_PORT")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn publisher_transport_is_available_in_standard_and_sandbox_modes_on_macos() {
        assert!(publisher_transport_activation_allowed(false));
        assert!(publisher_transport_activation_allowed(true));
        assert_eq!(
            publisher_wake_policy(false, false),
            PublisherWakePolicy::System
        );
        assert_eq!(
            publisher_wake_policy(true, false),
            PublisherWakePolicy::System
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn publisher_transport_is_available_only_in_explicit_sandbox_mode_on_linux() {
        assert!(!publisher_transport_activation_allowed(false));
        assert!(publisher_transport_activation_allowed(true));
        assert_eq!(
            publisher_wake_policy(false, false),
            PublisherWakePolicy::System
        );
        assert_eq!(
            publisher_wake_policy(true, false),
            PublisherWakePolicy::System
        );
        assert_eq!(
            publisher_wake_policy(false, true),
            PublisherWakePolicy::System
        );
        assert_eq!(
            publisher_wake_policy(true, true),
            PublisherWakePolicy::ExplicitSandboxNoHostSuspendGuarantee
        );
    }

    #[test]
    fn no_host_suspend_marker_requires_the_exact_authenticated_context() {
        assert!(!validate_explicit_no_host_suspend_guarantee(true, None).unwrap());
        assert!(validate_explicit_no_host_suspend_guarantee(true, Some(OsStr::new(""))).is_err());
        assert!(
            validate_explicit_no_host_suspend_guarantee(true, Some(OsStr::new("true"))).is_err()
        );
        assert!(validate_explicit_no_host_suspend_guarantee(false, Some(OsStr::new("1"))).is_err());
        assert!(validate_explicit_no_host_suspend_guarantee(true, Some(OsStr::new("1"))).unwrap());
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn publisher_transport_stays_inactive_on_unsupported_platforms() {
        assert!(!publisher_transport_activation_allowed(false));
        assert!(!publisher_transport_activation_allowed(true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn standard_mode_fails_closed_on_trust_or_helper_preflight() {
        assert!(validate_macos_standard_preflight(false, Ok(())).is_err());
        assert!(
            validate_macos_standard_preflight(true, Err(anyhow::anyhow!("unauthorized")))
                .unwrap_err()
                .to_string()
                .contains("sudo locald admin setup")
        );
        validate_macos_standard_preflight(true, Ok(())).expect("complete installation is ready");
    }
}

#[cfg(test)]
mod catalog_writer_lock_tests {
    use super::{
        CatalogWriterLock, INHERITED_CATALOG_WRITER_LOCK_FD, path_entry_exists, restart_environment,
    };
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt as _;

    #[allow(unsafe_code)]
    fn close_on_exec(fd: std::os::fd::RawFd) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(flags, -1, "read descriptor flags");
        flags & libc::FD_CLOEXEC != 0
    }

    #[test]
    fn catalog_writer_lock_has_one_live_owner() {
        let directory = tempfile::tempdir().expect("create lock test directory");
        let path = directory.path().join("catalog.writer.lock");
        let first = CatalogWriterLock::acquire_at(&path).expect("acquire first writer lock");
        assert_eq!(
            first.file.metadata().expect("inspect lock").mode() & 0o7777,
            0o600
        );

        let error =
            CatalogWriterLock::acquire_at(&path).expect_err("second live writer must be rejected");
        assert!(error.to_string().contains("another locald daemon"));

        drop(first);
        CatalogWriterLock::acquire_at(&path).expect("reacquire released writer lock");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_treats_a_dangling_catalog_symlink_as_existing_state() {
        let directory = tempfile::tempdir().expect("create catalog path fixture");
        let catalog = directory.path().join("catalog.json");
        std::os::unix::fs::symlink(directory.path().join("missing-target"), &catalog)
            .expect("create dangling catalog symlink");

        assert!(
            path_entry_exists(&catalog)
                .await
                .expect("inspect dangling catalog symlink")
        );
        assert!(
            !path_entry_exists(&directory.path().join("absent.json"))
                .await
                .expect("inspect absent catalog path")
        );
    }

    #[test]
    fn restart_handoff_adopts_the_same_lock_and_preserves_exclusion() {
        let directory = tempfile::tempdir().expect("create lock test directory");
        let path = directory.path().join("catalog.writer.lock");
        let first = CatalogWriterLock::acquire_at(&path).expect("acquire writer lock");
        let fd = first.file.as_raw_fd();
        assert!(close_on_exec(fd));

        assert_eq!(first.prepare_for_exec().expect("prepare lock handoff"), fd);
        assert!(!close_on_exec(fd));

        // A successful exec transfers ownership of the still-open descriptor
        // to the replacement image, so model that ownership transfer here.
        std::mem::forget(first);
        let adopted = CatalogWriterLock::adopt_at(&path, fd).expect("adopt inherited lock");
        assert!(close_on_exec(adopted.file.as_raw_fd()));

        let error = CatalogWriterLock::acquire_at(&path)
            .expect_err("adopted writer lock must exclude another owner");
        assert!(error.to_string().contains("another locald daemon"));

        drop(adopted);
        CatalogWriterLock::acquire_at(&path).expect("reacquire released adopted lock");
    }

    #[test]
    #[allow(unsafe_code)]
    fn restart_handoff_rejects_a_descriptor_for_another_file() {
        let directory = tempfile::tempdir().expect("create lock test directory");
        let path = directory.path().join("catalog.writer.lock");
        let other_path = directory.path().join("other.lock");
        std::fs::write(&other_path, []).expect("create other lock file");

        let first = CatalogWriterLock::acquire_at(&path).expect("acquire writer lock");
        let fd = first.prepare_for_exec().expect("prepare lock handoff");
        std::mem::forget(first);

        let error = CatalogWriterLock::adopt_at(&other_path, fd)
            .expect_err("descriptor identity mismatch must fail");
        assert!(error.to_string().contains("does not match"));
        CatalogWriterLock::acquire_at(&path)
            .expect_err("rejected descriptor continues to own the original lock");

        assert_eq!(unsafe { libc::close(fd) }, 0, "close rejected descriptor");
        CatalogWriterLock::acquire_at(&path).expect("reacquire lock after descriptor closes");
    }

    #[test]
    fn restart_environment_contains_one_private_lock_marker() {
        let environment = restart_environment(42).expect("build restart environment");
        let prefix = format!("{INHERITED_CATALOG_WRITER_LOCK_FD}=");
        let markers: Vec<_> = environment
            .iter()
            .filter_map(|entry| entry.to_str().ok())
            .filter(|entry| entry.starts_with(&prefix))
            .collect();

        assert_eq!(markers, vec![format!("{prefix}42")]);
    }
}

#[cfg(test)]
mod lifecycle_startup_tests {
    use super::{load_attachment_store_for_lifecycle_recovery, select_startup_catalog_authority};
    use crate::catalog_publication::{CatalogPublicationTransaction, host_set_for_catalog};
    use crate::lifecycle_transaction::{
        AttachmentTransactionImages, CatalogTransactionImages, LifecycleJournal,
        LifecycleTransaction, LifecycleTransactionKind, LifecycleTransactionPhase,
    };
    use locald_core::ProjectCatalog;
    use locald_core::attachments::{
        Attachment, AttachmentSource, AttachmentStore, AttachmentStoreSnapshot,
    };
    use std::time::SystemTime;

    async fn catalog_publication_images(
        directory: &tempfile::TempDir,
    ) -> (ProjectCatalog, ProjectCatalog) {
        let catalog_path = directory.path().join("catalog.json");
        let catalog_base = ProjectCatalog::with_path(catalog_path);
        let project_path = directory.path().join("project");
        std::fs::create_dir(&project_path).expect("create startup publication project");
        let mut catalog_target = catalog_base.clone();
        catalog_target
            .register_project(
                ProjectCatalog::discover(project_path)
                    .await
                    .expect("discover startup publication project"),
                Some("startup-publication".to_owned()),
            )
            .expect("register startup publication project");
        (catalog_base, catalog_target)
    }

    fn publication_transaction(
        catalog_base: ProjectCatalog,
        catalog_target: ProjectCatalog,
    ) -> CatalogPublicationTransaction {
        let previous_hosts =
            host_set_for_catalog(&catalog_base).expect("derive startup previous hosts");
        let candidate_hosts =
            host_set_for_catalog(&catalog_target).expect("derive startup candidate hosts");
        CatalogPublicationTransaction::new(
            catalog_base,
            catalog_target,
            &previous_hosts,
            &candidate_hosts,
        )
        .expect("build startup publication transaction")
    }

    fn lifecycle_transaction_with_catalog(
        catalog_base: ProjectCatalog,
        catalog_target: ProjectCatalog,
    ) -> LifecycleTransaction {
        LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            SystemTime::now(),
            Some(
                CatalogTransactionImages::new(catalog_base, catalog_target)
                    .expect("prepare startup lifecycle catalog images"),
            ),
            Vec::new(),
            AttachmentTransactionImages::new(
                AttachmentStoreSnapshot::default(),
                AttachmentStoreSnapshot::default(),
            ),
        )
        .expect("build startup lifecycle transaction")
    }

    #[tokio::test]
    async fn publication_authority_seeds_a_missing_catalog_and_disables_legacy_bootstrap() {
        let directory = tempfile::tempdir().expect("create startup authority fixture");
        let (catalog_base, catalog_target) = catalog_publication_images(&directory).await;
        let publication = publication_transaction(catalog_base.clone(), catalog_target);
        let lifecycle = LifecycleJournal::at(directory.path())
            .preflight()
            .await
            .expect("preflight empty lifecycle authority");

        let authority = select_startup_catalog_authority(
            catalog_base.storage_path(),
            false,
            Some(&publication),
            &lifecycle,
        )
        .expect("select publication startup authority");

        assert!(!authority.allow_legacy_bootstrap);
        assert_eq!(authority.recovery_catalog, Some(catalog_base));
    }

    #[tokio::test]
    async fn matching_prepared_lifecycle_and_catalog_publication_authority_is_accepted() {
        let directory = tempfile::tempdir().expect("create paired startup authority fixture");
        let (catalog_base, catalog_target) = catalog_publication_images(&directory).await;
        let publication = publication_transaction(catalog_base.clone(), catalog_target.clone());
        let lifecycle_transaction =
            lifecycle_transaction_with_catalog(catalog_base.clone(), catalog_target);
        let lifecycle_journal = LifecycleJournal::at(directory.path());
        lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("establish completed migration authority");
        lifecycle_journal
            .create(&lifecycle_transaction)
            .await
            .expect("persist paired lifecycle authority");
        let lifecycle = lifecycle_journal
            .preflight()
            .await
            .expect("preflight paired lifecycle authority");

        let authority = select_startup_catalog_authority(
            catalog_base.storage_path(),
            false,
            Some(&publication),
            &lifecycle,
        )
        .expect("accept matching prepared authorities");

        assert!(!authority.allow_legacy_bootstrap);
        assert_eq!(authority.recovery_catalog, Some(catalog_base));
    }

    #[tokio::test]
    async fn mismatched_or_advanced_lifecycle_authority_rejects_catalog_publication() {
        let directory = tempfile::tempdir().expect("create conflicting startup authority fixture");
        let (catalog_base, catalog_target) = catalog_publication_images(&directory).await;
        let publication = publication_transaction(catalog_base.clone(), catalog_target.clone());
        let mismatched_lifecycle =
            lifecycle_transaction_with_catalog(catalog_base.clone(), catalog_base.clone());
        let lifecycle_journal = LifecycleJournal::at(directory.path());
        lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("establish mismatched completed migration authority");
        lifecycle_journal
            .create(&mismatched_lifecycle)
            .await
            .expect("persist mismatched lifecycle authority");
        let lifecycle = lifecycle_journal
            .preflight()
            .await
            .expect("preflight mismatched lifecycle authority");
        let error = select_startup_catalog_authority(
            catalog_base.storage_path(),
            false,
            Some(&publication),
            &lifecycle,
        )
        .expect_err("mismatched authorities must block startup");
        assert!(error.to_string().contains("does not match"));

        let advanced_directory =
            tempfile::tempdir().expect("create advanced startup authority fixture");
        let (advanced_base, advanced_target) =
            catalog_publication_images(&advanced_directory).await;
        let advanced_publication =
            publication_transaction(advanced_base.clone(), advanced_target.clone());
        let matching_lifecycle =
            lifecycle_transaction_with_catalog(advanced_base.clone(), advanced_target);
        let advanced_lifecycle_journal = LifecycleJournal::at(advanced_directory.path());
        advanced_lifecycle_journal
            .mark_migration_complete(uuid::Uuid::new_v4(), SystemTime::now())
            .await
            .expect("establish advanced completed migration authority");
        advanced_lifecycle_journal
            .create(&matching_lifecycle)
            .await
            .expect("persist matching lifecycle authority");
        advanced_lifecycle_journal
            .advance(
                matching_lifecycle.id(),
                LifecycleTransactionPhase::Prepared,
                LifecycleTransactionPhase::CatalogPublished,
            )
            .await
            .expect("advance lifecycle authority past prepared");
        let lifecycle = advanced_lifecycle_journal
            .preflight()
            .await
            .expect("preflight advanced lifecycle authority");
        let error = select_startup_catalog_authority(
            advanced_base.storage_path(),
            false,
            Some(&advanced_publication),
            &lifecycle,
        )
        .expect_err("advanced lifecycle authority must reject an inner publication journal");
        assert!(error.to_string().contains("after its prepared phase"));
    }

    #[tokio::test]
    async fn availability_published_startup_accepts_only_an_exact_target() {
        let directory = tempfile::tempdir().expect("create lifecycle startup fixture");
        let attachment_path = directory.path().join("attachments.json");
        let catalog = ProjectCatalog::with_path(directory.path().join("catalog.json"));
        let attachment_base = AttachmentStoreSnapshot::default();
        let mut attachment_target = attachment_base.clone();
        let project_path = directory.path().join("project");
        attachment_target.replace_project(
            &project_path,
            vec![Attachment {
                project_path: project_path.clone(),
                source: AttachmentSource::Pin,
                created_at: SystemTime::now(),
            }],
            false,
        );
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LegacyV1Migration,
            SystemTime::now(),
            Some(
                CatalogTransactionImages::new(catalog.clone(), catalog)
                    .expect("prepare stable catalog images"),
            ),
            Vec::new(),
            AttachmentTransactionImages::new(attachment_base.clone(), attachment_target.clone()),
        )
        .expect("prepare AvailabilityPublished migration fixture");
        let journal = LifecycleJournal::at(directory.path());
        journal
            .create(&transaction)
            .await
            .expect("create migration journal");
        journal
            .advance(
                transaction.id(),
                LifecycleTransactionPhase::Prepared,
                LifecycleTransactionPhase::CatalogPublished,
            )
            .await
            .expect("advance migration through catalog publication");
        journal
            .advance(
                transaction.id(),
                LifecycleTransactionPhase::CatalogPublished,
                LifecycleTransactionPhase::AvailabilityPublished,
            )
            .await
            .expect("advance migration through availability publication");
        let preflight = journal
            .preflight()
            .await
            .expect("preflight AvailabilityPublished migration");
        assert!(preflight.pending_legacy_attachment_images().is_some());

        // A permissive v1 document is accepted only because its normalized
        // projection is the journal's exact base image.
        let legacy_base = serde_json::json!({
            "attachments": {},
            "manually_stopped": []
        });
        tokio::fs::write(
            &attachment_path,
            serde_json::to_vec_pretty(&legacy_base).expect("serialize legacy base"),
        )
        .await
        .expect("write legacy base");
        let mut store = AttachmentStore::new(attachment_path.clone());
        load_attachment_store_for_lifecycle_recovery(&mut store, &preflight)
            .await
            .expect("accept normalized legacy base");
        assert_eq!(store.snapshot(), attachment_base);

        // Once the target has been published, only its complete exact-v2 shape
        // is accepted.
        tokio::fs::write(
            &attachment_path,
            serde_json::to_vec_pretty(&attachment_target).expect("serialize exact target"),
        )
        .await
        .expect("write exact target");
        let mut store = AttachmentStore::new(attachment_path.clone());
        load_attachment_store_for_lifecycle_recovery(&mut store, &preflight)
            .await
            .expect("accept exact compatibility target");
        assert_eq!(store.snapshot(), attachment_target);

        let target_value =
            serde_json::to_value(&attachment_target).expect("encode target corruption fixtures");
        let mut target_with_unknown = target_value.clone();
        target_with_unknown["unexpected"] = serde_json::json!(true);
        let mut target_missing_field = target_value;
        target_missing_field
            .as_object_mut()
            .expect("compatibility target is an object")
            .remove("instance_owners");

        for (label, malformed_target, expected_error) in [
            (
                "unknown field",
                target_with_unknown,
                "declares exact v2 shape",
            ),
            (
                "missing field",
                target_missing_field,
                "does not match the journal base",
            ),
        ] {
            let preserved =
                serde_json::to_vec_pretty(&malformed_target).expect("serialize malformed target");
            tokio::fs::write(&attachment_path, &preserved)
                .await
                .expect("write malformed target");
            let mut store = AttachmentStore::new(attachment_path.clone());
            let error = load_attachment_store_for_lifecycle_recovery(&mut store, &preflight)
                .await
                .expect_err("non-exact target must block startup");
            assert!(
                error.to_string().contains(expected_error),
                "{label}: {error:#}"
            );
            assert_eq!(
                tokio::fs::read(&attachment_path)
                    .await
                    .expect("reread preserved malformed target"),
                preserved,
                "{label} must remain byte-for-byte preserved"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn noop_availability_publication_preserves_strict_and_unreadable_state() {
        let directory = tempfile::tempdir().expect("create no-op lifecycle startup fixture");
        let attachment_path = directory.path().join("attachments.json");
        let catalog = ProjectCatalog::with_path(directory.path().join("catalog.json"));
        let attachments = AttachmentStoreSnapshot::default();
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LegacyV1Migration,
            SystemTime::now(),
            Some(
                CatalogTransactionImages::new(catalog.clone(), catalog)
                    .expect("prepare stable catalog images"),
            ),
            Vec::new(),
            AttachmentTransactionImages::new(attachments.clone(), attachments.clone()),
        )
        .expect("prepare no-op AvailabilityPublished migration fixture");
        let journal = LifecycleJournal::at(directory.path());
        journal
            .create(&transaction)
            .await
            .expect("create no-op migration journal");
        journal
            .advance(
                transaction.id(),
                LifecycleTransactionPhase::Prepared,
                LifecycleTransactionPhase::CatalogPublished,
            )
            .await
            .expect("advance no-op migration through catalog publication");
        journal
            .advance(
                transaction.id(),
                LifecycleTransactionPhase::CatalogPublished,
                LifecycleTransactionPhase::AvailabilityPublished,
            )
            .await
            .expect("advance no-op migration through availability publication");
        let preflight = journal
            .preflight()
            .await
            .expect("preflight no-op AvailabilityPublished migration");

        let mut malformed =
            serde_json::to_value(&attachments).expect("encode no-op target corruption fixture");
        malformed["unexpected"] = serde_json::json!(true);
        let preserved = serde_json::to_vec_pretty(&malformed)
            .expect("serialize malformed no-op compatibility target");
        tokio::fs::write(&attachment_path, &preserved)
            .await
            .expect("write malformed no-op compatibility target");
        let mut store = AttachmentStore::new(attachment_path.clone());
        let error = load_attachment_store_for_lifecycle_recovery(&mut store, &preflight)
            .await
            .expect_err("strict-shaped no-op target must block legacy fallback");
        assert!(
            error.to_string().contains("declares exact v2 shape"),
            "{error:#}"
        );
        assert_eq!(
            tokio::fs::read(&attachment_path)
                .await
                .expect("reread preserved malformed no-op target"),
            preserved
        );

        tokio::fs::remove_file(&attachment_path)
            .await
            .expect("remove malformed no-op target");
        let missing_target = directory.path().join("missing-attachment-target");
        std::os::unix::fs::symlink(&missing_target, &attachment_path)
            .expect("create dangling compatibility-state symlink");
        let mut store = AttachmentStore::new(attachment_path.clone());
        let error = load_attachment_store_for_lifecycle_recovery(&mut store, &preflight)
            .await
            .expect_err("dangling compatibility-state entry must block legacy fallback");
        assert!(
            error
                .to_string()
                .contains("Failed to read existing lifecycle compatibility state"),
            "{error:#}"
        );
        assert!(
            tokio::fs::symlink_metadata(&attachment_path)
                .await
                .expect("inspect preserved dangling compatibility-state entry")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            tokio::fs::read_link(&attachment_path)
                .await
                .expect("read preserved dangling compatibility-state link"),
            missing_target
        );
    }
}
