# Phase 23 Walkthrough: Advanced Service Configuration

## Goal

The goal of this phase was to support a wider range of service types and configurations to match or exceed Foreman/Heroku capabilities. This includes supporting non-network "worker" services, parsing `Procfile`s, auto-discovering ports, implementing advanced health checks (HTTP probes, shell commands), and supporting `.env` files and custom stop signals.

## Key Changes

### 1. Service Types & Workers

We introduced a `type` field to `ServiceConfig` (defaulting to `exec`). We added explicit support for `worker` services, which are processes that do not bind to a port (e.g., background job processors).

- **Schema**: `locald-core/src/config.rs` updated to include `ServiceType` enum.
- **Logic**: `locald-server` skips port assignment and TCP probing for services explicitly marked as `type = "worker"`.

### 2. Procfile Support

We added support for parsing `Procfile`s, a standard format for defining process types in Heroku-like environments.

- **Implementation**: `locald-core/src/config.rs` now includes a `Procfile` parser.
- **Integration**: When `locald init` is run, or when `locald` starts without a config, it checks for a `Procfile` and auto-generates a configuration in memory.

### 3. Port Discovery

For services that don't respect the `$PORT` environment variable (e.g., some legacy apps or hardcoded configs), we implemented a port discovery mechanism.

- **Mechanism**: We scan `/proc/net/tcp` (on Linux) to find ports opened by the service's process ID (PID) tree.
- **Integration**: If a service is configured with `port_discovery = "scan"`, `locald` will wait for the process to start and then scan for its listening port.

### 4. Advanced Health Checks

We moved beyond simple TCP probes and Docker health checks to support custom health checks defined in `locald.toml`.

- **Schema**: Added `health_check` field.
  - **HTTP Probe**: `health_check = { type = "http", path = "/health", port = 8080 }`
  - **Command Check**: `health_check = "curl -f http://localhost:3000/health"` or `health_check = { command = "..." }`
- **Implementation**: `locald-server` spawns a background task to poll the health check.
  - **HTTP**: Uses `reqwest` to poll the endpoint.
  - **Command**: Executes the shell command and checks for exit code 0.

### 5. Foreman Parity (.env & Signals)

We added support for features common in Foreman and Heroku workflows.

- **.env Support**: `locald` now automatically loads `.env` files from the project root using the `dotenvy` crate. These variables are injected into all services.
- **Custom Signals**: Services can now specify a `stop_signal` (e.g., `SIGINT`, `SIGQUIT`) in `locald.toml`. This is useful for graceful shutdown of workers that handle long-running jobs.

## Verification

We verified these features with a suite of example projects:

- `examples/procfile-test`: Verifies `Procfile` parsing.
- `examples/worker-test`: Verifies `type = "worker"`.
- `examples/port-discovery-test`: Verifies port scanning.
- `examples/health-check-test`: Verifies HTTP and Command health checks.
- `examples/env-test`: Verifies `.env` loading.
- `examples/signal-test`: Verifies custom `stop_signal` handling.

## Conclusion

With these changes, `locald` is now a capable replacement for Foreman and similar tools, offering a superset of features including dependency management, health checks, and integrated SSL/DNS.
