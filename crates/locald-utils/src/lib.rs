//! Shared utilities for locald.

/// Menu bar agent management utilities (macOS).
#[cfg(target_os = "macos")]
pub mod agent;
/// Certificate management utilities.
pub mod cert;
/// Cgroup v2 helpers (Linux only).
#[cfg(target_os = "linux")]
pub mod cgroup;
/// Container detection helpers.
pub mod container;
/// Port discovery utilities.
pub mod discovery;
/// Environment variable utilities.
pub mod env;
/// Filesystem utilities.
pub mod fs;
/// IPC utilities.
pub mod ipc;
/// Notification server utilities (Linux only).
#[cfg(target_os = "linux")]
pub mod notify;
/// Postgres management utilities.
pub mod postgres;
/// Privileged capability acquisition + readiness reporting.
pub mod privileged;
/// Probe utilities.
pub mod probe;
/// Process management utilities.
pub mod process;
/// Project management utilities.
pub mod project;
/// Shim management utilities.
pub mod shim;
