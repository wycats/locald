---
title: CLI Reference
---

`locald` provides a CLI for the core development loop: define a project in `locald.toml`, run `locald up`, then use stable local domains, logs, monitor, and the dashboard while the daemon supervises your services.

> **Core focus**: The primary workflow is `locald up` → stable domains/HTTPS → monitor/logs. Optional, experimental, and plumbing commands are secondary.

For the canonical taught vocabulary (verbs/nouns and stability rules), see RFC 0114 “Surface Contract v1”. For the exact generated command surface, run `locald --help` or a subcommand's `--help` output.

## Core workflow

### `locald init`

Initialize a new `locald` project by generating a `locald.toml`.

Common flags:

- `--name <name>` sets the project name.
- `--target <path>` sets the target directory.
- `--no-scaffold` skips scaffold files.
- `--offline` uses only bundled plugins.
- `-y, --yes` accepts defaults without prompting.
- `-v, --verbose` shows detailed setup steps.

```bash
locald init
locald init --name my-app --target ./my-app
locald init --yes
```

### `locald up [path]`

Start the daemon if needed and register the current project. If `path` is omitted, `locald` uses the current directory when it contains a `locald.toml`.

`locald up` displays startup progress for builds, service launches, and health checks. If startup fails, the progress UI keeps error details visible for debugging.

```bash
locald up
locald up ./my-project
locald up --verbose
```

### `locald status`

List running services.

```bash
locald status
locald status --json
```

### `locald logs [service]`

Stream service logs. Pass a service name to focus on one service, or `--follow` / `-f` to keep streaming.

```bash
locald logs
locald logs web
locald logs --follow
```

### `locald stop [name]`

Stop a running service. If no service name is provided, `locald` stops all services defined in `locald.toml` for the current project.

```bash
locald stop
locald stop web
locald stop --json
```

### `locald restart <name>`

Restart a running service.

```bash
locald restart web
locald restart web --json
```

### `locald monitor`

Open the terminal UI (TUI) for monitoring running services. The monitor is useful for inspecting service state and logs without opening the web dashboard.

```bash
locald monitor
```

### `locald dashboard`

Open the web dashboard in your default browser.

```bash
locald dashboard
```

## Diagnostics and setup

### `locald doctor`

Diagnose whether your machine is ready to run `locald`, especially features that require privileged setup such as the `locald-shim` and cgroup-based cleanup.

Typical output includes:

- whether the privileged shim is installed and usable,
- whether cgroup v2 is available and the `locald` cgroup root is established,
- suggested next steps, usually `sudo locald admin setup`.

`locald doctor` may also surface integration availability, such as whether a legacy Docker daemon integration is reachable. For details, see [Integrations](/reference/integrations/).

```bash
locald doctor
locald doctor --json
locald doctor --verbose
```

### `locald admin setup`

Perform one-time privileged setup. This is the canonical remediation when `doctor` reports missing privileged readiness.

```bash
sudo locald admin setup
```

### `locald trust`

Install the local Certificate Authority into the system trust store so HTTPS works cleanly for local domains.

```bash
locald trust
```

## Ad-hoc and service-context commands

### `locald try [command]...`

Run a scratch host command in the current terminal with a dynamically assigned `$PORT` injected into the environment. When the command exits, `locald` can prompt to save it as a permanent service in `locald.toml`.

```bash
locald try python3 -m http.server $PORT
```

### `locald run <service> -- <command>...`

Run a one-off task in the context of a defined service. This injects that service's environment variables and network context, which is useful for migrations, consoles, and other administrative commands.

`locald exec` currently exists as an alias for `locald run`, but the docs teach `run` as the canonical spelling.

```bash
locald run web -- rails db:migrate
locald run api -- npm test
```

## Adding services

### `locald add`

Add a service to `locald.toml` using the shortcut form. This can add an exec service, the last successful `try` command, or a managed Postgres service.

```bash
locald add npm start
locald add --name api --port 3000 npm run dev
locald add last
locald add postgres db
```

### `locald service add exec`

Add a shell command service.

```bash
locald service add exec npm start
locald service add exec --name api --port 3000 npm run dev
```

### `locald service add postgres`

Add a managed Postgres service.

```bash
locald service add postgres db
locald service add postgres --version 15 db
```

Typed Postgres services expose a connection URL for dependents and dashboard inspection rather than a browser URL.

### `locald service add container`

Add a container service.

```bash
locald service add container redis:7
locald service add container --name cache --container-port 6379 redis:7
```

### `locald service add site`

Add a static site service.

```bash
locald service add site ./public
locald service add site --name docs --build "npm run build" ./dist
```

### `locald service reset <name>`

Reset a service by stopping it, wiping its data when applicable, and restarting it. This is primarily useful for managed data services.

```bash
locald service reset db
```

## Reference and maintenance commands

### `locald config show`

Show the current configuration.

```bash
locald config show
locald config show --provenance
```

### `locald registry list|pin|unpin|clean`

Inspect and maintain the project registry.

```bash
locald registry list
locald registry pin ./my-project
locald registry unpin ./my-project
locald registry clean
```

### `locald serve [path]`

Serve a directory over HTTP for quick static-file checks.

```bash
locald serve
locald serve ./public --port 3000 --bind 127.0.0.1
```

### `locald tray start|stop|status|restart`

Manage the optional desktop tray/menu-bar status agent. The tray agent shows daemon status, service health, and quick actions such as opening the dashboard, restarting all services, running setup when host readiness is missing, and quitting the tray agent without stopping the daemon or services.

```bash
locald tray start
locald tray status
locald tray stop
locald tray restart
```

Supported backends:

- macOS menu bar sessions use the LaunchAgent installed by `sudo locald admin setup`.
- Linux desktop sessions use StatusNotifier/AppIndicator support. GNOME may require AppIndicator/StatusNotifier support to be installed or enabled.

On Linux, run `locald tray start` from your desktop user session, not through `sudo`. Headless shells, missing D-Bus session buses, and desktops without a visible tray host fail with an explicit diagnostic instead of silently starting an invisible agent. `locald tray status` reports whether the agent is installed/running and which `locald` daemon path is pinned.

For platform requirements, see [Desktop Tray](/reference/desktop-tray/).

## Not taught as primary workflows

Some commands are available for contributors, automation, diagnostics, or feature-gated experiments but are not part of the stable front-door workflow. Use `locald --help` for the complete generated list in your build.

- `locald server ...` is daemon lifecycle plumbing; normal commands start the daemon when needed.
- `locald project ...` and `locald debug ...` are contributor or integration surfaces.
- `locald build`, `locald container ...`, `locald plugin ...`, and `locald distribution ...` are gated by experimental build features when present.
- `--sandbox` is a contributor/CI isolation option, not the default user setup path.

The removed `locald down` command is intentionally not documented. Use `locald stop` for the current service-stop workflow.
