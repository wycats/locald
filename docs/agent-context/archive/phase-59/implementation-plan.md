# Implementation Plan - UX Polish: User-Visible Improvements

**Goal**: Make the user-visible surfaces of locald coherent and trustworthy before finishing the UX polish phase: service status accuracy, dashboard connection display, CLI errors/docs, host-process startup behavior, and current handoff artifacts.

**Primary RFC**: RFC 0146

**Supporting audit decisions**:

- RFC 0112 / CL-041 / C-012: managed Postgres persistence is XDG-first, with a local fallback only when the platform data directory is unavailable.
- RFC 0112 / C-013: `${services.<name>.url}` is the canonical Postgres connection string surface, and the stable default contract is passwordless.

## 0. Scope

This phase is about **user-visible correctness and polish**, not container runtime plumbing or VMM work.

In scope:

- Expose service type and connection URL consistently through IPC, status, inspect, and dashboard surfaces.
- Make managed Postgres interpolation, metadata, docs, and dashboard examples agree on the passwordless URL contract.
- Normalize default-path CLI error rendering.
- Make the website CLI reference the canonical command-surface source of truth.
- Add typed Postgres command coverage.
- Preserve host-service access to CLI PATH values such as `pnpm` when services are launched by the daemon.
- Keep `locald up` concise by default while preserving verbose log streaming behind `--verbose`.
- Refresh current agent-context handoff artifacts so they describe this phase.

Out of scope:

- VMM execution work.
- Full Mark-Sweep GC implementation for managed service state.
- New service types beyond typed Postgres.
- Large command vocabulary changes outside the current CLI documentation slice.

## 1. Target Architecture

### Status and inspect contract

- `ServiceStatus` carries enough typed data for UIs and tools to avoid guessing from URL presence.
- Browser URLs and data-service connection URLs are separate concepts.
- Postgres status/inspect connection data uses the same helper as interpolation and controller metadata.

### Postgres connection contract

- `${services.db.url}` is the stable user-facing connection string surface.
- The default managed Postgres URL shape is:
  - `postgres://postgres@localhost:<port>/postgres`
- Docs must not invent placeholder passwords or teach hardcoded ports.

### Host process environment contract

- The CLI forwards a curated inherited environment to the daemon for service startup.
- Today, the inherited set is intentionally small: `PATH` only.
- Precedence is: inherited CLI env < `.env` < explicit service env.

### `locald up` output contract

- Default `locald up` reports concise startup progress and a follow-up hint for logs.
- `locald up --verbose` keeps the log-streaming behavior for users who asked for it.

## 2. Work Breakdown

### 2.1 Service status and dashboard accuracy

- Add `service_type` and `connection_url` to shared IPC status types.
- Populate server status and inspect data from service config/runtime metadata.
- Update dashboard components to render public browser URLs separately from copyable connection URLs.
- Add dashboard display tests for Postgres connection URL handling.

### 2.2 CLI error and documentation polish

- Convert default-path CLI helpers from ad-hoc `anyhow` errors to `CliError`/miette rendering.
- Keep crash-report handling for actual unexpected failures rather than normal command errors.
- Reduce the manual CLI page to a source-of-truth pointer.
- Synchronize the Starlight CLI reference to current commands.
- Add direct coverage for typed Postgres add command generation.

### 2.3 Host process startup UX

- Extend Start and ProjectAttach IPC with curated inherited env.
- Capture CLI `PATH` and merge it into daemon host-service startup env.
- Preserve `.env` and explicit service env precedence.
- Gate post-`up` log streaming behind `--verbose`.

### 2.4 Postgres contract reconciliation

- Make `postgres_connection_url(port)` the shared source of truth.
- Use the passwordless URL for interpolation, service status, inspect output, metadata, dashboard tests, docs, and the ignored Postgres integration example.
- Document XDG-first data location with `.locald/postgres/<name>` only as fallback.

### 2.5 Current handoff coherence

- Replace stale previous-phase current artifacts with UX polish plan/task/walkthrough artifacts.
- Remove obsolete previous-phase completion status from the current directory because that work already has archived context.
- Leave the claim-ledger triage plan as a durable current planning reference, but do not treat it as the active phase walkthrough.

## 3. Verification Plan

- `cargo fmt --check`
- `cargo check -q -p locald-core -p locald-server -p locald-cli`
- `cargo test -p locald-cli handlers::ux_tests`
- `cargo test -p locald-server manager::tests::merge_service_start_env_preserves_precedence`
- `cargo test -p locald-server manager::tests::postgres_status_connection_url_matches_service_interpolation_url`
- `cargo test -p locald-server manager::tests::inspect_includes_connection_url_for_postgres_service`
- Dashboard focused display tests.
- `pnpm -C locald-docs build`
- Grep current handoff files for stale previous-phase markers.

## 4. Acceptance Criteria

- Postgres URL surfaces agree on the passwordless `${services.<name>.url}` contract.
- Managed services docs no longer teach passworded Postgres URLs or the legacy workspace-local data-path claim as the normal state location.
- Host services launched through the daemon can find commands present on the CLI `PATH`, such as `pnpm`.
- `locald up` is quiet by default and streams logs only with `--verbose`.
- Current agent-context artifacts describe `phase-ux-polish`, not a previous phase.
- Exosuit task and goal state can be completed with evidence from code, docs, and validation.
