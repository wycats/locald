# Design: Builtin Services

**Goal**: Provide "Heroku-style" managed data services (Postgres, Redis) that work out of the box without Docker, `mise`, or manual binary management.

## The Concept

`locald` should feel like it has "batteries included". Just as `locald` serves its own documentation and dashboard (`locald.localhost`), it should be able to serve common backing services.

We are **not** building a generic package manager (like `mise` or `asdf`). We are building a curated set of "Builtin Services" that are critical for web development.

## Supported Services

1.  **Postgres**:

    - **Implementation**: Use `postgresql_embedded` (Rust crate).
    - **Behavior**: Downloads a portable Postgres binary for the current OS/Arch, initializes the data directory, and manages the process.
    - **Why**: It's the standard relational DB. `postgresql_embedded` abstracts the cross-platform binary fetching perfectly.

2.  **Redis**:

    - **Implementation**: Download static binaries from a trusted source (or build a small wrapper crate similar to `postgresql_embedded`).
    - **Why**: The standard K/V store.

3.  **Future**:
    - **Wasm**: If a reliable Wasm registry emerges for server-side components (e.g., a WASI-compliant Redis or Postgres), we can switch to running these in a Wasm runtime (like `wasmtime`) embedded in `locald`. For now, native binaries are the pragmatic choice.

## Configuration & Dependency Injection

Services are defined in `locald.toml`. Dependent services automatically get connection strings injected.

```toml
# Define a builtin Postgres service
[services.db]
type = "postgres"
version = "15" # Optional, defaults to stable
# port is auto-assigned
# credentials are auto-generated

# Define a web service that needs the DB
[services.web]
command = "npm start"
depends_on = ["db"]

# Configuration for Env Injection
[services.web.env]
# By default, locald injects "DATABASE_URL" for postgres services.
# You can remap it if needed:
DB_CONNECTION_STRING = "${services.db.url}"
```

## "Magic" Features

1.  **Zero-Config Defaults**:
    - `locald add postgres` creates the entry.
    - No port conflicts (ephemeral ports by default).
    - No password management (auto-generated, injected via env).
2.  **Lifecycle Management**:
    - `locald` manages the `initdb` step automatically.
    - Data is persisted in `$XDG_DATA_HOME/locald/services/<name>/`.
    - Logs are captured alongside app logs.
3.  **Wait-for-Ready**:
    - The `depends_on` directive knows how to wait for Postgres to actually accept connections (pg_isready) before starting the web app.

## Comparison to Docker

| Feature           | Builtin (Native)                 | Docker Provider                   |
| :---------------- | :------------------------------- | :-------------------------------- |
| **Prerequisites** | None (locald downloads binaries) | Docker Desktop / Podman           |
| **Performance**   | Native Process                   | Container / VM overhead           |
| **Isolation**     | Process-level (shared kernel)    | Container-level                   |
| **Versions**      | Limited to available binaries    | Any tag on Docker Hub             |
| **Use Case**      | "I just need a DB"               | "I need a specific complex stack" |

## Implementation Plan

1.  **Phase 21a**: Integrate `postgresql_embedded`.
2.  **Phase 21b**: Implement the "Service Type" abstraction (distinguishing `exec` services from `builtin` services).
3.  **Phase 21c**: Implement Dependency Injection (Env var templating).
