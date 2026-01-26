# CLI Reference

`locald` provides a powerful CLI for managing your development environment.

> **Core Focus**: The primary workflow is `locald up` → stable domains/HTTPS → monitor/logs. Optional or experimental commands should be treated as secondary.

## Core Commands

### `locald up`

Starts the `locald` daemon and the services defined in your `locald.toml`.

It displays a dynamic progress UI that shows the status of builds and service startups.

- **Building**: Shows build progress for services that require it.
- **Starting**: Shows health check status.
- **Ready**: Indicates when services are fully up and running.

If a step fails, the UI will persist the error details for debugging.

### `locald monitor`

Open the TUI service monitor for running services.

```bash
locald monitor
```

The monitor shows service status, recent logs, and health check state. Press `q` to exit.

See RFC 0016 for historical context.

## Ad-Hoc Execution

### `locald try`

Run an ad-hoc host command with a dynamically assigned `$PORT` injected into the environment. This is useful for quick experiments before you have a `locald.toml` (or when you don’t want to add a service).

```bash
# Run a simple HTTP server on an available port
locald try python3 -m http.server $PORT
```

### `locald run`

Run a command within the context of a defined service. This injects the service's environment variables and network context.

```bash
# Run a database migration using the 'web' service's environment
locald run web -- rails db:migrate
```

Note: This runs the command _locally_ on your machine (as a host process), but with the environment configuration of the service.

## Service Management

### `locald service reset`

Reset a service by stopping it, wiping its data (when applicable), and restarting it.

```bash
locald service reset <service>
```

This is primarily used for managed data services (like Postgres) when you need a clean state.

See RFC 0029 for historical context.

## Diagnostics

### `locald doctor`

Diagnose host readiness for running `locald` and print actionable fixes.

```bash
# Human-readable output
locald doctor

# Machine-readable JSON (for CI)
locald doctor --json

# Verbose mode with extra details
locald doctor --verbose
```

Checks:

- Shim availability and permissions
- Socket connectivity (in container environments)
- Cgroup readiness

See [locald doctor reference](doctor.md) for detailed documentation.

## Server Lifecycle

`locald` manages a background daemon. Most commands will start it automatically if it is not already running.

### `locald server start`

Start the daemon in the foreground.

### `locald server shutdown`

Gracefully shut down the running daemon.

### `locald server restart`

Restart the daemon. The CLI may also restart the daemon automatically if it detects a version mismatch.

See RFC 0044 for historical context.

## Administration

### `locald admin setup`

Install privileged components (setuid shim, cgroup root).

```bash
sudo locald admin setup
```

This command:

1. Extracts the embedded shim binary
2. Sets up permissions (root-owned, setuid)
3. Configures cgroups for process isolation
4. Installs polkit policy for GUI authentication

### `locald-shim serve`

Start the shim daemon for container environments.

```bash
# Run in background (default)
sudo locald-shim serve

# Run in foreground (for debugging)
sudo locald-shim serve --foreground

# Custom socket path
sudo locald-shim serve --socket /run/user/1000/locald/shim.sock
```

The daemon:

- Listens on `~/.locald/shim.sock`
- Handles privileged operations (hosts sync, port binding, cgroups)
- Auto-exits after 5 minutes idle or 1 hour max lifetime

See [Container Development Environments](../development/container-environments.md) for the complete guide.
