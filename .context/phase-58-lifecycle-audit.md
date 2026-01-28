# Phase 58.3 DDD Audit — Service Lifecycle

Date: 2026-01-28  
Scope: Service lifecycle management (`crates/locald-server/`)

## 1. Lifecycle Model (States & Transitions)

### State Vocabulary

- **ServiceState**: `Stopped`, `Starting`, `Running`, `Building` (`crates/locald-core/src/state.rs`)
- **HealthStatus**: `Unknown`, `Healthy`, `Unhealthy`, `Starting` (`crates/locald-core/src/health.rs`)

### Controller Contract

- Lifecycle hooks are `prepare()` → `start()` → `stop()` with `status()` for status polling (`crates/locald-server/src/controller/mod.rs`).
- Only `SiteService` explicitly sets `ServiceState::Starting` + `HealthStatus::Starting` during `start()` (`crates/locald-server/src/controller/site.rs`).

### Transitions & Triggers

- **`locald up`** → IPC `RegisterProject` → `ProjectManager::register()` → config load → per-service `prepare()` then `start()` (`crates/locald-server/src/manager.rs`, `crates/locald-server/src/ipc.rs`).
- **Health gate**: after each service start, `HealthMonitor::wait_healthy()` blocks next service; timeout fails startup (30s) (`crates/locald-server/src/health.rs`).
- **`locald stop`** → IPC `StopService` → `ProjectManager::stop_service()` → controller `stop()` → persist & broadcast (`crates/locald-server/src/manager.rs`, `crates/locald-server/src/ipc.rs`).
- **`locald restart`** → stop then start via project path (`crates/locald-server/src/manager.rs`, `crates/locald-cli/src/commands/restart.rs`).
- **Shutdown (Ctrl+C)** → `Daemon::shutdown()` stops all controllers (`crates/locald-server/src/daemon.rs`, `crates/locald-server/src/controller/exec.rs`).

### Notes on "Building"

- `ServiceState::Building` exists in the enum but only `CnbController` uses it. Other controllers don't set it, so build/prepare is often invisible in state (`crates/locald-server/src/controller/cnb.rs`).

## 2. Friction Points by Persona

### New Hire

- No explicit lifecycle diagram or state machine; only implicit flow through `prepare()`/`start()`/`stop()` and enums (`crates/locald-core/src/state.rs`, `crates/locald-server/src/controller/mod.rs`).
- `prepare()`/`start()` are inconsistently used across controllers, making the lifecycle model feel ad‑hoc (`crates/locald-server/src/controller/*.rs`).

### Security Auditor

- Host processes run as the current user; privileged actions are delegated to `locald-shim` without a centralized policy document in the lifecycle code path (`crates/locald-server/src/controller/exec.rs`).
- IPC is a Unix socket; access control is based on filesystem permissions and errors are surfaced but not audited (`crates/locald-server/src/ipc.rs`, `crates/locald-utils/src/ipc.rs`).

### Test Engineer

- Dependency ordering is unit‑tested, but lifecycle transitions (prepare/start/stop, health timeouts, restart) lack visible tests in server modules; the flow is largely implicit in `ProjectManager::register()` (`crates/locald-server/src/manager.rs`, `crates/locald-server/tests/`).

### SRE

- No restart policy or crash recovery; `controller.reset()` is a no‑op and no watchdog restarts failed services (`crates/locald-server/src/controller/mod.rs`).
- Observability exists (log and metrics events) but no explicit lifecycle/audit events for state transitions (`crates/locald-server/src/events.rs`, `crates/locald-server/src/manager.rs`).

## 3. Recommendations (Prioritized)

### P0

1. **Document the lifecycle state machine** (states, transitions, triggers, error paths) in a single place; align `prepare()` and `start()` semantics across controllers (`docs/manual/concepts/`, `crates/locald-server/src/controller/mod.rs`).
2. **Standardize state transitions in controllers**: ensure `start()` sets `Starting`/`Running` and `stop()` sets `Stopped` consistently (currently only `SiteService` does) (`crates/locald-server/src/controller/site.rs`).

### P1

3. **Add crash detection + restart policy hooks** (even if default is "never"); wire into a minimal supervisor loop or periodic health check reconciliation (`crates/locald-server/src/health.rs`).
4. **Emit explicit lifecycle events** (Started/Stopped/Restarted/Failed) over the IPC event stream for better observability and testing (`crates/locald-server/src/events.rs`).

### P2

5. **Strengthen dependency failure behavior**: include which dependency failed and whether partial startup should be rolled back, and document the rule (`crates/locald-server/src/manager.rs`).
6. **Add tests for lifecycle flows** (start→prepare→health gate; stop; restart; dependency failure) around `ProjectManager` and controllers (`crates/locald-server/tests/`).

## 4. Code References

- State enums: `crates/locald-core/src/state.rs`
- Controller lifecycle contract: `crates/locald-server/src/controller/mod.rs`
- Start/apply_config/health gate: `crates/locald-server/src/manager.rs`, `crates/locald-server/src/health.rs`
- Stop/restart/reset: `crates/locald-server/src/manager.rs`
- Dependencies (topological sort + cycle detection): `crates/locald-core/src/config/deps.rs`
- Exec stop signal + cgroup cleanup: `crates/locald-server/src/controller/exec.rs`
- Graceful termination + SIGKILL fallback: `crates/locald-server/src/controller/exec.rs`
- SiteService `Starting` usage: `crates/locald-server/src/controller/site.rs`
- IPC server (Unix socket + JSON protocol): `crates/locald-server/src/ipc.rs`
- CLI IPC client: `crates/locald-utils/src/ipc.rs`
- Shutdown handling: `crates/locald-server/src/daemon.rs`
- Event stream definitions: `crates/locald-server/src/events.rs`
- Health monitor hooks: `crates/locald-server/src/health.rs`
