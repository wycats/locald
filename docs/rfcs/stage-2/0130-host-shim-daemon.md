---
title: Host Shim Daemon for Container Development Environments
stage: 2
feature: Container Development
---

# RFC 0130: Host Shim Daemon for Container Development Environments

**Stage**: 2 (Draft)
**Author**: locald team
**Created**: 2026-01-06
**Related**: RFC 0096 (Leaf Node Axiom), RFC 0097 (Strict Discovery)

## Summary

Enable `locald` to "just work" in container development environments (Toolbx, Distrobox) by having the container-side `locald` communicate with a host-side privileged daemon via a Unix socket.

## Decision

**Option B (Persistent Daemon Mode)** is the chosen approach because:

1. **Socket-based communication is mechanism-agnostic**: Once the daemon is running, containers connect via `~/.locald/shim.sock` without needing any special host-exec mechanism
2. **Manual fallback always works**: Users can start the daemon on the host themselves if auto-detection fails
3. **Lower latency**: No per-operation overhead for repeated privileged operations

## Motivation

The canonical workflow for Toolbx/Distrobox users is:

1. Development environment lives in the container (compilers, runtimes, tools)
2. Browser runs on the host
3. `*.localhost` domains should work seamlessly

**The problem**: Privileged operations (cgroups, `/etc/hosts`, cert trust) require host access, but:

- Setuid binaries can't be executed with privileges across user namespace boundaries
- Asking users to manually run setup on the host breaks the "just works" experience

**Goal**: `locald up` inside a container should automatically handle the host/container split without manual intervention.

## Relationship to RFC 0096 (Leaf Node Axiom)

RFC 0096 establishes that the shim should be a "leaf node":

> "The shim performs its task and exits. It never executes the locald binary."

The daemon mode introduces a **long-running shim process** which technically violates this axiom. However, the daemon still never executes the `locald` binary—it only handles requests from clients. The axiom's intent (preventing privilege escalation loops) is preserved.

**Safeguards** to mitigate the persistent process concern:

- Idle timeout (5 minutes after last client disconnects)
- Max lifetime (1 hour, ensures fresh binary after updates)
- Socket authentication via `SO_PEERCRED`
- Automatic restart: clients transparently restart the shim if needed

### Shim vs Server: Two Separate Daemons

It's important to distinguish between the **shim daemon** (this RFC) and the **locald server**:

| Daemon            | Purpose                                     | Runs On           | Lifecycle                      |
| ----------------- | ------------------------------------------- | ----------------- | ------------------------------ |
| **locald server** | Manages dev services (processes, proxies)   | Container or host | Persistent until `locald stop` |
| **locald shim**   | Privileged ops only (hosts, certs, cgroups) | Host only         | On-demand with timeouts        |

**The server has NO idle timeout**—it stays running as long as you want dev services available. "Pinned" projects (RFC 0026) should auto-start when the server boots.

**The shim has aggressive timeouts** because:

- It only handles discrete, quick operations (< 100ms each)
- It doesn't manage long-running work
- Auto-restart is transparent to users
- Recycling ensures fresh binaries after updates

## Design

### Host-Exec Configuration

The daemon can be started automatically (if a host-exec mechanism is available) or manually. To support diverse environments, locald uses a **configurable host-exec template**:

```toml
# ~/.config/locald/config.toml (or locald.toml in project)
[container]
# Template for running commands on the host from inside a container.
# {command} is replaced with the actual command to run.
#
# Examples:
#   host_exec = "flatpak-spawn --host {command}"
#   host_exec = "distrobox-host-exec {command}"
#   host_exec = "ssh host {command}"
#
# If not set, auto-detection is attempted.
host_exec = "flatpak-spawn --host {command}"
```

### Startup Flow

```
User runs: locald up (inside container)
                │
                ▼
┌─────────────────────────────────────────┐
│ Is shim socket available and responsive?│
│ (~/.locald/shim.sock)                   │
└─────────────────┬───────────────────────┘
                  │
         ┌───────┴───────┐
         │ Yes           │ No
         ▼               ▼
┌─────────────┐  ┌──────────────────────────────────────────┐
│ Connect to  │  │ Try to start shim daemon on host:        │
│ existing    │  │                                          │
│ shim,       │  │ 1. Use configured host_exec template     │
│ proceed     │  │ 2. Auto-detect: flatpak-spawn, host-exec │
│             │  │ 3. Fail with actionable message          │
└─────────────┘  └─────────────────┬────────────────────────┘
                                   │
                                   ▼
                 ┌─────────────────────────────────────────┐
                 │ If all fail, print:                     │
                 │                                         │
                 │ "Could not start host shim daemon.      │
                 │  Please run on your host:               │
                 │    sudo locald shim serve               │
                 │                                         │
                 │  Or configure host_exec in config.toml" │
                 └─────────────────────────────────────────┘
```

