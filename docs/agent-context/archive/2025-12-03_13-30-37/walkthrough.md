# Walkthrough - Phase 20: Builtin Services

**Goal**: Provide "Heroku-style" managed data services (Postgres) that work out of the box without Docker, `mise`, or manual binary management.

## Changes

### 1. Service Type Abstraction

- Refactored `ServiceConfig` to use a tagged enum `TypedServiceConfig`.
- Supported types: `exec` (default) and `postgres`.
- This allows strict validation of service configuration (e.g., `postgres` services don't have a `command`).

### 2. Postgres Integration

- Added `postgresql_embedded` dependency to `locald-server`.
- Implemented `PostgresRunner` which:
  - Downloads the Postgres binary on demand.
  - Initializes a data directory in `$XDG_DATA_HOME/locald/services/<project>/<service>`.
  - Starts the Postgres server on a dynamic port.
  - Creates a default database and user.

### 3. Dependency Injection

- Implemented environment variable substitution in `locald.toml`.
- Services can reference other services' properties using `${services.<name>.<property>}` syntax.
- Example: `DATABASE_URL = "${services.db.url}"`.
- `PostgresRunner` exposes a `url` property with the full connection string.

### 4. Testing

- Added integration tests using `assert_cmd` and `tokio`.
- `dependency_test.rs`: Verifies that environment variables are correctly substituted.
- `postgres_test.rs`: Verifies that Postgres starts and accepts connections (currently ignored in CI due to GitHub API rate limits).

### 5. CLI Updates

- Introduced `locald service add` subcommand group.
- `locald service add exec <command>`: Adds a standard shell command service.
- `locald service add postgres <name>`: Adds a managed Postgres service.
- Updated `locald add` to be a shortcut for `locald service add exec`.
- Added `locald service reset <name>`: Stops a service, wipes its data directory (for stateful services like Postgres), and restarts it.

### 6. UX Improvements

- Automated `.gitignore` updates: `locald init` and `locald service add` now ensure `.locald/` is added to `.gitignore` to prevent checking in local state.
