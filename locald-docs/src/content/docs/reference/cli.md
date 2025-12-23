---
title: CLI Reference
---

`locald` provides a powerful CLI for managing your development environment.

For the canonical taught vocabulary (verbs/nouns and stability rules), see RFC 0114 “Surface Contract v1”.

## Core Commands

For a complete list of commands and flags, run:

```bash
locald --help
```

### `locald init`

Initialize a new `locald` project by interactively generating a `locald.toml`.

### `locald up`

Starts the `locald` daemon and the services defined in your `locald.toml`.

It displays a dynamic progress UI that shows the status of builds and service startups.

- **Building**: Shows build progress for services that require it.
- **Starting**: Shows health check status.
- **Ready**: Indicates when services are fully up and running.

If a step fails, the UI will persist the error details for debugging.

### `locald stop`

Stop a running service. If no service name is provided, stops all services defined in `locald.toml` for the current project.

### `locald server shutdown`

Shutdown the running daemon.

### `locald status`

List running services.

### `locald logs`

Stream logs from services.

### `locald restart`

Restart a running service.

### `locald monitor`

Open the terminal UI (TUI) to monitor running services.

### `locald dashboard`

Open the dashboard in your default browser.

## Diagnostics

### `locald doctor`

Diagnose whether your machine is ready to run `locald` (especially features that require the privileged `locald-shim` and cgroup-based cleanup).

Typical output includes:

- Whether a privileged shim is installed and usable
- Whether cgroup v2 is available and the locald cgroup root is established
- Suggested next steps (usually `sudo locald admin setup`)

`locald doctor` may also surface **integration availability** (for example, whether a legacy Docker daemon integration is reachable). For details, see [Integrations](/reference/integrations).

Exit code:

- `0` when critical checks pass
- non-zero when critical checks fail

Flags:

- `--json` prints a machine-readable report (useful in CI)
- `--verbose` includes additional evidence details

```bash
locald doctor

# CI-friendly
locald doctor --json
```

## Ad-Hoc Execution

### `locald try`

Run a scratch command on your host with a dynamically assigned `$PORT` injected into the environment.

```bash
# Run a command with a dynamic PORT and save it later if desired
locald try python3 -m http.server $PORT
```

### `locald run`

Run a one-off task within the context of a defined service. This injects the service's environment variables (DB URL, etc.) and network context.

Note: `locald exec` currently exists as an alias for `locald run`, but is reserved for a future “attach to an existing runner” workflow. The docs teach `run` as the canonical spelling.

```bash
# Run a database migration using the 'web' service's environment
locald run web -- rails db:migrate
```

Note: This runs the command _locally_ on your machine (as a host process), but with the environment configuration of the service.

### `locald trust`

Install the local Certificate Authority into the system trust store so HTTPS works cleanly.

### `locald admin setup`

Perform one-time privileged setup (install/configure `locald-shim`, cgroups, etc.).
