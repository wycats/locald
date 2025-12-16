use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use listeners::get_all;
use nix::unistd::{Gid, Uid};
use std::env;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

// Constants for prctl
// const PR_CAP_AMBIENT: i32 = 47;
// const PR_CAP_AMBIENT_RAISE: i32 = 2;
// const CAP_NET_BIND_SERVICE: u64 = 10;

#[derive(Debug, Parser)]
#[command(name = "locald-shim")]
#[command(about = "Privileged helper for locald (internal protocol)")]
#[command(disable_help_subcommand = true)]
struct Cli {
    /// Print the shim version and exit.
    #[arg(long = "shim-version", action = clap::ArgAction::SetTrue)]
    shim_version: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Execute an OCI bundle.
    Bundle {
        #[command(subcommand)]
        command: BundleCommand,
    },

    /// Bind a privileged TCP port and pass the FD over a unix socket.
    Bind(BindArgs),

    /// Privileged admin operations.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },

    /// Debug helpers.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Debug, Subcommand)]
enum BundleCommand {
    /// Run a bundle as the container init process.
    Run(BundleRunArgs),
}

#[derive(Debug, Args)]
struct BundleRunArgs {
    /// Path to OCI bundle directory.
    #[arg(long)]
    bundle: PathBuf,

    /// Container identifier.
    #[arg(long)]
    id: String,
}

#[derive(Debug, Args)]
struct BindArgs {
    port: u16,
    socket_path: PathBuf,
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// Synchronize /etc/hosts section for locald domains.
    SyncHosts(AdminSyncHostsArgs),

    /// Recursively remove a locald-managed directory.
    Cleanup(AdminCleanupArgs),
}

#[derive(Debug, Args)]
struct AdminSyncHostsArgs {
    domains: Vec<String>,
}

#[derive(Debug, Args)]
struct AdminCleanupArgs {
    path: PathBuf,
}

#[derive(Debug, Subcommand)]
enum DebugCommand {
    /// Show processes listening on a port (requires root for full visibility).
    Port(DebugPortArgs),
}

#[derive(Debug, Args)]
struct DebugPortArgs {
    port: u16,
}

fn run_bundle(bundle_path: &Path, container_id: &str) -> Result<i32> {
    let canonical_bundle_path = bundle_path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize bundle path: {bundle_path:?}"))?;

    // Keep container state inside the bundle directory to remain sandboxed/cleanup-able.
    let state_root = canonical_bundle_path.join(".locald-shim-state");
    std::fs::create_dir_all(&state_root).context("Failed to create shim state root")?;

    let mut container = libcontainer::container::builder::ContainerBuilder::new(
        container_id.to_string(),
        libcontainer::syscall::syscall::SyscallType::Linux,
    )
    .with_root_path(&state_root)?
    .as_init(&canonical_bundle_path)
    .with_systemd(false)
    .with_detach(false)
    .build()?;

    container.start()?;

    let init_pid = container
        .pid()
        .ok_or_else(|| anyhow::anyhow!("libcontainer did not report an init pid"))?;

    let init_pid_raw = init_pid.as_raw();

    // Forward termination-ish signals to the container init process.
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGQUIT,
        signal_hook::consts::SIGHUP,
    ])
    .context("Failed to register signal handlers")?;

    std::thread::spawn(move || {
        for sig in signals.forever() {
            // Avoid nix::Pid type mismatches (libcontainer depends on a different nix).
            // libc::kill uses the raw pid_t.
            unsafe {
                let _ = libc::kill(init_pid_raw, sig);
            }
        }
    });

    // Wait for the container init process to exit.
    loop {
        let mut status: libc::c_int = 0;
        let res = unsafe { libc::waitpid(init_pid_raw, &mut status, 0) };

        if res < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }

            let _ = std::fs::remove_dir_all(container.root);
            return Err(anyhow::anyhow!("waitpid failed: {err}"));
        }

        // Decode wait status. This is the traditional waitpid encoding:
        // - exited: (status & 0x7f) == 0, code = (status >> 8) & 0xff
        // - signaled: (status & 0x7f) != 0 && (status & 0x7f) != 0x7f
        let status_i32 = status as i32;
        let low = status_i32 & 0x7f;

        if low == 0 {
            let code = (status_i32 >> 8) & 0xff;
            let _ = std::fs::remove_dir_all(container.root);
            return Ok(code);
        }

        // Stopped/continued: keep waiting.
        if low == 0x7f {
            continue;
        }

        let _ = std::fs::remove_dir_all(container.root);
        return Ok(128 + low);
    }
}

