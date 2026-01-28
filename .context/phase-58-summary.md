# Phase 58: DDD Audit Summary

Date: 2026-01-28  
Status: Ready for Execution (Tasks 58.7-58.8)

## Decisions Made

| Question | Decision |
|----------|----------|
| `serve --bind` flag | **Implement** - trivial fix, currently confusing (default says `0.0.0.0`, binds to `127.0.0.1`) |
| Health `interval`/`timeout` | **Implement** - config values should be respected |
| Error handling scope | **Fix egregious cases** in Phase 58, plan rest for Phase 59 |
| P1/P2 scope | **In scope** for Phase 58, except secret redaction → Phase 59 |

## Audit Files Created

1. [phase-58-cli-audit.md](phase-58-cli-audit.md) - CLI Surface (Task 58.1)
2. [phase-58-config-audit.md](phase-58-config-audit.md) - Configuration Parsing (Task 58.2)
3. [phase-58-lifecycle-audit.md](phase-58-lifecycle-audit.md) - Service Lifecycle (Task 58.3)
4. [phase-58-health-urls-audit.md](phase-58-health-urls-audit.md) - Health Checks & URLs (Task 58.4)
5. [phase-58-privileged-audit.md](phase-58-privileged-audit.md) - Privileged Operations (Task 58.5)
6. [phase-58-doctor-audit.md](phase-58-doctor-audit.md) - Doctor Command (Task 58.6)

---

## Cross-Cutting Themes

### 1. Documentation Drift (HIGH PRIORITY)

Multiple areas show misalignment between docs and implementation:

- Doctor problem IDs don't match docs (`shim.missing` vs `SHIM_MISSING`)
- Health check `interval`/`timeout` documented but not implemented
- Config hierarchy description doesn't match merge order
- Shim discovery docs reference non-existent PATH lookup
- Security docs reference non-existent `locald-shim cap` command

### 2. Error Handling Inconsistency (HIGH PRIORITY)

- CLI uses mix of `miette::Result`, `process::exit()`, and printing errors
- "Unexpected response" branches exit successfully
- Exit codes only documented for `doctor`
- TOML parse errors lack line/column info

### 3. Machine Readability Gaps (MEDIUM PRIORITY)

- `--json` output limited to few commands (doctor, config, schema)
- Operational commands (ps, logs, restart, stop) not machine-readable
- No `--no-color` / `NO_COLOR` support
- Optional integrations (CNB, KVM) not in doctor JSON output

### 4. Security Hygiene (MEDIUM PRIORITY)

- Raw config logged (may leak secrets in `env`)
- Command history stored unredacted
- Crash reports include all `LOCALD_` env vars
- `privileged_ports` defaults to true (higher attack surface)
- Remote plugin URLs allow unchecked downloads

### 5. Testability Gaps (MEDIUM PRIORITY)

- Health probes use real network/shell with no mock layer
- Doctor checks depend on real host state
- Privileged ops require setuid shim in CI
- No `--config` override for deterministic testing

### 6. Lifecycle Model Clarity (MEDIUM PRIORITY)

- No explicit state machine diagram
- `prepare()`/`start()` semantics vary by controller
- No restart policy or crash recovery
- Health checks stop once healthy (no liveness)

---

## Prioritized Refactoring Candidates (Task 58.7)

### P0 (Must Fix)

1. **Fix `serve --bind` flag** - Currently ignored despite being documented
2. **Align doctor docs with reality** - Update problem IDs, remove fictional checks
3. **Standardize error handling** - Return `miette::Result` everywhere, no ad-hoc `process::exit()`
4. **Implement health check `interval`/`timeout`** - Or remove from schema/docs

### P1 (Should Fix)

5. **Add `--json` to operational commands** - `ps`, `logs`, `restart`, `stop`
6. **Add `NO_COLOR` support** - Detect non-TTY, respect env var
7. **Redact secrets in logs** - Config logging, crash reports, command history
8. **Fix config hierarchy docs** - Or adjust merge order to match docs

### P2 (Nice to Have)