### Manual Setup Options

When auto-start isn't available, users have two choices:

**Option 1: Run manually (no systemd)**

```bash
# On host (outside container)
sudo locald shim serve

# This runs in foreground; daemon exits when you Ctrl-C
# Or background it: sudo locald shim serve &
```

**Option 2: Systemd user service**

```bash
# On host
locald shim install-service

# Creates ~/.config/systemd/user/locald-shim.service
# Starts automatically when you log in
systemctl --user enable --now locald-shim
```

The advantage of no-systemd mode: if you can access the host at all (e.g., via a terminal), you can start the daemon. No extra setup required.

### Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│ HOST                                                                     │
│                                                                          │
│   locald-shim (started on-demand, persists while locald server runs)    │
│   ├── Listens on: ~/.locald/shim.sock                                   │
│   ├── Manages: /etc/hosts, cgroups, privileged ports, cert trust        │
│   └── Started via: distrobox-host-exec / flatpak-spawn --host           │
│                                                                          │
│   [Socket is in $HOME, shared between host and container]               │
│                                                                          │
│   ┌────────────────────────────────────────────────────────────────┐    │
│   │ TOOLBX / DISTROBOX CONTAINER                                    │    │
│   │                                                                  │    │
│   │   locald server (runs your dev services)                        │    │
│   │   ├── Starts shim daemon on host if not running                 │    │
│   │   ├── Delegates privileged ops via ~/.locald/shim.sock          │    │
│   │   └── Services run here with container's environment            │    │
│   │                                                                  │    │
│   │   locald CLI                                                    │    │
│   │   └── Talks to locald server as usual                           │    │
│   └────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────┘
```

### Key Insight: Network Namespace is Shared

Toolbx and Distrobox share the host's network namespace. This means:

- Containers can bind to host ports directly (no port forwarding)
- `localhost` in the container IS `localhost` on the host
- Browsers on the host can reach services in the container via `127.0.0.1`

**We don't need the shim for networking at all**—only for:

1. `/etc/hosts` synchronization (for `*.localhost` domains)
2. cgroup-based process isolation
3. Privileged port binding (< 1024)
4. System trust store modification

### Non-Persistent by Default

The shim daemon is **not** a systemd service by default. It's started on-demand and lives only as long as needed:

1. First `locald up` starts the shim (via host-exec, manual start, or systemd)
2. Shim creates `~/.locald/shim.sock` and listens
3. When all clients disconnect and idle timeout expires, shim exits
4. Next `locald up` starts it again if needed

This avoids:

- Persistent root processes running when not needed
- Complex systemd service setup
- "Did you remember to start the service?" UX problems

### Socket Location

Using `~/.locald/shim.sock` (inside home directory) instead of `/run/locald/shim.sock`:

**Pros**:

- Automatically shared between host and Toolbx/Distrobox (home is bind-mounted)
- No special container volume mounts needed
- Works with user-level setup (no system directories)

**Cons**:

- Home directory on networked filesystem could be problematic (fallback to `/run/user/$UID/locald/`)
- Socket files in home feel unusual

### Starting the Shim from Container

The shim startup uses the configured `host_exec` template or auto-detection:

```rust
fn start_host_shim(config: &Config) -> Result<()> {
    // 1. Check for user-configured host_exec template
    if let Some(template) = &config.container.host_exec {
        let cmd = template.replace("{command}", "pkexec locald shim serve");
        return run_shell_command(&cmd);
    }

    // 2. Auto-detect available mechanisms
    if command_exists("flatpak-spawn") {
        return Command::new("flatpak-spawn")
            .args(["--host", "pkexec", "locald", "shim", "serve"])
            .status()
            .map(|_| ());
    }

    if command_exists("distrobox-host-exec") {
        return Command::new("distrobox-host-exec")
            .args(["pkexec", "locald", "shim", "serve"])
            .status()
            .map(|_| ());
    }

    // 3. No mechanism available - guide user to manual setup
    Err(anyhow!(
        "Could not start host shim daemon.\n\n\
         Please run on your host:\n\
           sudo locald shim serve\n\n\
         Or configure host_exec in ~/.config/locald/config.toml:\n\
           [container]\n\
           host_exec = \"your-host-exec-command {{command}}\""
    ))
}
```

### Shim Protocol

The protocol uses length-prefixed JSON over the Unix socket.

#### Wire Format

```
┌──────────────────────────────────────────────────────────────┐
│  4 bytes (big-endian u32)  │  N bytes (UTF-8 JSON payload)  │
│       payload length       │         ShimMessage            │
└──────────────────────────────────────────────────────────────┘
```

- **Length prefix**: 4 bytes, big-endian unsigned 32-bit integer
- **Payload**: UTF-8 encoded JSON, exactly `length` bytes
- **Max payload size**: 1 MiB (1,048,576 bytes) — reject larger messages

#### Message Types

```rust
// Handshake (sent first on each connection)
#[derive(Serialize, Deserialize)]
struct Handshake {
    protocol_version: u32,   // Wire format version (currently 1)
    client_version: String,  // Semantic version "1.2.3"
}

