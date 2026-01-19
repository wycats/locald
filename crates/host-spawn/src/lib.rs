//! Guest-to-host command execution for containerized environments.
//!
//! This crate provides type-safe abstractions for executing commands on the host
//! system from within containerized environments like Toolbx, Distrobox, or Flatpak.
//!
//! # Overview
//!
//! When developing inside a container (e.g., Fedora Toolbx), you often need to run
//! commands on the host system—for example, starting a privileged daemon. This crate
//! abstracts the various mechanisms for doing so:
//!
//! - `flatpak-spawn --host` for Toolbx and Flatpak
//! - `distrobox-host-exec` for Distrobox
//! - Custom templates for SSH or other mechanisms
//! - Direct execution when not containerized
//!
//! # Example
//!
//! ```no_run
//! use host_spawn::{detect_host_exec, HostCommand, Privilege, SpawnHost};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Build a command to run on the host
//!     let cmd = HostCommand::builder()
//!         .program("locald-shim")
//!         .args(vec!["serve".into()])
//!         .privilege(Privilege::Pkexec)
//!         .build();
//!
//!     // Detect the best mechanism and execute
//!     if let Some(exec) = detect_host_exec().await {
//!         match exec.spawn(&cmd).await {
//!             Ok(status) => println!("Exited with: {:?}", status.code()),
//!             Err(e) => eprintln!("Failed: {e}"),
//!         }
//!     }
//! }
//! ```
//!
//! # Container Detection
//!
//! The crate uses a tiered detection hierarchy based on specification stability:
//!
//! - **Tier 1 (Official Standards)**: `$FLATPAK_ID`, `/.flatpak-info`, `/run/systemd/container`
//! - **Tier 2 (De Facto Standards)**: `/run/.containerenv`, `/.dockerenv`
//! - **Tier 3 (Implementation Details)**: `$TOOLBOX_PATH`, `$DISTROBOX_ENTER_PATH`
//!
//! # Security
//!
//! - Direct modes (`FlatpakSpawn`, `DistroboxHostExec`, `Direct`) pass arguments
//!   directly to the OS without shell interpretation.
//! - Template mode uses proper shell escaping (single-quote wrapping) to prevent injection.
//! - Only `pkexec` and `sudo` are supported for privilege escalation.

#![warn(missing_docs)]

mod command;
mod detect;
mod error;
mod exec;

pub use command::{HostCommand, HostCommandBuilder, Privilege};
pub use detect::{
    ContainerEnvironment, command_exists, detect_container, detect_host_exec, is_containerized,
};
pub use error::{HostSpawnError, Result};
pub use exec::{HostExec, SpawnHost};
