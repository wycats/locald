---
title: Ad-Hoc Execution
description: Running one-off commands and ephemeral containers.
---

While `locald.toml` defines your long-running services, development often requires running one-off tasks: database migrations, REPLs, or testing tools. `locald` provides first-class support for these "Ad-Hoc" tasks.

## The `try` Command

The `try` command lets you experiment with a command in the context of your project. It injects the same environment variables (like `$PORT` or `DATABASE_URL`) that your services get.

```bash
locald try npm run test
```

If the command is successful, `locald` will ask if you want to save it as a permanent service in your `locald.toml`.

## The `exec` Command

Use `exec` to run a command "inside" the context of an existing service. This is useful for running administrative tasks that need the exact same environment as a running service.

```bash
# Run a migration using the 'api' service's environment
locald exec api npm run migrate
```

## Ephemeral Containers

You can run OCI containers (like Docker images) directly with `locald`, without needing a separate Docker daemon running.

```bash
locald container run alpine echo "Hello World"
```

This pulls the image, unpacks it, generates an OCI runtime spec, and executes it via the `locald-shim`.

`locald-shim` executes the OCI bundle using an embedded container runtime.

### Interactive Mode

You can run interactive containers (like a shell) using the `-i` flag:

```bash
locald container run -i alpine /bin/sh
```

### Background Services

`locald` discourages running ad-hoc containers in the background ("detached mode"). If a container needs to run persistently (like a database or queue), it belongs in your `locald.toml` as a managed service.

To add a containerized service to your workspace:

```bash
locald service add container redis:7 --name redis
```

This ensures the service is:

1.  **Documented**: Visible in `locald.toml`.
2.  **Managed**: Automatically restarted if it crashes.
3.  **Observable**: Logs are captured in the dashboard.
