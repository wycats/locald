use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "locald")]
#[command(about = "Local development proxy and process manager", long_about = None)]
pub struct Cli {
    /// Run in a sandbox environment
    #[arg(long, global = true)]
    pub sandbox: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new locald project
    Init,
    /// Build a project using Cloud Native Buildpacks
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
    #[command(alias = "run")]
    Exec {
        /// Name of the service to use as context
        service: String,
        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Add a service to locald.toml (shortcut for `locald service add exec`)
    Add {
        /// Command to run (or "last" to use the last successful `try` command)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
        /// Name of the service (default: web)
        #[arg(short, long)]
        name: Option<String>,
        /// Port the service listens on
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
    /// Start the daemon (if needed) and register the current project
    Up {
        /// Path to the service directory (defaults to current directory if locald.toml exists)
        path: Option<std::path::PathBuf>,
        /// Show verbose output
        #[arg(long, short)]
        verbose: bool,
    },
    /// Open the dashboard in the default browser
    Dashboard,
    /// Stop a running service. If no name is provided, stops all services defined in locald.toml in the current directory.
    Stop {
        /// Name of the service to stop
        name: Option<String>,
    },
    /// Restart a running service
    Restart {
        /// Name of the service to restart
        name: String,
    },
    /// List running services
    Status,
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
    /// Container management commands
    Container {
        #[command(subcommand)]
        command: ContainerCommands,
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
}

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
    /// Remove non-existent projects from the registry
    Clean,
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
    /// Setup locald permissions (requires sudo)
    Setup,
    /// Sync hosts file with running services (requires sudo)
    SyncHosts,
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
}
