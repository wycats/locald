# Implementation Plan - Phase 20: Builtin Services

**Goal**: Provide "Heroku-style" managed data services (Postgres) that work out of the box without Docker, `mise`, or manual binary management.

## User Verification

- [ ] **Add Postgres**: Run `locald add postgres db` and verify it adds to `locald.toml`.
- [ ] **Start**: Run `locald up` and verify Postgres downloads, initializes, and starts.
- [ ] **Connect**: Verify you can connect to the database using `psql` or a GUI tool (using the credentials from `locald status` or logs).
- [ ] **Dependency**: Create a dummy app that depends on `db` and prints `$DATABASE_URL`. Verify it gets the correct connection string.
- [ ] **Persistence**: Restart `locald` and verify data persists.

## Proposed Changes

### 1. Service Type Abstraction

- **Refactor `ServiceConfig`**:
  - Use a Serde tagged enum (`#[serde(tag = "type", rename_all = "lowercase")]`) to strictly define service types.
  - **`Exec`** (default): Requires `command`.
  - **`Postgres`**: Does **not** allow `command`. Supports `version` (optional).
  - This ensures invalid configs (like `type = "postgres"` with a `command`) are rejected at deserialization time.

### 2. Postgres Integration (`postgresql_embedded`)

- **Dependency**: Add `postgresql_embedded` to `locald-server`.
- **Runner Implementation**:
  - Implement `PostgresRunner` that uses the crate to:
    - Download the binary (if missing).
    - Initialize the data directory in `$XDG_DATA_HOME/locald/services/<project>/<service_name>`.
    - Start the server on a dynamic port.
    - Create a default database and user.
- **UX & On-Demand Loading**:
  - The binary download should happen on-demand during `locald up` or service start.
  - **Crucial**: Provide clear feedback (progress bar/spinner) to the user during the download phase, as it may take time.
  - Service status should reflect `Downloading` or `Installing` during this process.

### 3. Dependency Injection

- **Env Var Templating**:
  - Implement variable substitution for environment variables in `locald.toml`.
  - Support referencing service properties, e.g., `${services.db.url}`.
  - This allows users to map the connection string to whatever env var their app expects (e.g., `DB_CONNECTION_STRING`, `POSTGRES_URI`).
  - The `PostgresRunner` will expose a `url` property containing: `postgres://<user>:<password>@127.0.0.1:<port>/<dbname>`.

### 4. CLI Updates

- **`locald add`**:
  - Enhance `locald add` (or `init`) to support adding builtin services.
  - Example: `locald add postgres my-db`.

### 5. Automated Testing

- **Goal**: Convert manual verification steps into runnable tests.
- **Tools**: Introduce `assert_cmd` and `predicates` for integration testing.
- **Test Case**: Create a test that:
  1. Initializes a temporary project.
  2. Adds a postgres service.
  3. Starts `locald`.
  4. Verifies the service becomes healthy.
  5. Connects to the database (using `sqlx` or `postgres` crate in the test).
  6. Shuts down.

## Tasks

- [ ] Add `postgresql_embedded` dependency.
- [ ] Refactor `ServiceConfig` to support `type` field (defaulting to "exec").
- [ ] Create `ServiceRunner` trait/enum in `locald-server`.
- [ ] Implement `PostgresRunner` using `postgresql_embedded`.
- [ ] Implement data directory management for builtin services.
- [ ] Implement `DATABASE_URL` injection for dependent services.
- [ ] Update `locald add` to support `postgres` type.
- [ ] **Testing**: Add `assert_cmd`, `predicates`, `tokio`, `sqlx` (dev) dependencies.
- [ ] **Testing**: Implement integration test for Postgres service.
- [ ] Verify with a test project.
