# Design: Docker Integration

## Context

Modern web development often requires a "Hybrid" environment:

- **Application Code**: Runs locally (on the host) for fast feedback loops (HMR, incremental compilation).
- **Infrastructure Dependencies**: Run in containers (Docker) because installing databases/queues locally is cumbersome and version management is hard.

Currently, `locald` only supports running local commands. Users have to manually run `docker-compose up` alongside `locald`, which fragments the process management (logs are split, lifecycle is split).

## Goals

1.  **Unified Lifecycle**: `locald start` should start both local apps and Docker containers.
2.  **Unified Logs**: `locald logs` should show logs from containers too.
3.  **Simple Config**: Avoid the need for a separate `docker-compose.yml` for simple dependencies.

## Proposed Approaches

### Approach 1: Native Docker Support (The "Locald Way")

Add support for defining container-based services directly in `locald.toml`.

```toml
[services.db]
image = "postgres:15"
port = 5432
env = { POSTGRES_PASSWORD = "secret" }
# Optional: Persistence
volumes = ["./data:/var/lib/postgresql/data"]
```

**Mechanism**:

- `locald` constructs a `docker run` command.
- `docker run --rm --name project-db -p 5432:5432 -e ... postgres:15`
- `locald` manages the `docker` CLI process.

**Pros**:

- Single config file (`locald.toml`).
- Tighter integration (we know the port, we can inject it into other services).
- No `docker-compose` dependency (just `docker`).

**Cons**:

- Re-inventing parts of Docker Compose.

### Approach 2: Docker Compose Delegation

Allow a service to reference an entry in `docker-compose.yml`.

```toml
[services.db]
compose_service = "db"
```

**Mechanism**:

- `locald` runs `docker-compose up db`.

**Pros**:

- Reuses existing configuration.
- Supports complex container networking/volumes defined in Compose.

**Cons**:

- Split configuration.
- Harder to dynamically assign ports (Compose usually has fixed ports).

## Recommendation: Approach 1 (Native)

For the "App Builder" persona, defining a database in `locald.toml` is a magical experience. It removes the friction of learning Docker Compose syntax for simple needs.

**Decision**: We will implement Approach 1, but with a **strict and minimal schema**. We will not attempt to support every Docker feature. If users need complex networking or capabilities, they should use `command = "docker-compose up ..."` instead.

## Implementation Plan

### 1. Schema Update

We will add the following optional fields to `ServiceConfig`:

- `image: Option<String>`: The Docker image to run (e.g., `postgres:15`). If present, `command` is ignored (or used as args?). _Decision: If `image` is present, `command` is optional and treated as the container command/args._
- `container_port: Option<u16>`: The port _inside_ the container to expose. Required if we want to map a port.
- `volumes: Option<Vec<String>>`: List of volume mounts (passed to `-v`).

**Example**:

```toml
[services.db]
image = "postgres:15"
container_port = 5432
env = { POSTGRES_PASSWORD = "secret" }
volumes = ["./data:/var/lib/postgresql/data"]
```

### 2. Process Manager & Lifecycle

- **Command Construction**: `docker run --rm --name locald-<project>-<service> -p <host_port>:<container_port> ... <image>`
- **Deterministic Naming**: Containers must be named `locald-<project>-<service>` to allow for cleanup.
- **Robust Cleanup**:
  - On `start()`, before spawning, run `docker rm -f locald-<project>-<service>` to remove any zombie containers from previous runs.
  - On `stop()`, the `docker` CLI process is killed. Since we use `--rm`, the container _should_ die, but we should also explicitly `docker stop` it if possible, or rely on the startup cleanup of the next run.

### 3. Networking

- **Host Access**: Containers often need to talk to the host (e.g., to reach another service running locally).
  - We should investigate injecting `host.docker.internal` mapping if possible, or document how to reach the host.
  - _Note_: On Linux, `host.docker.internal` is not always available by default. We might need `--add-host host.docker.internal:host-gateway`.

### 4. Port Management

- If `container_port` is specified, `locald` binds a random ephemeral port on the host (just like it does for local processes) and maps it: `-p <random_host_port>:<container_port>`.
- The `<random_host_port>` is injected into the environment of _other_ services as `PORT_<SERVICE_NAME>`.
