---
title: CLI Reference
description: Complete command reference for the locald CLI.
---

The `locald` CLI is the primary interface for interacting with the daemon.

## Usage

```bash
locald <COMMAND> [ARGS]
```

## Core Commands

| Command   | Description                                                                                   |
| :-------- | :-------------------------------------------------------------------------------------------- |
| `init`    | Interactively creates a `locald.toml` file in the current directory.                          |
| `server`  | Starts the `locald-server` daemon in the background. Safe to run multiple times (idempotent). |
| `ping`    | Checks if the daemon is running and reachable via IPC.                                        |
| `status`  | Lists all currently running services and their status (PID, Port, etc.).                      |
| `monitor` | Opens a TUI dashboard to view running services in real-time.                                  |
| `logs`    | Streams logs from running services.                                                           |

## Project Commands

These commands operate on the project defined in the `locald.toml` of the current directory.

| Command | Description                                                 |
| :------ | :---------------------------------------------------------- |
| `start` | Registers and starts the services defined in `locald.toml`. |
| `stop`  | Stops the services defined in `locald.toml`.                |

## Admin Commands

These commands require elevated privileges (`sudo`) to modify system configuration.

| Command            | Description                                                                                                              |
| :----------------- | :----------------------------------------------------------------------------------------------------------------------- |
| `admin setup`      | Grants `cap_net_bind_service` to the `locald-server` binary, allowing it to bind port 80.                                |
| `admin sync-hosts` | Updates `/etc/hosts` to map project domains (e.g., `app.local`) to `127.0.0.1`. Only touches the `# BEGIN locald` block. |

## Examples

**Start the server:**

```bash
locald server
```

**Start a project:**

```bash
cd ~/my-project
locald start
```

**Check status:**

```bash
locald status
```
