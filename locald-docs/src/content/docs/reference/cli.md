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
