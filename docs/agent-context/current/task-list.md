# Task List - UX Polish: User-Visible Improvements

## 0. Phase State

- **Epoch:** E4 — The Build Era
- **Phase:** `phase-ux-polish`
- **Mode:** executing
- **Active remaining goal:** `ux.8` — Phase Finish Blockers: Resolve Postgres URL contract and current handoff drift

## 1. Completed Goals

- [x] **ux.1 — Service Status Contract**
  - `ServiceStatus` includes `service_type` and `connection_url`.
  - Server status/API surfaces populate these fields from service config/runtime data.

- [x] **ux.2 — Postgres DATABASE_URL Injection**
  - Services depending on typed Postgres receive a shared interpolation-based `DATABASE_URL`.
  - Added direct regression coverage for injection and env resolution behavior.

- [x] **ux.3 — Dashboard Accuracy**
  - Inspect/status surfaces expose `service_type` and nullable `connection_url`.
  - Dashboard components render browser URLs as links and data-service connection URLs as copyable non-navigation values.
  - Added `service-display` utility coverage.

- [x] **ux.4 — CLI Error Standardization**
  - Default-path CLI runtime errors render through `CliError`/miette.
  - Empty command coverage asserts normalized error output without crash-report footer noise.

- [x] **ux.5 — CLI Documentation**
  - Manual CLI page is a pointer-only stub.
  - Website CLI reference documents the current command surface and avoids removed/plumbing commands.

- [x] **ux.6 — Postgres Add Command Coverage**
  - `locald add postgres` and `locald service add postgres` dispatch to typed Postgres configuration generation.
  - Added direct parser/config regression coverage.

- [x] **ux.7 — Host Process UX Follow-up**
  - CLI `PATH` is inherited through Start and ProjectAttach IPC into daemon host-service startup.
  - Env precedence remains inherited CLI env < `.env` < explicit service env.
  - `locald up` no longer streams logs by default; `--verbose` preserves log streaming.

## 2. Active Goal: `ux.8`

- [ ] **ux.8::ux.8::postgres-url-contract — Resolve Postgres URL contract**
  - [x] Update shared Postgres URL helper to return `postgres://postgres@localhost:<port>/postgres`.
  - [x] Update server status/inspect tests to expect passwordless URL shape.
  - [x] Update dashboard display tests to expect passwordless Postgres connection URLs.
  - [x] Update managed services and service URL docs to stop inventing passwords.
  - [x] Update ignored Postgres integration example to use the same passwordless URL contract.
  - [ ] Finish focused validation and complete Exosuit task.

- [ ] **ux.8::ux.8::current-handoff-drift — Refresh current handoff artifacts**
  - [x] Replace stale implementation plan with UX polish plan.
  - [x] Replace stale task list with UX polish goal/task status.
  - [x] Replace stale walkthrough with UX polish walkthrough.
  - [x] Remove obsolete completion status from the current directory.
  - [ ] Confirm grep no longer finds previous-phase/current placeholder drift in `docs/agent-context/current`.
  - [ ] Complete Exosuit task.

## 3. Validation Checklist

- [ ] `cargo fmt --check`
- [ ] `cargo check -q -p locald-core -p locald-server -p locald-cli`
- [ ] `cargo test -p locald-cli handlers::ux_tests`
- [ ] `cargo test -p locald-server manager::tests::merge_service_start_env_preserves_precedence`
- [ ] `cargo test -p locald-server manager::tests::postgres_status_connection_url_matches_service_interpolation_url`
- [ ] `cargo test -p locald-server manager::tests::inspect_includes_connection_url_for_postgres_service`
- [ ] Dashboard focused service display test
- [ ] `pnpm -C locald-docs build`
- [ ] Grep source/current docs for stale Postgres URL and previous-phase handoff markers

## 4. Finish Criteria

- `ux.8` tasks are complete in Exosuit.
- `ux.8` goal outcome is reviewed and recorded.
- Phase finish only after current handoff artifacts and validation evidence are coherent.
