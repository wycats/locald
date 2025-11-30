---
title: CLI Reference
description: Command reference for the locald CLI.
---

## Global Commands

### `locald server`

Starts the `locald-server` daemon in the background.

### `locald ping`

Checks if the daemon is reachable.

### `locald status`

Lists all running processes managed by the daemon.

### `locald stop <name>`

Stops a specific service by name.

## Project Commands

These commands must be run from a directory containing a `locald.toml`.

### `locald start`

Starts the service defined in the current directory's `locald.toml`.

### `locald stop`

Stops the service defined in the current directory's `locald.toml`.

## Admin Commands

### `locald admin setup`

Applies necessary capabilities to the `locald-server` binary to allow binding to privileged ports (like port 80).
Requires `sudo`.

```bash
sudo locald admin setup
```

### `locald admin sync-hosts`

Updates your system's hosts file (`/etc/hosts` or equivalent) to map configured domains to `127.0.0.1`.
Requires `sudo`.

```bash
sudo locald admin sync-hosts
```
