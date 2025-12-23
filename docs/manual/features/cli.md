# CLI Reference

`locald` provides a powerful CLI for managing your development environment.

## Core Commands

### `locald up`

Starts the `locald` daemon and the services defined in your `locald.toml`.

It displays a dynamic progress UI that shows the status of builds and service startups.

- **Building**: Shows build progress for services that require it.
- **Starting**: Shows health check status.
- **Ready**: Indicates when services are fully up and running.

If a step fails, the UI will persist the error details for debugging.

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
