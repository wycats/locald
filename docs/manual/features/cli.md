# CLI Reference

`locald` provides a powerful CLI for managing your development environment.

> **Core Focus**: The primary workflow is `locald up` → stable domains/HTTPS → monitor/logs. Optional or experimental commands should be treated as secondary.

## Global Options

### `--sandbox <name>`

Run commands in an isolated sandbox environment. Useful for testing or running multiple instances.

```bash
locald --sandbox mytest up
```

## Core Commands

### `locald up [path]`

Starts the `locald` daemon and the services defined in your `locald.toml`.

```bash
# Start services in current directory
locald up

# Start services from a specific path
locald up ./my-project

# Show verbose output during startup
locald up --verbose
```

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

### `locald status`

List running services and their status.

```bash
# Human-readable output
locald status

# Machine-readable JSON
locald status --json
```

### `locald stop [name]`

Stop a running service. If no name is provided, stops all services in the current project.

```bash
# Stop a specific service
locald stop myproject:web

# Stop all services in current directory's project
locald stop

# Machine-readable JSON output
locald stop --json
```

### `locald restart <name>`

Restart a running service.

```bash
locald restart myproject:web
```

### `locald logs [service]`

Stream logs from services.

```bash
# Stream all logs
locald logs

# Stream logs from a specific service
locald logs myproject:web

# Follow log output (like tail -f)
locald logs --follow
locald logs -f myproject:web
```

### `locald dashboard`

Open the web dashboard in your default browser.

```bash
locald dashboard
```

## Project Initialization

### `locald init`

Initialize a new locald project in the current directory.

```bash
# Interactive initialization
locald init

# Accept all defaults
locald init --yes

# Initialize from a distribution (experimental)
locald init --from-distribution ./my-distribution.locald-distribution

# Initialize with specific project name
locald init --name my-project --target ./my-project
```

**Options:**
- `--from-distribution <source>` - Initialize from a distribution archive (experimental)
- `--name <name>` - Project name
- `--target <path>` - Target directory
- `--no-scaffold` - Skip scaffold files
- `--offline` - Use only bundled plugins
- `-y, --yes` - Accept all defaults
- `-v, --verbose` - Show detailed steps

## Ad-Hoc Execution

### `locald try`

Run an ad-hoc host command with a dynamically assigned `$PORT` injected into the environment. This is useful for quick experiments before you have a `locald.toml`.

```bash
# Run a simple HTTP server on an available port
locald try python3 -m http.server $PORT
```

When the command exits, you'll be prompted to save it as a permanent service.

### `locald run` / `locald exec`

Run a command within the context of a defined service. This injects the service's environment variables and network context.

```bash
# Run a database migration using the 'web' service's environment
locald run web -- rails db:migrate

# The 'exec' alias works the same way
locald exec web -- rails db:migrate
```

Note: This runs the command _locally_ on your machine (as a host process), but with the environment configuration of the service.

## Service Management

### `locald add`

Shortcut to add a service to `locald.toml`.

```bash
# Add a command as a service
locald add npm start

# Add with a custom name and port
locald add --name api --port 3000 npm start

# Add the last successful 'try' command
locald add last
```

### `locald service add`

Add a new service with a specific type.

#### `locald service add exec`

Add a shell command service.

```bash
locald service add exec npm start
locald service add exec --name api --port 3000 npm run dev
```

#### `locald service add postgres`

Add a managed Postgres database service.

```bash
locald service add postgres db
locald service add postgres --version 15 db
```

#### `locald service add container`

Add a container-based service.

```bash
locald service add container redis:7
locald service add container --name cache --container-port 6379 redis:7
```

#### `locald service add site`

Add a static site service.

```bash
locald service add site ./public
locald service add site --name docs --build "npm run build" ./dist
```

### `locald service reset <name>`

Reset a service by stopping it, wiping its data (when applicable), and restarting it.

```bash
locald service reset myproject:db
```

This is primarily used for managed data services (like Postgres) when you need a clean state.

## Configuration

### `locald config show`

Show the current configuration.

```bash
# Show configuration
locald config show

# Show where each value came from
locald config show --provenance
```

## Registry Management

The registry tracks all projects known to locald.

### `locald registry list`

List all registered projects.

### `locald registry pin [path]`

Pin a project to keep it running.

```bash
locald registry pin
locald registry pin ./my-project
```

### `locald registry unpin [path]`

Unpin a project.

### `locald registry clean`

Remove non-existent projects from the registry.

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
- Cgroup readiness
- System requirements

See [locald doctor reference](doctor.md) for detailed documentation.

### `locald ping`

Check if the daemon is running and responsive.

```bash
locald ping
```

### `locald debug port <port>`

Check which process is listening on a specific port.

```bash
locald debug port 3000
```

## Server Lifecycle

`locald` manages a background daemon. Most commands will start it automatically if it is not already running.

### `locald server start`

Start the daemon in the foreground.

### `locald server shutdown`

Gracefully shut down the running daemon.

### `locald server restart`

Restart the daemon. The CLI may also restart the daemon automatically if it detects a version mismatch.

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

### `locald admin sync-hosts`

Sync the hosts file with running services. Requires root privileges.

```bash
sudo locald admin sync-hosts
```

### `locald trust`

Install the locald Root CA into the system trust store for HTTPS support.

```bash
locald trust
```

### `locald selfupgrade`

Self-upgrade locald to a newer version.

```bash
# Upgrade to latest
locald selfupgrade

# Check for updates without installing
locald selfupgrade --check

# Install specific version
locald selfupgrade --version 0.5.0
```

## AI Integration

### `locald ai schema`

Get the JSON schema for `locald.toml`. Useful for editor integration and AI assistants.

### `locald ai context`

Get the current system context (running services, configuration, etc.) as JSON.

## Static File Server

### `locald serve [path]`

Serve a directory via HTTP. Useful for quick static file serving.

```bash
# Serve current directory on port 8080
locald serve

# Serve a specific directory
locald serve ./public

# Use custom port and bind address
locald serve --port 3000 --bind 127.0.0.1 ./dist
```

## Experimental Commands

These commands require specific feature flags and are not enabled in stable builds.

### `locald build` (experimental-cnb)

Build a project using Cloud Native Buildpacks.

```bash
locald build
locald build --builder heroku/builder:22 --buildpack heroku/nodejs
```

### `locald container run` (experimental-containers)

Run an ephemeral container.

```bash
locald container run alpine
locald container run -i alpine /bin/sh
locald container run -d redis:7
```

### `locald plugin` (experimental-plugins)

Manage WASM plugins.

- `locald plugin install <source>` - Install a plugin
- `locald plugin inspect <plugin>` - Inspect a plugin
- `locald plugin validate <plugin>` - Validate a plugin
- `locald plugin create` - Create a plugin package

### `locald distribution` (experimental-plugins)

Manage distributions.

- `locald distribution create` - Create a distribution archive
