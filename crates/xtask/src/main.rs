#![allow(clippy::disallowed_methods)]
#![allow(clippy::collapsible_if)]

use anyhow::Result;
use clap::{Parser, Subcommand};
use xshell::Shell;

mod assets;
mod check;
mod ci;
mod coverage;
mod deps;
mod dev;
mod docs;
mod e2e;
mod fast;
mod fix;
mod sandbox;
mod util;
mod verify;

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Development tasks for locald", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run CI-equivalent checks (fmt, clippy, test)
    Check {
        /// Run in full mode (includes sudo + e2e)
        #[arg(long)]
        full: bool,
    },
    /// Asset management tasks
    Assets {
        #[command(subcommand)]
        command: AssetsCommands,
    },
    /// E2E test tasks
    E2e {
        #[command(subcommand)]
        command: E2eCommands,
    },
    /// Dependency management tasks
    Deps {
        #[command(subcommand)]
        command: DepsCommands,
    },
    /// Verification tasks
    Verify {
        #[command(subcommand)]
        command: VerifyCommands,
    },
    /// Fix code style and lint issues
    Fix,
    /// Run fast build (wraps cargo build with sccache/mold)
    Build {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Run fast clippy (wraps cargo clippy with sccache/mold)
    Clippy {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// CI helper tasks
    Ci {
        #[command(subcommand)]
        command: CiCommands,
    },
    /// Sandbox helper tasks
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommands,
    },
    /// Development helper tasks
    Dev {
        #[command(subcommand)]
        command: DevCommands,
    },
    /// Run coverage
    Coverage,
}

#[derive(Subcommand)]
enum CiCommands {
    /// Watch CI status
    Watch {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, default_value = "CI")]
        workflow: String,
        #[arg(long, default_value = "10")]
        interval: u64,
    },
    /// Check CI logs
    Logs {
        #[arg(long)]
        watch: bool,
    },
    /// Check for untested changes
    Tripwire {
        #[arg(default_value = "origin/main")]
        base: String,
    },
}

#[derive(Subcommand)]
enum SandboxCommands {
    /// Run command in sandbox
    Run {
        sandbox: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum DevCommands {
    /// Run dev server
    Server,
}

#[derive(Subcommand)]
enum AssetsCommands {
    /// Build and sync assets
    Build,
}

#[derive(Subcommand)]
enum E2eCommands {
    /// Run dashboard E2E tests
    Dashboard,
}

#[derive(Subcommand)]
enum DepsCommands {
    /// Update dependencies
    Update,
}

#[derive(Subcommand)]
enum VerifyCommands {
    /// Verify install assets
    InstallAssets,
    /// Verify update
    Update,
    /// Verify autostart
    Autostart,
    /// Verify OCI example
    Oci,
    /// Verify Phase 33 (Draft Mode & History)
    Phase33,
    /// Verify Phase (General)
    Phase,
    /// Verify documentation
    Docs,
    /// Verify Exec Controller
    Exec,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sh = Shell::new()?;

    match cli.command {
        Commands::Check { full } => check::run(&sh, full)?,
        Commands::Assets { command } => match command {
            AssetsCommands::Build => assets::build(&sh)?,
        },
        Commands::E2e { command } => match command {
            E2eCommands::Dashboard => e2e::dashboard(&sh)?,
        },
        Commands::Deps { command } => match command {
            DepsCommands::Update => deps::update(&sh)?,
        },
        Commands::Verify { command } => match command {
            VerifyCommands::InstallAssets => verify::install_assets(&sh)?,
            VerifyCommands::Update => verify::update(&sh)?,
            VerifyCommands::Autostart => verify::autostart(&sh)?,
            VerifyCommands::Oci => verify::oci(&sh)?,
            VerifyCommands::Phase33 => verify::phase33(&sh)?,
            VerifyCommands::Phase => verify::phase(&sh)?,
            VerifyCommands::Docs => verify::docs(&sh)?,
            VerifyCommands::Exec => verify::exec(&sh)?,
        },
        Commands::Fix => fix::run(&sh)?,
        Commands::Build { args } => fast::build(&sh, args)?,
        Commands::Clippy { args } => fast::clippy(&sh, args)?,
        Commands::Ci { command } => match command {
            CiCommands::Watch {
                run_id,
                branch,
                workflow,
                interval,
            } => ci::watch(&sh, run_id, branch, workflow, interval)?,
            CiCommands::Logs { watch } => ci::logs(&sh, watch)?,
            CiCommands::Tripwire { base } => ci::tripwire(&sh, base)?,
        },
        Commands::Sandbox { command } => match command {
            SandboxCommands::Run { sandbox, args } => sandbox::run(&sh, sandbox, args)?,
        },
        Commands::Dev { command } => match command {
            DevCommands::Server => dev::server(&sh)?,
        },
        Commands::Coverage => coverage::run(&sh)?,
    }

    Ok(())
}
