# Design: Advanced Service Configuration

**Goal**: Support a wider range of service types and configurations to match or exceed Foreman/Heroku capabilities.

## Service Types

- **Goal**: Support non-network services (workers, cron jobs) that do not bind a port.
- **Mechanism**: Add `type = "worker"` to service config. Skip port assignment and TCP probe for workers.

## Procfile Support

- **Goal**: Drop-in compatibility with existing `Procfile`-based projects (Foreman, Heroku).
- **Mechanism**:
  - Add a parser for `Procfile`.
  - If `locald.toml` is missing but `Procfile` exists, auto-generate or run directly from it.
  - Map `web` process type to a service with `$PORT`.

## Port Discovery

- **Goal**: Automatically detect the port a service is listening on, even if it ignores the `PORT` environment variable (e.g. Vite).
- **Mechanism**: Scan `/proc/net/tcp` or use `lsof` logic to find listening sockets owned by the service's PID (or process group). Update the service's port in `locald` state dynamically.

## Advanced Health Checks

- **Goal**: Allow `depends_on` to wait until a service is actually ready.
- **Mechanism**: HTTP probe, TCP connect, or command execution.

## Foreman Parity / Gap Analysis

- **Goal**: Ensure `locald` is a viable replacement for `foreman`, `overmind`, and `hivemind`.
- **Missing Features**:
  - `.env` file loading (standard in Foreman).
  - `run` command (one-off tasks with loaded env).
  - Signal handling customization.
  - Concurrency control (`-c web=2`).