// Request format (after handshake)
#[derive(Serialize, Deserialize)]
enum ShimRequest {
    Ping,
    HostsSync { entries: Vec<HostEntry> },
    CgroupSetup { strategy: CgroupStrategy },
    CgroupKill { path: String },
    BindPrivilegedPort { port: u16 },
    TrustInstall { ca_pem: String },
    Shutdown,  // Graceful daemon shutdown
}

#[derive(Serialize, Deserialize)]
struct ShimResponse {
    code: u32,           // 0 = success, non-zero = error
    message: Option<String>,  // Human-readable message (for errors)
    payload: Option<ResponsePayload>,
}

#[derive(Serialize, Deserialize)]
enum ResponsePayload {
    Pong { daemon_version: String, protocol_version: u32 },
    PortReady,  // Followed by SCM_RIGHTS with FD
}
```

#### Error Codes

| Code | Name                   | Description                                        |
| ---- | ---------------------- | -------------------------------------------------- |
| 0    | `OK`                   | Success                                            |
| 1    | `UNKNOWN_REQUEST`      | Request type not recognized                        |
| 2    | `PERMISSION_DENIED`    | Client UID doesn't match daemon's expected UID     |
| 3    | `VERSION_INCOMPATIBLE` | Protocol version not supported                     |
| 4    | `OPERATION_FAILED`     | Operation failed (details in `message`)            |
| 5    | `INVALID_PAYLOAD`      | JSON parse error or invalid field values           |
| 6    | `SHUTTING_DOWN`        | Daemon is shutting down, retry with new daemon     |
| 7    | `RECYCLE_DAEMON`       | Client is newer; daemon will shut down for restart |

#### Version Negotiation

1. Client sends handshake with `protocol_version` and `client_version` (semver)
2. Daemon compares versions:
   - If protocol version unsupported → `VERSION_INCOMPATIBLE`
   - If client version > daemon version → `RECYCLE_DAEMON`
   - Otherwise → proceed with request
3. Client handles `RECYCLE_DAEMON` by restarting the daemon transparently

**See "Resolved Design Decisions > Version Mismatch Policy"** for full details on the recycling logic.

### Socket Security

The socket must be protected from unauthorized access:

```rust
// Socket creation with restricted permissions
let socket_path = home_dir.join(".locald/shim.sock");
let listener = UnixListener::bind(&socket_path)?;

// Set permissions to owner-only (0600)
use std::os::unix::fs::PermissionsExt;
std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

// Validate client UID on each connection
fn validate_client(stream: &UnixStream) -> Result<()> {
    use std::os::unix::net::UCred;
    let cred = stream.peer_cred()?;
    let expected_uid = nix::unistd::getuid();
    if cred.uid() != expected_uid.as_raw() {
        anyhow::bail!("Unauthorized client (UID {} != {})", cred.uid(), expected_uid);
    }
    Ok(())
}
```

**Security Properties**:

- Socket is mode `0600` (owner read/write only)
- Client UID validated via `SO_PEERCRED`
- Only the user who started the daemon can connect
- NFS home directories: socket path should fall back to `/run/user/$UID/locald/` if `$HOME` is networked

### Daemon Lifecycle

```rust
struct ShimDaemon {
    socket: UnixListener,
    active_clients: AtomicUsize,
    last_activity: Mutex<Instant>,
    shutdown: AtomicBool,
}

