//! # locald-core
//!
//! `locald-core` provides the shared types, configuration structures, and common utilities
//! used across the `locald` ecosystem (CLI, Server, Builder).
//!
//! ## Architecture
//!
//! ```mermaid
//! graph TD
//!     CLI[locald-cli] -->|Uses| Core[locald-core]
//!     Server[locald-server] -->|Uses| Core
//!     Builder[locald-builder] -->|Uses| Core
//!     
//!     Core --> Config[Configuration]
//!     Core --> IPC[IPC Protocol]
//!     Core --> State[Service State]
//! ```
//!
//! ## Key Modules
//!
//! *   [`config`]: Defines the `locald.toml` schema and global configuration.
//! *   [`ipc`]: Defines the request/response protocol between CLI and Server.
//! *   [`state`]: Defines the runtime state of services (Running, Stopped, etc.).
//! *   [`registry`]: Manages the list of known projects.
//!
//! ## Entry Points
//!
//! *   **Parsing Config**: Start with [`LocaldConfig`].
//! *   **Talking to the Daemon**: Use [`IpcRequest`] and [`IpcResponse`].
//! *   **Managing State**: See [`ServiceState`] and [`Registry`].

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
#![warn(clippy::let_underscore_must_use)] // Don't swallow errors with `let _`
#![warn(clippy::manual_let_else)] // Enforces clean "Guard Clause" style
#![warn(clippy::unwrap_used)] // Force error propagation (no panics)
#![warn(clippy::expect_used)] // Force error propagation

// 2. Numeric Safety (Critical for PIDs/Ports)
#![warn(clippy::cast_possible_truncation)] // Warn on u64 -> u32 (potential data loss)
#![warn(clippy::cast_possible_wrap)] // Warn on u32 -> i32 (potential overflow)

// 3. Observability
#![warn(clippy::print_stderr)] // Ban eprintln! (Use tracing::error!)

// 4. Import Hygiene
#![warn(clippy::wildcard_imports)] // Ban `use crate::*` (Explicit imports only)
#![warn(clippy::shadow_unrelated)] // Ban accidental variable shadowing

// 5. Documentation
#![allow(missing_docs)] // TODO: Enable later
#![allow(clippy::missing_errors_doc)] // TODO: Enable later
#![allow(clippy::doc_markdown)]
#![allow(clippy::let_underscore_must_use)]

pub mod config;
#[doc(inline)]
pub use config::LocaldConfig;
pub mod ipc;
#[doc(inline)]
pub use ipc::{IpcRequest, IpcResponse};
pub mod hosts;
#[doc(inline)]
pub use hosts::HostsFileSection;
pub mod state;
#[doc(inline)]
pub use state::{ServerState, ServiceState};
pub mod registry;
#[doc(inline)]
pub use registry::Registry;
pub mod resolver;
#[doc(inline)]
pub use resolver::ServiceResolver;
pub mod buildpack;
pub mod service;
#[doc(inline)]
pub use service::{
    RuntimeState, ServiceCommand, ServiceContext, ServiceController, ServiceFactory,
};

#[cfg(test)]
mod config_test;