fn check_port(port: u16) -> Result<()> {
    println!("Checking port {}...", port);

    let listeners = get_all()
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .context("Failed to get system listeners")?;

    let mut found = false;
    let mut printed_header = false;

    for listener in listeners {
        if listener.socket.port() == port {
            if !printed_header {
                println!("{:<10} {:<20} {:<20}", "PID", "NAME", "ADDRESS");
                printed_header = true;
            }
            found = true;
            println!(
                "{:<10} {:<20} {:<20}",
                listener.process.pid, listener.process.name, listener.socket
            );
        }
    }

    if !found {
        println!("No process found listening on port {}.", port);
    }

    Ok(())
}

use std::fmt::Write;

#[allow(clippy::disallowed_methods)]
fn update_hosts_file(domains: &[String]) -> Result<()> {
    let path = if cfg!(windows) {
        std::path::PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    } else {
        std::path::PathBuf::from("/etc/hosts")
    };

    let current_content = std::fs::read_to_string(&path).context("Failed to read hosts file")?;

    let start_marker = "# BEGIN locald";
    let end_marker = "# END locald";

    let mut new_section = String::new();
    new_section.push_str(start_marker);
    new_section.push('\n');
    for domain in domains {
        let _ = writeln!(new_section, "127.0.0.1 {domain}");
    }
    new_section.push_str(end_marker);

    let new_content = if let Some(start) = current_content.find(start_marker) {
        if let Some(end_idx) = current_content[start..].find(end_marker) {
            let end = start + end_idx;
            // Replace existing section
            let mut output = String::from(&current_content[..start]);
            output.push_str(&new_section);
            output.push_str(&current_content[end + end_marker.len()..]);
            output
        } else {
            // Malformed block, append
            let mut output = String::from(&current_content);
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&new_section);
            output.push('\n');
            output
        }
    } else {
        // Append if not found
        let mut output = String::from(&current_content);
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&new_section);
        output.push('\n');
        output
    };

    std::fs::write(&path, new_content).context("Failed to write hosts file")?;
    Ok(())
}

fn main() -> Result<()> {
    // Sanitize environment: Unset LOCALD_SHIM_ACTIVE to prevent spoofing
    // SAFETY: This is safe because we are single-threaded at this point.
    unsafe {
        env::remove_var("LOCALD_SHIM_ACTIVE");
    }

    let cli = Cli::parse();
    if cli.shim_version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let Some(command) = cli.command else {
        // clap will print help for `--help`; this is just for the no-args case.
        anyhow::bail!("Missing command. Run with --help for usage.");
    };

    // Determine the real user (who ran the shim)
    let _real_uid = Uid::current();
    let _real_gid = Gid::current();

    // We expect to be setuid root
    let effective_uid = Uid::effective();
    if !effective_uid.is_root() {
        // If not root, we cannot perform privileged operations.
        // We must NOT exec locald, as that creates a loop if locald called us.
        eprintln!("locald-shim must be setuid root to function.");
        eprintln!("Current effective UID: {}", effective_uid);
        eprintln!("Please run `sudo locald admin setup` to fix permissions.");
        std::process::exit(1);
    }

    match command {
        Commands::Bundle {
            command: BundleCommand::Run(args),
        } => {
            let code = run_bundle(&args.bundle, &args.id)?;
            std::process::exit(code);
        }
        Commands::Bind(args) => {
            // Case: Bind - Run as root
            // Bind a privileged port and pass the FD to locald via Unix socket.
            let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", args.port))
                .with_context(|| format!("Failed to bind to port {}", args.port))?;

            let stream =
                std::os::unix::net::UnixStream::connect(&args.socket_path).with_context(|| {
                    format!("Failed to connect to socket {}", args.socket_path.display())
                })?;

            let fd = listener.as_raw_fd();
            let stream_fd = stream.as_raw_fd();

            let iov = [std::io::IoSlice::new(b"fd")];
            let cmsgs = [nix::sys::socket::ControlMessage::ScmRights(&[fd])];

            nix::sys::socket::sendmsg::<nix::sys::socket::UnixAddr>(
                stream_fd,
                &iov,
                &cmsgs,
                nix::sys::socket::MsgFlags::empty(),
                None,
            )
            .context("Failed to send file descriptor")?;

            Ok(())
        }
        Commands::Admin {
            command: AdminCommand::SyncHosts(args),
        } => {
            update_hosts_file(&args.domains)?;
            Ok(())
        }
        Commands::Admin {
            command: AdminCommand::Cleanup(args),
        } => {
            let path = &args.path;

            if !path.is_absolute() {
                anyhow::bail!("Cleanup path must be absolute");
            }

            let is_safe = path.components().any(|c| c.as_os_str() == ".locald");
            if !is_safe {
                anyhow::bail!("Cleanup path must be within a .locald directory");
            }

            std::fs::remove_dir_all(path).context("Failed to remove directory")?;
            Ok(())
        }
        Commands::Debug {
            command: DebugCommand::Port(args),
        } => {
            check_port(args.port)?;
            Ok(())
        }
    }
}
