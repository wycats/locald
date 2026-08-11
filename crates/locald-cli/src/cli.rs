use crate::channel::{BUILD_CHANNEL, BUILD_VERSION};
use clap::{Parser, Subcommand};
use std::sync::LazyLock;

/// The long version string, computed once at startup.
static LONG_VERSION: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}\nChannel: {}\nFeatures: {}",
        BUILD_VERSION,
        BUILD_CHANNEL,
        enabled_features().join(", ")
    )
});

/// Returns a list of enabled experimental features.
fn enabled_features() -> Vec<&'static str> {
    let mut features = vec![];

    #[cfg(feature = "experimental-plugins")]
    features.push("plugins");

    #[cfg(feature = "experimental-vmm")]
    features.push("vmm");

    #[cfg(feature = "experimental-cnb")]
    features.push("cnb");

    #[cfg(feature = "experimental-containers")]
    features.push("containers");

    if features.is_empty() {
        features.push("none (stable)");
    }

    features
}

#[derive(Parser)]
#[command(name = "locald")]
#[command(version = BUILD_VERSION)]
#[command(long_version = LONG_VERSION.as_str())]
#[command(about = "Local development proxy and process manager", long_about = None)]
pub struct Cli {
    /// Run in a sandbox environment
    #[arg(long, global = true)]
    pub sandbox: Option<String>,

