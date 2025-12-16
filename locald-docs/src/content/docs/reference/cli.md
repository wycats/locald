---
title: CLI Reference
---

`locald` provides a powerful CLI for managing your development environment.

## Core Commands

### `locald up`

Starts the `locald` daemon and the services defined in your `locald.toml`.

It displays a dynamic progress UI that shows the status of builds and service startups.

- **Building**: Shows build progress for services that require it.
- **Starting**: Shows health check status.
- **Ready**: Indicates when services are fully up and running.

If a step fails, the UI will persist the error details for debugging.

### `locald down`

Stops all running services and the daemon.

## Ad-Hoc Execution

### `locald try`

Run a command in a temporary, isolated environment. This is useful for trying out tools or running one-off scripts without installing them globally.

```bash
# Run a python script without installing python
locald try python:3.9 python my_script.py
```

### `locald run`

Run a command within the context of a defined service. This injects the service's environment variables and network context.

```bash
# Run a database migration using the 'web' service's environment
locald run web -- rails db:migrate
```

Note: This runs the command _locally_ on your machine (as a host process), but with the environment configuration of the service.
