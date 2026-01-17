# Container Development Environments

`locald` is designed to "just work" in **containerized development environments** like [Toolbx](https://containertoolbx.org/) and [Distrobox](https://distrobox.it/).

This guide covers how to set up and use `locald` when your development environment lives inside a container while your host OS provides privileged operations.

## Overview

Modern immutable/atomic Linux distributions (Fedora Silverblue, Fedora Kinoite, etc.) encourage running development tools inside containers. `locald` supports this workflow through a **host shim daemon** that handles privileged operations on behalf of the container.

```
┌─────────────────────────────────────────────────────────────────────────┐
│ HOST OS                                                                 │
│                                                                         │
│   locald-shim (daemon)                                                  │
│   ├── Listens on: ~/.locald/shim.sock                                   │
│   ├── Manages: /etc/hosts, cgroups, privileged ports, cert trust        │
│   └── Started via: locald-shim serve                                    │
│                                                                         │
│   [Socket is in $HOME, shared between host and container]               │
│                                                                         │
│   ┌────────────────────────────────────────────────────────────────┐    │
│   │ TOOLBX / DISTROBOX CONTAINER                                    │    │
│   │                                                                  │    │
│   │   locald server (runs your dev services)                        │    │
│   │   ├── Connects to host shim daemon via socket                   │    │
│   │   ├── Delegates privileged ops to the shim                      │    │
│   │   └── Services run here with container's environment            │    │
│   │                                                                  │    │
│   │   Your code, compilers, runtimes live here                      │    │
│   └────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────┘
```

### Key Insight: Network Namespace is Shared

Toolbx and Distrobox share the host's network namespace. This means:

- Containers can bind to host ports directly (no port forwarding needed)
- `localhost` in the container IS `localhost` on the host
- Browsers on the host can reach services in the container via `127.0.0.1`

**We only need the shim for**:

1. `/etc/hosts` synchronization (for `*.localhost` domains)
2. cgroup-based process isolation
3. Privileged port binding (< 1024)
4. System trust store modification (HTTPS certificates)

## Quick Start

### Step 1: Install locald on the Host

On your host OS (outside any container):

```bash
# Install locald
cargo install locald

# Run privileged setup (installs setuid shim, configures cgroups)
sudo locald admin setup
```

### Step 2: Start the Shim Daemon on the Host

Before using `locald` in a container, start the shim daemon on the host:

```bash
# Option A: Run in foreground (good for testing)
sudo locald-shim serve

# Option B: Run in background
sudo locald-shim serve &
```

The daemon listens on `~/.locald/shim.sock` which is accessible from inside Toolbx/Distrobox containers (because they share your home directory).

### Step 3: Use locald in Your Container

```bash
# Enter your container
toolbox enter  # or: distrobox enter my-container

# Navigate to your project
cd ~/Code/my-project

# Start your services
locald up
```

`locald` automatically detects the container environment and connects to the host shim daemon via the Unix socket.

## How It Works

### Privilege Acquisition Strategy

When you run `locald up` inside a container:

1. **Socket-first**: `locald` tries to connect to `~/.locald/shim.sock`
2. **Auto-start**: If the socket doesn't exist but a host-exec mechanism is available, `locald` tries to start the daemon on the host
3. **Graceful degradation**: If no socket is available, `locald` proceeds without privileged features

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
│             │  │ 3. Fail gracefully with warning          │
└─────────────┘  └─────────────────────────────────────────┘
```

### What Works Without Privileged Features

When the shim is unavailable, `locald` runs in **degraded mode**:

| Feature                             | Status   |
| ----------------------------------- | -------- |
| Running services from `locald.toml` | ✅ Works |
| Port discovery and assignment       | ✅ Works |
| Service dependencies                | ✅ Works |
| Dashboard                           | ✅ Works |
| Logs and monitoring                 | ✅ Works |

### What Requires the Shim Daemon

| Feature                         | Requires Shim |
| ------------------------------- | ------------- |
| Privileged ports (80, 443)      | Yes           |
| `/etc/hosts` auto-sync          | Yes           |
| cgroup-based process isolation  | Yes           |
| HTTPS with system-trusted certs | Yes           |

## Configuration

### The `[container]` Section

You can configure container-specific behavior in your global config (`~/.config/locald/config.toml`) or per-project `locald.toml`:

```toml
[container]
# Template for running commands on the host from inside a container.
# {command} is replaced with the actual command to run.
#
# Examples:
#   host_exec = "flatpak-spawn --host {command}"
#   host_exec = "distrobox-host-exec {command}"
#   host_exec = "ssh myhost {command}"
#
# If not set, auto-detection is attempted.
host_exec = "flatpak-spawn --host {command}"

# Override the socket path for the shim daemon.
# Defaults to ~/.locald/shim.sock
# Use this if your home directory is on NFS or another filesystem
# that doesn't support Unix sockets.
shim_socket = "/run/user/1000/locald/shim.sock"
```

### Host-Exec Templates

The `host_exec` template uses `{command}` as a placeholder for the actual command to run on the host:

| Environment     | Template                                    |
| --------------- | ------------------------------------------- |
| Toolbx          | `flatpak-spawn --host {command}` (auto-detected) |
| Distrobox       | `distrobox-host-exec {command}` (auto-detected) |
| SSH to host     | `ssh hostname {command}`                    |
| Custom          | `your-host-exec-wrapper {command}`          |

**Auto-detection**: If `host_exec` is not set, `locald` checks for:
1. `flatpak-spawn` (available in Toolbx)
2. `distrobox-host-exec` (available in Distrobox)

## Manual Daemon Management

### Starting the Daemon

```bash
# On the host (outside container):

# Foreground mode (for debugging)
sudo locald-shim serve --foreground

# Background mode (default)
sudo locald-shim serve
```

### Checking Daemon Status

```bash
# Check if the socket exists
ls -la ~/.locald/shim.sock

# Check the PID file
cat ~/.locald/shim.pid

# Use locald doctor (from inside container)
locald doctor
```

### Stopping the Daemon

The daemon automatically shuts down after:
- **Idle timeout**: 5 minutes after the last client disconnects
- **Max lifetime**: 1 hour (ensures fresh binary after updates)

To stop it manually:
- Send `SIGTERM` or `SIGINT` to the daemon process
- Remove the socket file: `rm ~/.locald/shim.sock`

## Troubleshooting

### "locald-shim is not available as a privileged helper"

This warning appears when `locald` can't connect to the shim daemon. To fix:

1. **Start the daemon on the host**:
   ```bash
   # On the host (outside container)
   sudo locald-shim serve
   ```

2. **Verify the socket exists**:
   ```bash
   ls -la ~/.locald/shim.sock
   ```

3. **Run `locald doctor`** to diagnose:
   ```bash
   locald doctor
   ```

### Socket Connection Refused

If the socket exists but connections fail:

1. **Check if the daemon is running**:
   ```bash
   cat ~/.locald/shim.pid
   ps aux | grep locald
   ```

2. **Check socket permissions**:
   ```bash
   ls -la ~/.locald/shim.sock
   # Should be: srw------- (mode 0600)
   ```

3. **Restart the daemon**:
   ```bash
   rm ~/.locald/shim.sock
   sudo locald-shim serve
   ```

### Permission Denied on Socket

The socket uses `SO_PEERCRED` to verify client UID. This error means the connecting user doesn't match the daemon's expected user.

**Solution**: The daemon must be started by (or configured for) the same user that runs `locald` in the container.

### Home Directory on NFS

Unix sockets don't work on network filesystems. If your `$HOME` is on NFS:

```toml
# ~/.config/locald/config.toml
[container]
shim_socket = "/run/user/1000/locald/shim.sock"
```

Then ensure the daemon is started with the same override:
```bash
sudo locald-shim serve --socket /run/user/1000/locald/shim.sock
```

### Services Can't Bind to Port 80/443

Privileged port binding requires the shim. Either:

1. **Start the shim daemon** (recommended)
2. **Use high ports** in your `locald.toml`:
   ```toml
   [services.web]
   type = "exec"
   command = "npm start"
   port = 8080  # Instead of 80
   ```

### Container Can't See the Socket

Toolbx and Distrobox bind-mount your home directory. If the socket isn't visible:

1. **Check the socket path**: The socket must be under `$HOME` (or a path shared with the container)
2. **Verify bind mounts**: `mount | grep home`

### Auto-Start Fails

If `locald` can't auto-start the daemon:

1. **Configure `host_exec` explicitly**:
   ```toml
   [container]
   host_exec = "flatpak-spawn --host {command}"
   ```

2. **Start the daemon manually** on the host before running `locald` in the container

## Systemd User Service (Optional)

For convenience, you can set up a systemd user service:

```bash
# On the host
mkdir -p ~/.config/systemd/user

cat > ~/.config/systemd/user/locald-shim.service << 'EOF'
[Unit]
Description=locald shim daemon
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/pkexec %h/.cargo/bin/locald-shim serve --foreground
Restart=on-failure

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now locald-shim
```

**Note**: This requires polkit to be configured for passwordless access, or you'll get a prompt on each login.

## Testing the Workflow

```bash
# On host: ensure shim is installed
sudo locald admin setup

# On host: start the daemon
sudo locald-shim serve &

# Inside container: verify connectivity
toolbox run locald doctor

# Inside container: start your project
toolbox run bash -c "cd ~/Code/my-project && locald up"
```

The `locald doctor` command reports:
- Whether the socket connection is working
- Privileged feature availability
- Cleanup mode (enabled vs degraded)

## Security Considerations

### Socket Security

- The socket is mode `0600` (owner read/write only)
- Client UID is validated via `SO_PEERCRED`
- Only the user who started the daemon can connect

### Daemon Lifecycle

- **Idle timeout**: 5 minutes (prevents persistent root processes)
- **Max lifetime**: 1 hour (ensures fresh binaries after updates)
- The daemon never executes the `locald` binary (Leaf Node Axiom)