    /// Assert that this sandbox's host cannot suspend while locald is running
    #[arg(long, global = true, requires = "sandbox")]
    pub sandbox_no_host_suspend: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new locald project
    Init {
        /// Initialize from a distribution archive (local path or URL)
        #[arg(long = "from-distribution")]
        from_distribution: Option<String>,

        /// Project name (overrides prompt/default when using --from-distribution)
        #[arg(long)]
        name: Option<String>,

        /// Target directory (default: `./<project_name>`)
        #[arg(long)]
        target: Option<std::path::PathBuf>,

        /// Skip scaffold files (only install plugins + locald.toml)
        #[arg(long)]
        no_scaffold: bool,

        /// Use only bundled plugins, skip remote fetches
        #[arg(long)]
        offline: bool,

        /// Accept all defaults without prompting
        #[arg(short, long)]
        yes: bool,

        /// Show detailed initialization steps
        #[arg(short, long)]
        verbose: bool,
    },
    /// Build a project using Cloud Native Buildpacks (nightly only)
    #[cfg(feature = "experimental-cnb")]
    Build {
        /// Path to the project (default: current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        /// Builder image to use (default: heroku/builder:22)
        #[arg(long, default_value = "heroku/builder:22")]
        builder: String,
        /// Additional buildpacks to use (can be specified multiple times)
        #[arg(long, short = 'b')]
        buildpack: Vec<String>,
        /// Show verbose output
        #[arg(long, short)]
        verbose: bool,
    },
    /// Experiment with a command (attached). On exit, prompts to save to locald.toml.
    ///
    /// This command runs the specified command in the current terminal.
    /// It injects a dynamic PORT and sets up the environment.
    /// When the command exits (e.g. via Ctrl-C), you will be asked if you want
    /// to save it as a permanent service in your locald.toml.
    Try {
        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Run a one-off task in the context of a service (with injected environment)
    ///
    /// This is useful for running database migrations, consoles, or other
    /// ad-hoc tasks that need the same environment variables (DB URL, etc.)
    /// as your running services.
    #[command(name = "run", alias = "exec")]
    Exec {
        /// Name of the service to use as context
        service: String,
        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Add a service to locald.toml
    ///
    /// Examples:
    ///   locald add npm start        # Add an exec service
    ///   locald add postgres mydb    # Add a postgres service named "mydb"
    ///   locald add last             # Add the last successful `try` command
    Add {
        /// Command to run, "postgres [name]", or "last" for the last successful `try` command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
        /// Name of the service (default: web for exec, db for postgres)
        #[arg(short, long)]
        name: Option<String>,
        /// Port the service listens on (exec services only)
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Manage services
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },
    /// Monitor running services (TUI)
    Monitor,
    /// Ping the locald daemon
    Ping,
    /// Install the locald Root CA into the system trust store
    Trust,
    /// Server management commands
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
    /// Self-upgrade locald to a newer version
    Selfupgrade {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,

        /// Install specific version (default: latest)
        #[arg(long)]
        version: Option<String>,
    },
    /// Ensure the current project is ready and print its URLs
    Up {
        /// Path to the service directory (defaults to current directory if locald.toml exists)
        path: Option<std::path::PathBuf>,
        /// Show verbose output
        #[arg(long, short)]
        verbose: bool,
        /// Follow this project's logs after it becomes ready
        #[arg(long)]
        follow: bool,
    },
    /// Open the dashboard in the default browser
    Dashboard,

    /// Diagnose host readiness for running locald
    Doctor {
        /// Machine-readable JSON output
        #[arg(long)]
        json: bool,

        /// Include extra diagnostic evidence
        #[arg(long)]
        verbose: bool,
    },
    /// Pause the current project. With a name, stop only that service.
    Stop {
        /// Name of the service to stop
        name: Option<String>,

        /// Machine-readable JSON output
        #[arg(long)]
        json: bool,
    },
    /// Keep a project available even without an active demand
    Pin {
        /// Path to the project (defaults to the current directory)
        path: Option<std::path::PathBuf>,
    },
    /// Return a project to automatic demand-based availability
    Unpin {
        /// Path to the project (defaults to the current directory)
        path: Option<std::path::PathBuf>,
    },
    /// Restart a running service
    Restart {
        /// Name of the service to restart
        name: String,

        /// Machine-readable JSON output
        #[arg(long)]
        json: bool,
    },
    /// Explain desired and actual project availability
    Status {
        /// Project path (defaults to the current directory)
        path: Option<std::path::PathBuf>,
        /// Show all known projects instead of the current project
        #[arg(long, conflicts_with = "path")]
        all: bool,
        /// Machine-readable JSON output
        #[arg(long)]
        json: bool,
    },
    /// Stream logs from services
    Logs {
        /// Name of the service to stream logs for (optional)
        service: Option<String>,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
    /// Administrative commands
    Admin {
        #[command(subcommand)]
        command: AdminCommands,
    },
    /// Manage the menu bar agent
    Tray {
        #[command(subcommand)]
        command: TrayCommands,
    },
    /// AI integration commands
    Ai {
        #[command(subcommand)]
        command: AiCommands,
    },
    /// Debugging tools
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Registry management commands
    Registry {
        #[command(subcommand)]
        command: RegistryCommands,
    },
    /// Project lifecycle management (plumbing).
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// Container management commands (nightly only)
    #[cfg(feature = "experimental-containers")]
    Container {
        #[command(subcommand)]
        command: ContainerCommands,
    },

    /// Manage WASM plugins (nightly only)
    #[cfg(feature = "experimental-plugins")]
    Plugin {
        #[command(subcommand)]
        command: PluginCommands,
    },

    /// Manage distributions (nightly only)
    #[cfg(feature = "experimental-plugins")]
    Distribution {
        #[command(subcommand)]
        command: DistributionCommands,
    },

    /// Serve a directory via HTTP
    Serve {
        /// Path to the directory to serve (default: current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
        /// Interface to bind to
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
    },

    /// Isolated child-process entry point for embedded Postgres setup.
    #[command(name = "__postgres-setup", hide = true)]
    PostgresSetup {
        #[arg(long, hide = true)]
        version: String,
        #[arg(long, hide = true)]
        port: u16,
        #[arg(long, hide = true)]
        data_dir: std::path::PathBuf,
        #[arg(long, hide = true)]
        installation_dir: std::path::PathBuf,
    },

    /// Internal tooling commands (not part of the taught surface)
    #[command(name = "__surface", hide = true)]
    Surface {
        #[command(subcommand)]
        command: SurfaceCommands,
    },
}

#[cfg(feature = "experimental-plugins")]
#[derive(Subcommand)]
pub enum PluginCommands {
    /// Install a plugin from a local path or URL
    Install {
        /// Local path or URL to a WASM component or .locald-package
        source: String,

        /// Optional installed name (only for raw .wasm; packages use manifest name)
        #[arg(long)]
        name: Option<String>,

        /// Install into the current project's .locald/plugins directory
        #[arg(long)]
        project: bool,

        /// Install into user-local plugins directory ($XDG_DATA_HOME/locald/plugins/)
        #[arg(long)]
        user: bool,

        /// Overwrite existing plugin with same name
        #[arg(long)]
        force: bool,
    },

    /// Inspect a plugin by running detect/apply and printing a normalized debug JSON plan
    Inspect {
        /// Plugin name (resolved from plugin dirs) or a path to a WASM component
        plugin: String,

        /// Service kind to present to the plugin
        #[arg(long)]
        kind: String,

        /// Service name to present to the plugin (defaults to kind)
        #[arg(long)]
        name: Option<String>,

        /// Dependencies (comma-separated)
        #[arg(long)]
        depends_on: Option<String>,

        /// Service config entries (repeatable): --config key=value
        #[arg(long, value_name = "key=value")]
        config: Vec<String>,

        /// Grant capabilities (repeatable): --grant `oci_pull`
        #[arg(long)]
        grant: Vec<String>,
    },

    /// Validate the plan produced by a plugin (non-zero on errors)
    Validate {
        /// Plugin name (resolved from plugin dirs) or a path to a WASM component
        plugin: String,

        /// Service kind to present to the plugin
        #[arg(long)]
        kind: String,

        /// Service name to present to the plugin (defaults to kind)
        #[arg(long)]
        name: Option<String>,

        /// Dependencies (comma-separated)
        #[arg(long)]
        depends_on: Option<String>,

        /// Service config entries (repeatable): --config key=value
        #[arg(long, value_name = "key=value")]
        config: Vec<String>,

        /// Grant capabilities (repeatable): --grant `oci_pull`
        #[arg(long)]
        grant: Vec<String>,
    },

    /// Create a distributable plugin package (.locald-package)
    Create {
        /// Source directory containing manifest.toml [default: .]
        #[arg(default_value = ".")]
        source: std::path::PathBuf,

        /// Output package path [default: {name}-{version}.locald-package]
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,

        /// Manifest file path relative to source [default: manifest.toml]
        #[arg(short, long)]
        manifest: Option<std::path::PathBuf>,

        /// Show what would be packaged without creating archive
        #[arg(long)]
        dry_run: bool,

        /// Overwrite existing output file
        #[arg(long)]
        force: bool,

        /// Show detailed packaging steps
        #[arg(short, long)]
        verbose: bool,
    },
}

#[cfg(feature = "experimental-plugins")]
#[derive(Subcommand)]
pub enum DistributionCommands {
    /// Create a distributable distribution archive (.locald-distribution)
    Create {
        /// Source directory containing distribution.toml [default: .]
        #[arg(default_value = ".")]
        source: std::path::PathBuf,

        /// Output distribution path [default: {name}-{version}.locald-distribution]
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,

        /// Manifest file path relative to source [default: distribution.toml]
        #[arg(short, long)]
        manifest: Option<std::path::PathBuf>,

        /// Fetch and bundle remote plugins instead of keeping as references
        #[arg(long)]
        include_remote: bool,

        /// Show what would be packaged without creating archive
        #[arg(long)]
        dry_run: bool,

        /// Overwrite existing output file
        #[arg(long)]
        force: bool,

        /// Show detailed packaging steps
        #[arg(short, long)]
        verbose: bool,
    },
}

#[derive(Subcommand)]
pub enum SurfaceCommands {
    /// Print a machine-readable CLI surface manifest (JSON)
    #[command(name = "cli-manifest")]
    CliManifest,
}

#[cfg(feature = "experimental-containers")]
#[derive(Subcommand)]
pub enum ContainerCommands {
    /// Run an ephemeral container
    Run {
        /// Image to run
        image: String,
        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
        /// Run in interactive mode
        #[arg(short = 'i', long)]
        interactive: bool,
        /// Run in detached mode
        #[arg(short = 'd', long)]
        detached: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show the current configuration
    Show {
        /// Show provenance (where each value came from)
        #[arg(long)]
        provenance: bool,
    },
}

#[derive(Subcommand)]
pub enum RegistryCommands {
    /// List registered projects
    List,
    /// Pin a project (keep it running)
    Pin {
        /// Path to the project (default: current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
    },
    /// Unpin a project
    Unpin {
        /// Path to the project (default: current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
    },
    /// Forget missing catalog records while preserving project data
    Clean,
}

#[derive(Subcommand)]
pub enum ProjectCommands {
    /// Machine-facing VS Code lifecycle adapter.
    #[command(hide = true)]
    Editor {
        #[command(subcommand)]
        command: EditorCommands,
    },
    /// Register an attachment (editor, CLI, or pin).
    Attach {
        path: std::path::PathBuf,
        /// Attachment source: editor or cli.
        #[arg(long)]
        source: Option<String>,
        /// Editor name (required when source=editor).
        #[arg(long)]
        editor_name: Option<String>,
        /// Editor id (required when source=editor).
        #[arg(long)]
        editor_id: Option<String>,
        /// Editor process id (used to reap stale editor attachments).
        #[arg(long, hide = true)]
        editor_pid: Option<u32>,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Remove an attachment.
    Detach {
        path: std::path::PathBuf,
        /// Attachment source: editor or cli.
        #[arg(long)]
        source: Option<String>,
        /// Editor id (required when source=editor).
        #[arg(long)]
        editor_id: Option<String>,
    },
    /// Force-start services (emergency override).
    Start { path: std::path::PathBuf },
    /// Force-stop services (emergency override).
    Stop { path: std::path::PathBuf },
    /// Restart this project's locald-managed services as one lifecycle operation.
    #[command(hide = true)]
    Restart {
        path: std::path::PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show project status.
    Status {
        path: std::path::PathBuf,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List known projects.
    List {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
        /// Filter: active, pinned, recent, all.
        #[arg(long)]
        filter: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum EditorCommands {
    /// Semantically activate or refocus a VS Code project and wait for readiness.
    Ensure {
        path: std::path::PathBuf,
        #[arg(long)]
        window_id: String,
        #[arg(long)]
        host_pid: u32,
        #[arg(long)]
        json: bool,
    },
    /// Passively renew an existing live VS Code window demand.
    Renew {
        path: std::path::PathBuf,
        #[arg(long)]
        window_id: String,
        #[arg(long)]
        host_pid: u32,
        #[arg(long)]
        json: bool,
    },
    /// Release one VS Code window demand.
    Release {
        path: std::path::PathBuf,
        #[arg(long)]
        window_id: String,
        #[arg(long)]
        host_pid: u32,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Add a new service
    Add {
        #[command(subcommand)]
        service_type: AddServiceType,
    },
    /// Reset a service (stop, wipe data, restart)
    Reset {
        /// Name of the service
        name: String,
    },
    /// Stop one service without changing project availability
    Stop {
        /// Name of the service
        name: String,
    },
    /// Restart one service
    Restart {
        /// Name of the service
        name: String,
    },
}

#[derive(Subcommand)]
pub enum AddServiceType {
    /// Add a shell command service
    Exec {
        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
        /// Name of the service
        #[arg(short, long)]
        name: Option<String>,
        /// Port the service listens on
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Add a managed Postgres service
    Postgres {
        /// Name of the service
        name: String,
        /// Postgres version
        #[arg(long)]
        version: Option<String>,
    },
    /// Add a container service
    Container {
        /// Docker image to run
        image: String,
        /// Name of the service
        #[arg(short, long)]
        name: Option<String>,
        /// Port exposed by the container
        #[arg(short, long)]
        container_port: Option<u16>,
        /// Command to run in the container
        #[arg(long)]
        command: Option<String>,
    },
    /// Add a static site service
    Site {
        /// Path to the directory to serve
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        /// Name of the service
        #[arg(short, long)]
        name: Option<String>,
        /// Port the service listens on
        #[arg(short, long)]
        port: Option<u16>,
        /// Build command to run before serving
        #[arg(long)]
        build: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ServerCommands {
    /// Run the daemon in the foreground
    Start,
    /// Shutdown the running daemon
    Shutdown,
    /// Restart the daemon
    Restart,
}

#[derive(Subcommand)]
pub enum AdminCommands {
    /// Setup locald permissions (auto-escalates to root).
    Setup,
    /// Remove admin setup (helper, `LaunchAgent`, config). Auto-escalates to root.
    Teardown,
    /// Sync the hosts file with daemon-owned domains.
    SyncHosts,
}

#[derive(Subcommand)]
pub enum TrayCommands {
    /// Start the menu bar agent
    Start,
    /// Stop the menu bar agent
    Stop,
    /// Check whether the menu bar agent is running
    Status,
    /// Restart the menu bar agent
    Restart,
}

#[derive(Subcommand)]
pub enum AiCommands {
    /// Get the JSON schema for locald.toml
    Schema,
    /// Get the current system context (running services, etc.)
    Context,
}

#[derive(Subcommand)]
pub enum DebugCommands {
    /// Check which process is listening on a port
    Port {
        /// Port number to check
        port: u16,
    },
    /// Show CLI and daemon binary identity
    Identity {
        /// Machine-readable JSON output
        #[arg(long)]
        json: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_host_suspend_guarantee_requires_an_explicit_sandbox() {
        let error = Cli::try_parse_from(["locald", "--sandbox-no-host-suspend", "ping"])
            .err()
            .expect("the no-host-suspend guarantee must not stand alone");

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn explicit_sandbox_accepts_the_no_host_suspend_guarantee() {
        let cli = Cli::try_parse_from([
            "locald",
            "--sandbox",
            "ci",
            "--sandbox-no-host-suspend",
            "ping",
        ])
        .expect("parse an explicitly guaranteed sandbox");

        assert_eq!(cli.sandbox.as_deref(), Some("ci"));
        assert!(cli.sandbox_no_host_suspend);
    }

    #[test]
    fn parse_run_maps_to_exec_variant() {
        let cli = Cli::try_parse_from(["locald", "run", "api", "echo", "hi"]).unwrap();

        match cli.command {
            Commands::Exec { service, command } => {
                assert_eq!(service, "api");
                assert_eq!(command, vec!["echo".to_string(), "hi".to_string()]);
            }
            _ => panic!("expected Commands::Exec"),
        }
    }

    #[test]
    fn parse_exec_alias_maps_to_exec_variant() {
        let cli = Cli::try_parse_from(["locald", "exec", "api", "echo", "hi"]).unwrap();

        match cli.command {
            Commands::Exec { service, command } => {
                assert_eq!(service, "api");
                assert_eq!(command, vec!["echo".to_string(), "hi".to_string()]);
            }
            _ => panic!("expected Commands::Exec"),
        }
    }

    #[test]
    fn parse_hidden_editor_ensure_captures_window_provenance() {
        let cli = Cli::try_parse_from([
            "locald",
            "project",
            "editor",
            "ensure",
            "/project",
            "--window-id",
            "window-a",
            "--host-pid",
            "42",
            "--json",
        ])
        .expect("parse editor ensure");

        match cli.command {
            Commands::Project {
                command:
                    ProjectCommands::Editor {
                        command:
                            EditorCommands::Ensure {
                                path,
                                window_id,
                                host_pid,
                                json,
                            },
                    },
            } => {
                assert_eq!(path, std::path::PathBuf::from("/project"));
                assert_eq!(window_id, "window-a");
                assert_eq!(host_pid, 42);
                assert!(json);
            }
            _ => panic!("expected editor ensure command"),
        }
    }

    #[test]
    fn parse_hidden_project_restart_captures_project_locator() {
        let cli = Cli::try_parse_from(["locald", "project", "restart", "/project", "--json"])
            .expect("parse project restart");

        match cli.command {
            Commands::Project {
                command: ProjectCommands::Restart { path, json },
            } => {
                assert_eq!(path, std::path::PathBuf::from("/project"));
                assert!(json);
            }
            _ => panic!("expected editor restart command"),
        }
    }

    #[test]
    fn parse_hidden_postgres_setup_captures_the_bounded_request() {
        let cli = Cli::try_parse_from([
            "locald",
            "__postgres-setup",
            "--version",
            "15.3",
            "--port",
            "54321",
            "--data-dir",
            "/data/postgres",
            "--installation-dir",
            "/data/postgres-dist",
        ])
        .expect("parse Postgres setup helper");

        match cli.command {
            Commands::PostgresSetup {
                version,
                port,
                data_dir,
                installation_dir,
            } => {
                assert_eq!(version, "15.3");
                assert_eq!(port, 54321);
                assert_eq!(data_dir, std::path::PathBuf::from("/data/postgres"));
                assert_eq!(
                    installation_dir,
                    std::path::PathBuf::from("/data/postgres-dist")
                );
            }
            _ => panic!("expected Postgres setup helper command"),
        }
    }
}
