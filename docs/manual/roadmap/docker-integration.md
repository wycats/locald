# Design: Docker Integration

> **Status**: Withdrawn. Superseded by the OCI/libcontainer execution layer.
>
> **Core focus**: The default experience is host execution; container integration is future work.

This proposal is retained for historical context. The DockerRuntime has been removed, and the `image` and `container_port` fields are no longer part of the service schema.

## Context

Modern web development often requires a "Hybrid" environment:

- **Application Code**: Runs locally (on the host) for fast feedback loops (HMR, incremental compilation).
- **Infrastructure Dependencies**: Run in containers (Docker) because installing databases/queues locally is cumbersome and version management is hard.

Currently, `locald` only supports running local commands. Users have to manually run `docker-compose up` alongside `locald`, which fragments the process management (logs are split, lifecycle is split).

## Goals

1.  **Unified Lifecycle**: `locald up` should start both local apps and Docker containers.
2.  **Unified Logs**: `locald logs` should show logs from containers too.
3.  **Simple Config**: Avoid the need for a separate `docker-compose.yml` for simple dependencies.

## Historical Approaches (Withdrawn)

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

## Current Direction

Container execution is handled via `locald`'s OCI/libcontainer stack. Docker-specific schema and runtime integration are no longer planned in this form.
