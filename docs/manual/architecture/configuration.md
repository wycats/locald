# Architecture: Configuration

This document describes how `locald` manages configuration, state, and project tracking.

## 1. Configuration Hierarchy

Configuration is resolved from multiple sources in a specific order (highest priority last):

1.  **Global**: `~/.config/locald/config.toml` (User defaults).
2.  **Context**: `.locald.toml` in parent directories (Directory-specific defaults).
3.  **Workspace**: `locald.workspace.toml` or Git Root (Shared resources, env vars).
4.  **Project**: `locald.toml` (Service-specific settings).

### In-Repo Configuration

The primary configuration lives in `locald.toml` within the project root. This ensures configuration is versioned with the code ("Infrastructure as Code").

### Typed Configuration

Service configuration uses a typed enum approach (e.g., `type = "exec"`, `type = "postgres"`) to allow different schemas for different service types.

## 2. State Persistence

The daemon persists its runtime state to disk to survive restarts.

- **Location**: `~/.local/share/locald/state.json` (XDG Data Home).
- **Content**: List of running services, their PIDs, and their last known status.
- **Usage**: On startup, the daemon reads this file to identify "zombie" processes that need to be cleaned up before restarting services.

## 3. Project Registry

`locald` maintains a centralized registry of known projects.

- **Location**: `~/.local/share/locald/registry.json`.
- **Purpose**: Tracks all projects that have been registered with `locald`, allowing for features like "Always Up" (starting services automatically on daemon boot) and a global dashboard view.

## 4. Gitignore Automation

To prevent local state (logs, temporary files) from being committed, `locald` can automatically append `.locald/` to the project's `.gitignore` file.

## 5. Global Configuration Sections

The global configuration file (`~/.config/locald/config.toml`) supports these sections:

### `[server]`

Server behavior settings:

```toml
[server]
# Whether to attempt binding to privileged ports (80/443)
privileged_ports = true

# Whether to fallback to unprivileged ports (8080/8443) if privileged fail
fallback_ports = true
```

### `[container]`

Settings for container development environments (Toolbx, Distrobox, etc.):

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

**`host_exec`**: A template string for executing commands on the host from inside a container. The `{command}` placeholder is replaced with the actual command. If not set, `locald` auto-detects available mechanisms (`flatpak-spawn`, `distrobox-host-exec`).

**`shim_socket`**: Override the default socket path (`~/.locald/shim.sock`). Useful when your home directory is on a network filesystem that doesn't support Unix sockets.

See [Container Development Environments](../development/container-environments.md) for complete documentation.
