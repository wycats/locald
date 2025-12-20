//! # locald-cli
//!
//! The command-line interface for `locald`.
//!
//! ## Entry Point
//!
//! *   [`main`]: The entry point.
//! *   [`handlers::run`]: The main command dispatcher.
//! *   [`cli::Cli`]: The Clap struct definition.

#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/wycats/dotlocal/phase-23-advanced-service-config/locald-docs/public/favicon.svg"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/wycats/dotlocal/phase-23-advanced-service-config/locald-docs/public/favicon.svg"
)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::disallowed_methods)] // CLI tool can use blocking I/O
#![allow(clippy::print_stdout)] // CLI tool uses stdout
#![allow(clippy::print_stderr)] // CLI tool uses stderr
#![allow(missing_docs)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::or_fun_call)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::branches_sharing_code)]
#![allow(clippy::let_underscore_must_use)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::unnecessary_debug_formatting)]
use anyhow::Result;
use clap::Parser;

mod build;
mod cli;
mod client;
mod container;
mod crash;
mod debug;
mod doctor;
mod handlers;
mod hints;
mod history;
mod init;
mod monitor;
mod progress;
mod run;
mod service;
mod style;
mod surface_manifest;
mod trust;
mod try_cmd;
mod utils;

// Force rebuild 3
fn main() {
    // Install panic hook for crash reporting
    std::panic::set_hook(Box::new(|info| {
        let err = anyhow::anyhow!("Panic: {}", info);
        crash::handle_crash(err);
    }));

    let cli = cli::Cli::parse();

    if let Err(e) = run_main(cli) {
        crash::handle_crash(e);
    }
}

fn run_main(cli: cli::Cli) -> Result<()> {
    if let Some(sandbox_name) = &cli.sandbox {
        utils::setup_sandbox(sandbox_name)?;
    }

    // Skip verification for admin setup, as it's used to fix the shim
    if !matches!(
        cli.command,
        cli::Commands::Admin {
            command: cli::AdminCommands::Setup
        } | cli::Commands::Doctor { .. }
            | cli::Commands::Surface { .. }
    ) {
        utils::verify_shim();
    }

    handlers::run(cli)
}
