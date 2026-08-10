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
use clap::Parser;

#[cfg(feature = "experimental-cnb")]
mod build;
mod channel;
mod cli;
mod client;
#[cfg(feature = "experimental-containers")]
mod container;
mod crash;
mod debug;
#[cfg(feature = "experimental-plugins")]
mod distribution;
mod doctor;
mod error;
mod global_config;
mod handlers;
mod hints;
mod history;
mod init;
#[cfg(target_os = "macos")]
mod macos_helper;
#[cfg(target_os = "macos")]
mod macos_setup;
mod monitor;
#[cfg(feature = "experimental-plugins")]
mod plugin;
mod progress;
mod run;
mod selfupgrade;
mod service;
mod style;
mod surface_manifest;
mod trust;
mod try_cmd;
mod update_check;
mod utils;

// Force rebuild 3
fn main() {
    let colors_enabled = style::configure_colors();

    if let Err(err) = miette::set_hook(Box::new(move |_| {
        Box::new(
            miette::MietteHandlerOpts::new()
                .color(colors_enabled)
                .terminal_links(true)
                .unicode(true)
                .build(),
        )
    })) {
        eprintln!("Warning: failed to install miette handler: {err}");
    }

    // Install panic hook for crash reporting
    std::panic::set_hook(Box::new(|info| {
        let report = miette::Report::msg(format!("Panic: {info}"));
        crash::handle_crash(report);
    }));

    let cli = cli::Cli::parse();

    if let Err(e) = run_main(cli) {
        if e.is_expected() {
            eprintln!("{e}");
            std::process::exit(1);
        }
        crash::handle_crash(miette::Report::new(e));
    }
}

fn run_main(cli: cli::Cli) -> error::CliResult<()> {
    // The daemon spawns this hidden helper specifically so opaque embedded
    // Postgres setup cannot inherit publisher descriptors. It must run before
    // ordinary installation readiness checks or daemon startup behavior.
    if let cli::Commands::PostgresSetup {
        version,
        port,
        data_dir,
        installation_dir,
    } = &cli.command
    {
        return handlers::run_internal_postgres_setup(version, *port, data_dir, installation_dir);
    }

    if let Some(sandbox_name) = &cli.sandbox {
        utils::setup_sandbox(sandbox_name)?;
    }

    // Repair, inspection, and shutdown commands must remain available when the
    // installation is incomplete. Other standard commands share the canonical
    // fail-closed readiness preflight.
    #[cfg(feature = "experimental-plugins")]
    let skip_verify = matches!(
        cli.command,
        cli::Commands::Admin {
            command: cli::AdminCommands::Setup | cli::AdminCommands::Teardown
        } | cli::Commands::Doctor { .. }
            | cli::Commands::Server {
                command: cli::ServerCommands::Shutdown
            }
            | cli::Commands::Trust
            | cli::Commands::Surface { .. }
            | cli::Commands::Init { .. }
            | cli::Commands::Selfupgrade { .. }
            | cli::Commands::Plugin {
                command: cli::PluginCommands::Create { .. }
            }
            | cli::Commands::Plugin {
                command: cli::PluginCommands::Install { .. }
            }
            | cli::Commands::Distribution {
                command: cli::DistributionCommands::Create { .. }
            }
    );
    #[cfg(not(feature = "experimental-plugins"))]
    let skip_verify = matches!(
        cli.command,
        cli::Commands::Admin {
            command: cli::AdminCommands::Setup | cli::AdminCommands::Teardown
        } | cli::Commands::Doctor { .. }
            | cli::Commands::Server {
                command: cli::ServerCommands::Shutdown
            }
            | cli::Commands::Trust
            | cli::Commands::Surface { .. }
            | cli::Commands::Init { .. }
            | cli::Commands::Selfupgrade { .. }
    );

    if !skip_verify {
        utils::verify_shim();
    }

    handlers::run(cli)
}