9. **Add `--config` override** - For testing and automation
10. **Document lifecycle state machine** - In manual concepts
11. **Centralize privilege policy matrix** - What requires shim, what degrades
12. **Add lifecycle events** - Started/Stopped/Failed over IPC stream

---

## Rustdoc/Doctest Candidates (Task 58.8)

### High-Value Targets

1. `crates/locald-core/src/config/` - Schema types need examples
2. `crates/locald-core/src/state.rs` - State enum needs docstrings
3. `crates/locald-core/src/doctor/` - Check functions need docs
4. `crates/locald-server/src/controller/mod.rs` - Lifecycle contract needs docs
5. `crates/locald-utils/src/privileged.rs` - Privilege model needs docs

### Doctest Opportunities

1. Config parsing examples with `serde`
2. State transition examples
3. URL/domain generation examples
4. Health check configuration examples

---

## Next Steps

1. ~~**Review this summary** with maintainer~~ ✅ Done
2. ~~**Prioritize P0 items** for immediate refactoring (Task 58.7)~~ ✅ Done
3. **Execute Task 58.7** - Refactoring (see execution plan below)
4. **Execute Task 58.8** - Rustdoc improvements
5. **Create Phase 59** - For deferred work (secret redaction, remaining error handling)

---

## Execution Plan (Task 58.7 - Refactoring)

### 58.7.1: Fix `serve --bind` flag
**Files:**
- `crates/locald-server/src/static_server.rs` - Add `bind` parameter to `run_static_server()`
- `crates/locald-cli/src/handlers.rs` - Pass `bind` arg instead of ignoring it

**Acceptance:** `locald serve --bind 127.0.0.1` binds to localhost only; `--bind 0.0.0.0` binds to all interfaces

### 58.7.2: Align doctor problem IDs with docs
**Files:**
- `docs/manual/features/doctor.md` - Update problem IDs to match code
- `crates/locald-core/src/doctor/` - Verify current IDs

**Acceptance:** All problem IDs in docs exactly match code (`shim.missing`, `cgroup.v2_unavailable`, etc.)

### 58.7.3: Implement health check `interval`/`timeout`
**Files:**
- `crates/locald-server/src/health.rs` - Use config values instead of hard-coded 250ms
- `crates/locald-core/src/config/health.rs` - Verify schema has proper defaults

**Acceptance:** Custom `interval` and `timeout` in `health_check` config are respected

### 58.7.4: Fix egregious error handling (exit 0 on error)
**Files:**
- `crates/locald-cli/src/handlers.rs` - Find "Unexpected response" branches that exit 0
- Return `Err()` instead of printing and continuing

**Acceptance:** Commands that fail return non-zero exit codes

### 58.7.5: Add `--json` to operational commands
**Files:**
- `crates/locald-cli/src/cli.rs` - Add `--json` flag to `ps`, `restart`, `stop`
- `crates/locald-cli/src/handlers.rs` - Implement JSON output

**Acceptance:** `locald ps --json` outputs parseable JSON

### 58.7.6: Add `NO_COLOR` support
**Files:**
- `crates/locald-cli/src/main.rs` - Check `NO_COLOR` env and TTY detection
- Disable miette/crossterm colors when appropriate

**Acceptance:** `NO_COLOR=1 locald doctor` produces no ANSI escapes

---

## Execution Plan (Task 58.8 - Rustdoc)

### 58.8.1: Config module docs
**Files:** `crates/locald-core/src/config/*.rs`
**Work:** Module-level docs, `///` for public types, parsing doctests

### 58.8.2: State enum docs
**Files:** `crates/locald-core/src/state.rs`
**Work:** Document `ServiceState`/`HealthStatus` with transition semantics

### 58.8.3: Doctor module docs
**Files:** `crates/locald-core/src/doctor/`
**Work:** Document problem ID inventory, check functions

### 58.8.4: Controller lifecycle docs
**Files:** `crates/locald-server/src/controller/mod.rs`
**Work:** Document `prepare()`/`start()`/`stop()` contract

---

## Deferred to Phase 59

1. **Secret redaction** - Config logging, crash reports, command history
2. **Remaining error handling** - All 18 `process::exit()` occurrences
3. **Config hierarchy docs** - Align docs with merge order or vice versa
