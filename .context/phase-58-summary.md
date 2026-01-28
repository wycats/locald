# Phase 58: DDD Audit Summary

Date: 2026-01-28  
Status: Audit Complete (Tasks 58.1-58.6)

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

1. **Review this summary** with maintainer
2. **Prioritize P0 items** for immediate refactoring (Task 58.7)
3. **Create issues** for deferred work (P1/P2)
4. **Add Rustdoc** to high-value targets (Task 58.8)
5. **Update `implementation-plan.toml`** with completed audit tasks