impl ShimDaemon {
    const IDLE_TIMEOUT: Duration = Duration::from_secs(300);  // 5 minutes
    const MAX_LIFETIME: Duration = Duration::from_secs(3600); // 1 hour

    fn run(&self) -> Result<()> {
        let start = Instant::now();

        loop {
            // Check max lifetime
            if start.elapsed() > Self::MAX_LIFETIME {
                info!("Max lifetime reached, shutting down");
                break;
            }

            // Check idle timeout
            if self.active_clients.load(Ordering::SeqCst) == 0 {
                let idle_time = self.last_activity.lock().elapsed();
                if idle_time > Self::IDLE_TIMEOUT {
                    info!("Idle timeout reached, shutting down");
                    break;
                }
            }

            // Accept connections with timeout
            // ...
        }
        Ok(())
    }
}
```

### Daemonization Sequence

The startup sequence ensures the caller knows the socket is ready before the spawn command returns:

```rust
fn serve_daemon(foreground: bool) -> Result<()> {
    let socket_dir = home_dir()?.join(".locald");
    fs::create_dir_all(&socket_dir)?;

    let socket_path = socket_dir.join("shim.sock");
    let pid_path = socket_dir.join("shim.pid");
    let log_path = socket_dir.join("shim.log");

    // Remove stale socket if exists
    if socket_path.exists() {
        // Check if another daemon is running
        if let Ok(pid) = fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                if process_exists(pid) {
                    // Already running, print path and exit
                    println!("{}", socket_path.display());
                    return Ok(());
                }
            }
        }
        fs::remove_file(&socket_path)?;
    }

    // 1. Bind socket BEFORE forking (ensures ready on return)
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, Permissions::from_mode(0o600))?;

    // 2. Print socket path to signal readiness
    println!("{}", socket_path.display());

    // 3. Flush stdout before fork
    std::io::stdout().flush()?;

    if foreground {
        // Run in foreground (for debugging)
        run_daemon_loop(listener)
    } else {
        // 4. Fork into background using daemonize crate
        let log_file = File::create(&log_path)?;
        let daemonize = Daemonize::new()
            .pid_file(&pid_path)
            .chown_pid_file(true)
            .stdout(log_file.try_clone()?)
            .stderr(log_file);

        daemonize.start()?;

        // 5. In daemon: run accept loop
        run_daemon_loop(listener)
    }
}
```

### Graceful Shutdown

The daemon handles shutdown gracefully to avoid leaving clients in a broken state:

```rust
impl ShimDaemon {
    fn handle_shutdown(&self, reason: &str) {
        info!("Shutdown initiated: {}", reason);
        self.shutdown.store(true, Ordering::SeqCst);

        // 1. Stop accepting new connections
        // (accept loop checks shutdown flag)

        // 2. Wait for in-flight requests (with timeout)
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.active_clients.load(Ordering::SeqCst) > 0 {
            if Instant::now() > deadline {
                warn!("Shutdown timeout, {} clients still active",
                      self.active_clients.load(Ordering::SeqCst));
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // 3. Remove socket file
        let _ = fs::remove_file(&self.socket_path);

        // 4. Remove PID file
        let _ = fs::remove_file(&self.pid_path);

        info!("Shutdown complete");
    }
}

// Signal handler registration
fn setup_signal_handlers(daemon: Arc<ShimDaemon>) {
    // SIGTERM: graceful shutdown
    signal_hook::flag::register(SIGTERM, daemon.shutdown.clone())?;

    // SIGINT: also graceful (for foreground mode)
    signal_hook::flag::register(SIGINT, daemon.shutdown.clone())?;
}
```

**Shutdown triggers**:

- `SIGTERM` / `SIGINT`: Graceful shutdown
- `Shutdown` request via protocol: Graceful shutdown
- Idle timeout (5 min): Auto-shutdown when no clients
- Max lifetime (1 hour): Forced restart for updates

### Graceful Degradation

If shim startup fails or is unavailable:

1. Warn the user (current behavior)
2. Proceed without privileged features
3. Services still run, but:
   - `*.localhost` domains may not resolve
   - Privileged ports won't bind
   - cgroup isolation disabled

## Comparison: Current vs Proposed

| Scenario                    | Current                                 | Proposed                               |
| --------------------------- | --------------------------------------- | -------------------------------------- |
| **Host-only**               | Setuid exec per-operation               | Same (no daemon needed) OR daemon mode |
| **Container: shim works**   | N/A (can't cross namespace)             | Auto-start daemon on host, delegate    |
| **Container: no host-exec** | Warn + degrade                          | Same                                   |
| **User action required**    | `sudo locald admin setup` on host first | None (automatic)                       |

## First-Run Authentication: Polkit

The shim requires root privileges. Rather than asking users to run `sudo` manually, we use **polkit** for a desktop-integrated authentication experience:

### First Run Flow

```
User runs: locald up (first time, no shim installed)
                │
                ▼
┌─────────────────────────────────────────────────────────────────┐
│ locald detects: no shim socket, no setuid binary installed     │
└─────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─────────────────────────────────────────────────────────────────┐
│ pkexec locald-shim install                                      │
│                                                                 │
│   ┌───────────────────────────────────────────────────────┐    │
│   │              Authentication Required                   │    │
│   │                                                        │    │
│   │  "locald" wants to install system components for      │    │
│   │  local development (HTTPS certificates, process       │    │
│   │  isolation, privileged ports).                        │    │
│   │                                                        │    │
│   │  Password: [________________________]                  │    │
│   │                                                        │    │
│   │              [Cancel]  [Authenticate]                  │    │
│   └───────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                │
                ▼
┌─────────────────────────────────────────────────────────────────┐
│ Shim installed as setuid binary at /usr/local/bin/locald-shim  │
│ Cgroups configured                                              │
│ CA certificate generated and trusted                            │
└─────────────────────────────────────────────────────────────────┘
                │
                ▼
        All subsequent operations use setuid shim (no prompts)
```

### Polkit Policy

We ship a polkit policy file that describes the action:

```xml
<!-- /usr/share/polkit-1/actions/dev.locald.policy -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE policyconfig PUBLIC
 "-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd">
<policyconfig>
  <action id="dev.locald.admin-setup">
    <description>Install locald system components</description>
    <message>Authentication is required to install locald system components</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
    <annotate key="org.freedesktop.policykit.exec.path">/usr/local/bin/locald-shim</annotate>
    <annotate key="org.freedesktop.policykit.exec.allow_gui">true</annotate>
  </action>
</policyconfig>
```

### Why Polkit?

| Approach             | UX                            | Security | Desktop Integration |
| -------------------- | ----------------------------- | -------- | ------------------- |
| Manual `sudo`        | Poor (user must know command) | Good     | None                |
| Terminal sudo prompt | Fair                          | Good     | Terminal only       |
| **Polkit (pkexec)**  | **Good (GUI dialog)**         | **Good** | **Native**          |
| Passwordless sudo    | Good                          | Poor     | None                |

Polkit is the standard Linux mechanism for GUI privilege escalation—the same system used by GNOME Software, system settings, and other desktop tools.

### Container Environment

From inside a container, we use `flatpak-spawn --host pkexec` or `distrobox-host-exec pkexec`:

```rust
fn setup_via_host_exec() -> Result<()> {
    // flatpak-spawn forwards the polkit dialog to the host desktop
    Command::new("flatpak-spawn")
        .args(["--host", "pkexec", "locald-shim", "install"])
        .status()?;
    Ok(())
}
```

The polkit GUI prompt appears on the host desktop, authenticates the host user, and installs the shim on the host—exactly what we need.

## Configuration Schema

The container configuration is added to `GlobalConfig` in `locald-core/src/config/global.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct GlobalConfig {
    #[serde(default)]
    pub server: ServerConfig,

    #[serde(default)]
    pub container: ContainerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
pub struct ContainerConfig {
    /// Template for running commands on the host from inside a container.
    ///
    /// The placeholder `{command}` is replaced with the actual command to run.
    /// If not set, auto-detection is attempted (flatpak-spawn, distrobox-host-exec).
    ///
    /// Examples:
    /// - `"flatpak-spawn --host {command}"`
    /// - `"distrobox-host-exec {command}"`
    /// - `"ssh myhost {command}"`
    #[serde(default)]
    pub host_exec: Option<String>,

    /// Override the socket path for the shim daemon.
    ///
    /// Defaults to `~/.locald/shim.sock`. Use this if your home directory
    /// is on a networked filesystem that doesn't support Unix sockets.
    #[serde(default)]
    pub shim_socket: Option<PathBuf>,
}
```

**Config file locations** (in priority order):

1. `./locald.toml` (project-level)
2. `~/.config/locald/config.toml` (user-level)

Example user config for a custom container environment:

```toml
# ~/.config/locald/config.toml
[container]
host_exec = "my-custom-host-exec {command}"
shim_socket = "/run/user/1000/locald/shim.sock"
```

## Integration with RFC 0110 (Privileged Capability)

RFC 0110 establishes that privileged operations require an acquired `Privileged` capability. The shim daemon integrates with this:

```rust
impl PrivilegedCapability {
    /// Acquire privileged capability, using socket if available.
    pub fn acquire(config: &AcquireConfig) -> Result<Self, ReadinessReport> {
        // 1. Try socket-based shim (preferred in containers)
        if let Some(socket) = find_shim_socket(config) {
            if let Ok(client) = ShimClient::connect(&socket) {
                // Validate connection with Ping
                if client.ping().is_ok() {
                    return Ok(Self::Socket(client));
                }
            }
        }

        // 2. Fall back to setuid binary (host-only)
        if let Some(shim_path) = find_setuid_shim() {
            return Ok(Self::Setuid(shim_path));
        }

        // 3. No privilege available
        Err(ReadinessReport::no_shim_available())
    }
}

enum PrivilegedCapability {
    Socket(ShimClient),    // Connected to daemon
    Setuid(PathBuf),       // Path to setuid binary
}
```

The capability abstraction hides whether we're using socket IPC or direct setuid exec—callers just use `privileged.hosts_sync(entries)`.

## Resolved Design Decisions

### Version Mismatch Policy

**Decision**: Newer client triggers automatic daemon recycle.

The handshake includes both protocol version and semantic version:

```rust
struct Handshake {
    protocol_version: u32,   // Wire format version (1, 2, ...)
    client_version: String,  // Semantic version "1.2.3"
}
```

**Compatibility rules**:

| Scenario                 | Action                                  |
| ------------------------ | --------------------------------------- |
| Client v1.2, Daemon v1.5 | ✅ Compatible (daemon is newer)         |
| Client v1.5, Daemon v1.2 | ⚠️ **Recycle daemon** (client is newer) |
| Client v2.0, Daemon v1.x | ❌ Incompatible major version           |

**Why recycle when client is newer?**

- Client may use features the older daemon doesn't have
- New client likely means user just updated `locald`
- Recycling is transparent (client handles it automatically)

**Client-side recycle logic**:

```rust
fn connect_to_shim() -> Result<ShimClient> {
    let client = ShimClient::connect()?;

    match client.handshake()? {
        Response::Ok => Ok(client),
        Response::RecycleDaemon { .. } => {
            // 1. Ask daemon to shut down gracefully
            client.send(ShimRequest::Shutdown)?;

            // 2. Wait for socket to disappear (max 5s)
            wait_for_socket_gone()?;

            // 3. Start new daemon (with our version)
            start_host_shim()?;

            // 4. Reconnect
            ShimClient::connect()
        }
    }
}
```

**Error codes** (updated):

| Code | Name             | Description                                        |
| ---- | ---------------- | -------------------------------------------------- |
| 7    | `RECYCLE_DAEMON` | Client is newer; daemon will shut down for restart |

### Multiple Containers

**Decision**: Shared socket with connection multiplexing.

- Socket at `~/.locald/shim.sock` is shared (home is bind-mounted)
- Daemon handles multiple concurrent connections
- Each connection is independent (no shared state between clients)
- Idle timeout only triggers when ALL clients disconnect

### macOS Support

**Decision**: Out of scope for this RFC.

- macOS doesn't have Toolbx/Distrobox
- Existing setuid model works for macOS host development
- Docker Desktop on macOS has its own privilege model
- Future RFC may address macOS container workflows if needed

### WSL (Windows Subsystem for Linux) Support

**Decision**: Out of scope for this RFC. Requires separate design (future RFC 0131).

**Why WSL is different from Toolbx/Distrobox**:

| Aspect                  | Toolbx/Distrobox          | WSL2                                              |
| ----------------------- | ------------------------- | ------------------------------------------------- |
| **Type**                | Linux container           | Linux VM                                          |
| **Network**             | Shared with host          | Separate (NAT or Mirrored mode)                   |
| **Home directory**      | Bind-mounted from host    | Separate filesystem                               |
| **Hosts file**          | One (host's `/etc/hosts`) | Two (`C:\Windows\...\hosts` + WSL's `/etc/hosts`) |
| **Cert store**          | One (host's)              | Two (Windows + WSL)                               |
| **Host-exec mechanism** | `flatpak-spawn --host`    | Windows interop (run `.exe` files)                |

**Key architectural differences**:

1. **Unix socket can't be shared** between WSL and Windows filesystem
2. **Privileged operations target Windows**, not Linux (e.g., Windows hosts file, Windows Certificate Store)
3. **Two hosts files** must be updated for `*.localhost` to work in Windows browsers
4. **Network may require port forwarding** in NAT mode (mirrored mode on Windows 11 22H2+ shares localhost)

**For MAP (Minimum Awesome Product)**:

- Detect WSL environment (`$WSL_DISTRO_NAME` set, `/proc/version` contains "microsoft")
- Warn user that Windows-side operations require manual setup
- Provide clear instructions for manual hosts file editing
- `*.localhost` domains work within WSL itself (Linux-side only)

**Post-MAP follow-up (RFC 0131: Windows/WSL Privileged Operations)**:

1. Build `locald-shim.exe` for Windows
2. Named pipe communication (`\\.\pipe\locald-shim`)
3. Windows hosts file manipulation (`C:\Windows\System32\drivers\etc\hosts`)
4. Windows Certificate Store integration via `certutil.exe`
5. Port forwarding setup for NAT mode

**Configuration for WSL** (future):

```toml
[wsl]
# Path to Windows shim named pipe
shim_pipe = "\\\\.\\pipe\\locald-shim"

# Network mode (auto-detected)
network_mode = "auto"  # "nat" or "mirrored"
```

## Alternatives Considered

### A. Persistent systemd service

Run `locald-shim` as a system service that starts on boot.

**Rejected because**:

- Requires explicit installation step
- Runs privileged process even when not needed
- More complex setup

### B. Full D-Bus service with polkit for every operation

Expose all shim operations over D-Bus with per-operation polkit authorization.

**Rejected because**:

- Too complex for our needs
- Would prompt on every privileged operation
- D-Bus may not be available in all container environments

**Note**: We DO use polkit, but only for the one-time setup. Subsequent operations use the installed setuid shim.

### C. Kernel sysctl only

Use `net.ipv4.ip_unprivileged_port_start=0` and `/etc/hosts` via other means.

**Rejected because**:

- Doesn't solve cgroup management
- Requires system-wide kernel config
- Doesn't help with cert trust

## Implementation Plan

The daemon mode implementation is divided into phases:

### Phase 1: Socket-based Shim Daemon

**Files**: `crates/locald-shim/src/daemon.rs` (new), `crates/locald-shim/src/protocol.rs` (new)

- [ ] Add `locald-shim serve [--foreground]` command (note: shim is a separate binary)
- [ ] Add `daemonize` and `signal-hook` dependencies to `crates/locald-shim/Cargo.toml`
- [ ] Implement `Handshake`, `ShimRequest`, `ShimResponse` types with serde
- [ ] Implement length-prefixed JSON wire format (read/write helpers)
- [ ] Bind socket before fork, print path, then daemonize
- [ ] Socket permissions: mode 0600, validate peer UID via `SO_PEERCRED`
- [ ] Request dispatcher: route to existing shim operations (hosts, cgroup, trust)
- [ ] Lifecycle: idle timeout (5 min), max lifetime (1 hour), SIGTERM handler
- [ ] PID file for duplicate detection

**Acceptance**: `sudo locald-shim serve` starts daemon, clients can connect and issue Ping.

### Phase 2: Host-Exec Configuration

**Files**: `crates/locald-core/src/config/global.rs`, `crates/locald-cli/src/utils.rs`

- [ ] Add `ContainerConfig` struct with `host_exec: Option<String>`
- [ ] Add `shim_socket: Option<PathBuf>` for socket path override
- [ ] Template substitution: replace `{command}` placeholder
- [ ] Auto-detection order: configured → flatpak-spawn → distrobox-host-exec
- [ ] Error message includes manual setup instructions

**Acceptance**: Setting `host_exec = "echo {command}"` in config causes echo output.

### Phase 3: Polkit Integration

**Files**: `assets/dev.locald.policy` (new), `crates/locald-shim/src/install.rs`

- [ ] Create polkit policy XML file
- [ ] Install policy to `/usr/share/polkit-1/actions/` during `admin setup`
- [ ] Use `pkexec locald shim serve` instead of `sudo`
- [ ] Handle polkit not installed (fall back to sudo with warning)

**Acceptance**: Running `flatpak-spawn --host pkexec locald shim serve` shows GUI dialog.

### Phase 4: Container Auto-Start

**Files**: `crates/locald-cli/src/handlers.rs`, `crates/locald-utils/src/privileged.rs`

- [ ] In `PrivilegedCapability::acquire()`: try socket first, then setuid
- [ ] `ShimClient` struct for socket communication
- [ ] Auto-start daemon if socket missing and in container
- [ ] Retry logic: wait up to 5s for daemon to become ready after spawn
- [ ] Integrate with existing `is_probably_container()` detection

**Acceptance**: `locald up` in Toolbx auto-starts host daemon and connects.

### Phase 5: Documentation and Polish

**Files**: `docs/manual/development/container-environments.md`, README

- [ ] Document `[container]` config section
- [ ] Document manual daemon start workflow
- [ ] Add troubleshooting section for common issues
- [ ] Systemd user service example (optional convenience)
- [ ] Update `locald doctor` to check socket connectivity

**Acceptance**: New user can follow docs to get locald working in Toolbx.

## Testing Strategy

### Unit Tests

- Socket protocol serialization/deserialization
- Peer credential validation
- Idle timeout logic
- Host-exec template substitution

### Integration Tests

```rust
#[test]
fn test_container_detection_enables_host_exec() {
    // Set container env vars
    // Verify host-exec path is taken for privileged ops
}

#[test]
fn test_daemon_lifecycle() {
    // Start daemon
    // Connect, send ping
    // Disconnect, wait for idle timeout
    // Verify daemon exits
}

#[test]
fn test_host_exec_template_substitution() {
    let template = "my-host-exec {command}";
    let result = substitute_template(template, "pkexec locald shim serve");
    assert_eq!(result, "my-host-exec pkexec locald shim serve");
}
```

### Manual Testing Matrix

| Environment                 | Test                                         |
| --------------------------- | -------------------------------------------- |
| Host (non-container)        | `locald up` works with setuid shim           |
| Toolbx                      | `locald up` auto-detects flatpak-spawn       |
| Distrobox                   | `locald up` auto-detects distrobox-host-exec |
| Custom host_exec config     | `locald up` uses configured template         |
| Manual daemon start         | `sudo locald shim serve` on host works       |
| Container without host-exec | Graceful error with setup instructions       |
| NFS home directory          | Socket falls back to `/run/user/$UID/`       |

## Migration Path

### From Current Implementation

1. Existing setuid shim continues to work on host (no change)
2. Container environment detection already exists
3. New: Socket daemon mode for container→host delegation
4. New: Configurable host_exec for diverse environments
5. New: Polkit policy shipped with installation

### Rollout Strategy

1. Ship daemon mode as primary container solution
2. Document manual setup for edge cases
3. Gather feedback on host_exec template usability
4. Consider additional auto-detection for new container runtimes

## References

- [Research: Host/Container Privilege Split](../research/host-container-privilege-split.md)
- [RFC 0026: Configuration Hierarchy](0026-configuration-hierarchy.md) (pinned apps concept)
- [RFC 0096: Leaf Node Axiom](../stage-4/0096-leaf-node-axiom.md)
- [RFC 0097: Strict Discovery](../stage-4/0097-strict-discovery.md)
- RFC 0131: Windows/WSL Privileged Operations (future, see "WSL Support" section)
- [Distrobox host-exec](https://distrobox.it/useful_tips/#using-host-executables-inside-the-container)
- [flatpak-spawn documentation](https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn)
