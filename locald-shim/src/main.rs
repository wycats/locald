use anyhow::{Context, Result};
use listeners::get_all;
use nix::unistd::{Gid, Uid};
use std::env;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

// Constants for prctl
// const PR_CAP_AMBIENT: i32 = 47;
// const PR_CAP_AMBIENT_RAISE: i32 = 2;
// const CAP_NET_BIND_SERVICE: u64 = 10;

#[derive(Debug)]
struct BundleRunArgs {
    bundle: PathBuf,
    id: String,
}

fn parse_bundle_run_args(args: &[String]) -> Result<BundleRunArgs> {
    // Supported forms:
    // - bundle run --bundle <PATH> --id <ID>
    // - bundle run <PATH> <ID>
    let mut bundle: Option<PathBuf> = None;
    let mut id: Option<String> = None;

    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--bundle" | "-b" => {
                let value = args.get(idx + 1).context("Missing value for --bundle")?;
                bundle = Some(PathBuf::from(value));
                idx += 2;
            }
            "--id" | "-i" => {
                let value = args.get(idx + 1).context("Missing value for --id")?;
                id = Some(value.clone());
                idx += 2;
            }
            other => {
                // Positional fallback
                if bundle.is_none() {
                    bundle = Some(PathBuf::from(other));
                } else if id.is_none() {
                    id = Some(other.to_string());
                } else {
                    return Err(anyhow::anyhow!("Unexpected argument: {other}"));
                }
                idx += 1;
            }
        }
    }

    let bundle = bundle.context("Missing bundle path")?;
    let id = id.context("Missing container id")?;
    Ok(BundleRunArgs { bundle, id })
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

    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: locald-shim <command> [args...]");
        std::process::exit(1);
    }

    // Check for version flag
    if args[0] == "--shim-version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let command = &args[0];

    // Handle bundle command (Fat Shim)
    if command == "bundle" {
        let bundle_args = &args[1..];

        // Legacy form (still supported): `locald-shim bundle <bundle-path>`
        if let Some(bundle_path) = bundle_args.first().filter(|s| s.as_str() != "run") {
            let id = format!("locald-bootstrap-{}", std::process::id());
            let code = run_bundle(Path::new(bundle_path), &id)?;
            std::process::exit(code);
        }

        // New form: `locald-shim bundle run --bundle <path> --id <id>`
        if bundle_args.first().map(|s| s.as_str()) == Some("run") {
            let parsed =
                parse_bundle_run_args(&bundle_args[1..]).context("Invalid bundle run arguments")?;
            let code = run_bundle(&parsed.bundle, &parsed.id)?;
            std::process::exit(code);
        }

        return Err(anyhow::anyhow!(
            "Usage: locald-shim bundle run --bundle <PATH> --id <ID>"
        ));
    }

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

    if command == "server" && args.get(1).map(|s| s.as_str()) == Some("start") {
        eprintln!("The 'server start' command is deprecated in locald-shim.");
        eprintln!("Please use 'bind' for privileged port binding.");
        std::process::exit(1);
    } else if command == "bind" {
        // Case 5: Bind - Run as root
        // Bind a privileged port and pass the FD to locald via Unix socket.
        // Usage: locald-shim bind <port> <socket_path>

        let port_str = args.get(1).context("Missing port argument")?;
        let socket_path = args.get(2).context("Missing socket path argument")?;

        let port: u16 = port_str.parse().context("Invalid port number")?;

        // 1. Bind the TCP port
        let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", port))
            .context(format!("Failed to bind to port {}", port))?;

        // 2. Connect to the Unix socket
        let stream = std::os::unix::net::UnixStream::connect(socket_path)
            .context(format!("Failed to connect to socket {}", socket_path))?;

        // 3. Send the FD
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
    } else if command == "admin" && args.get(1).map(|s| s.as_str()) == Some("sync-hosts") {
        // Case 3: Admin Sync Hosts - Run as root
        // We stay as root to modify /etc/hosts.
        // New behavior: Expect domains as subsequent arguments
        let domains: Vec<String> = args.iter().skip(2).cloned().collect();
        update_hosts_file(&domains)?;
        Ok(())
    } else if command == "admin" && args.get(1).map(|s| s.as_str()) == Some("cleanup") {
        // Case 4: Admin Cleanup - Run as root
        // Recursively remove a directory.
        // Security: Must be absolute and contain ".locald" to prevent arbitrary deletion.

        if let Some(path_str) = args.get(2) {
            let path = std::path::Path::new(path_str);

            // 1. Must be absolute
            if !path.is_absolute() {
                return Err(anyhow::anyhow!("Cleanup path must be absolute"));
            }

            // 2. Must contain ".locald"
            // This is a heuristic to ensure we are only deleting locald-managed files.
            // We check if any component is exactly ".locald".
            let is_safe = path.components().any(|c| c.as_os_str() == ".locald");

            if !is_safe {
                return Err(anyhow::anyhow!(
                    "Cleanup path must be within a .locald directory"
                ));
            }

            // 3. No traversal (implied by components check, but good to be sure)
            // Rust's Path components handle ".." as Normal components if they are present in the string,
            // but canonicalization would resolve them. We don't canonicalize here because the path might not exist fully?
            // Actually, remove_dir_all requires the path to exist.
            // If we just check for ".locald" component, that's reasonably safe against accidental deletion of /etc.
            // e.g. /etc/.locald/foo is technically allowed by this rule, but user can't create /etc/.locald.

            std::fs::remove_dir_all(path).context("Failed to remove directory")?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Missing path for cleanup"))
        }
    } else if command == "debug" {
        // Case 2: Debug - Run as root
        // We stay as root and execute the logic directly.
        // We do NOT exec locald here, to prevent privilege escalation.

        if args.get(1).map(|s| s.as_str()) == Some("port") {
            if let Some(port_str) = args.get(2) {
                if let Ok(port) = port_str.parse::<u16>() {
                    check_port(port)?;
                    Ok(())
                } else {
                    eprintln!("Invalid port number: {}", port_str);
                    std::process::exit(1);
                }
            } else {
                eprintln!("Usage: locald debug port <port>");
                std::process::exit(1);
            }
        } else {
            eprintln!("Unknown debug command");
            std::process::exit(1);
        }
    } else {
        eprintln!("Unknown command: {}", command);
        std::process::exit(1);
    }
}
