# Architecture: Configuration

> **Core focus**: Configuration should keep the default workflow minimal and predictable.

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
# Whether to attempt binding to privileged ports (80/443).
# If true, failure to bind will error. Use sandbox mode for unprivileged testing.
privileged_ports = true
```

### Container Configuration

Container-specific configuration was removed with RFC 0138. `locald` runs on the host only.

See [Container Development Environments](../development/container-environments.md) for the supported host-first patterns.
