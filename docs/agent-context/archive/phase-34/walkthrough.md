# Walkthrough - UX Polish: User-Visible Improvements

## Step 0: Reconcile the UX polish phase

1. **Objective**: Convert the user-programming-model audit into a concrete UX polish execution slice.
2. **Output**:
   - Active Exosuit phase: `phase-ux-polish` in E4 — The Build Era.
   - Goals `ux.1` through `ux.7` completed and reviewed.
   - Finish blocker goal `ux.8` added for Postgres URL contract and current handoff drift.

## Step 1: Service status and connection surfaces

1. **Objective**: Make service status accurate enough for users, dashboard, and tools.
2. **Output**:
   - Shared IPC `ServiceStatus` includes `service_type` and `connection_url`.
   - Server status and inspect paths populate typed service details.
   - Postgres exposes a data-service connection URL instead of a browser URL.

## Step 2: Dashboard display alignment

1. **Objective**: Stop treating every endpoint as a browser link.
2. **Output**:
   - `ProjectView`, `Rack`, `ServiceCard`, and `InspectorDrawer` distinguish public URLs from connection URLs.
   - Shared dashboard display helper prefers browser URLs for web services and connection URLs for data services.
   - Postgres connection URL display has focused Vitest coverage.

## Step 3: CLI errors and documentation source of truth

1. **Objective**: Make CLI failures and documentation more predictable.
2. **Output**:
   - Default-path runtime errors render through `CliError`/miette.
   - `locald try` empty command regression renders a normalized CLI error rather than crash-report boilerplate.
   - Manual CLI page points to the website reference as source of truth.
   - Website CLI reference reflects the current command surface and typed Postgres add workflow.

## Step 4: Host process startup UX

1. **Objective**: Fix daemon-launched host services losing CLI PATH, while reducing noisy `locald up` output.
2. **Output**:
   - `IpcRequest::Start` and `IpcRequest::ProjectAttach` carry curated inherited env.
   - CLI forwards `PATH`; daemon merges it into host-service startup env.
   - Env precedence remains inherited CLI env < `.env` < explicit service env.
   - `locald up` streams logs only with `--verbose` and otherwise points users at `locald logs --follow`.

## Step 5: Postgres contract reconciliation

1. **Objective**: Resolve the phase-finish blocker from RFC 0112 C-013.
2. **Output**:
   - Canonical Postgres URL helper returns `postgres://postgres@localhost:<port>/postgres`.
   - `${services.<name>.url}`, status/inspect `connection_url`, controller metadata, dashboard tests, docs, and integration example all use the passwordless contract.
   - Managed services docs teach XDG-first data location under `postgres/<name>` with `.locald/postgres/<name>` only as fallback.

## Step 6: Current handoff coherence

1. **Objective**: Ensure the `docs/agent-context/current` directory describes the active UX polish phase, not an old container runtime phase.
2. **Output**:
   - Current implementation plan, task list, and walkthrough describe `phase-ux-polish`.
   - Obsolete previous-phase completion status is removed from current context because that work is already archived.
   - Remaining claim-ledger triage plan is retained as planning context for the UX polish lineage.

## Step 7: Final verification and phase finish

1. **Objective**: Prove the code/docs/handoff state is coherent enough to complete `ux.8` and then finish the phase.
2. **Output**:
   - Rust formatting/checks and targeted CLI/server tests pass.
   - Dashboard/docs focused validation passes or any residual tool/environment limitation is recorded.
   - Grep confirms no stale previous-phase markers remain in current handoff files.
   - Exosuit records completion evidence for `ux.8` tasks and goal.
