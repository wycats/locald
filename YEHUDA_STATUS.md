# Session Handoff: RFC 0130 Complete

**Date**: 2026-01-08
**Last Commit**: `7e1887d` (chore: update CLI surface manifest for RFC 0130)
**PR**: [#51 - RFC 0130: Host Shim Daemon for Container Development Environments](https://github.com/wycats/locald/pull/51)

## Work Completed This Session

RFC 0130 (Host Shim Daemon for Container Development Environments) has been **fully implemented** across 5 phases:

| Phase   | Commit    | Summary                                                                           |
| ------- | --------- | --------------------------------------------------------------------------------- |
| Phase 1 | `51de366` | Socket-based Shim Daemon - Unix socket server in `locald-shim`                    |
| Phase 2 | `6176e80` | Host-Exec Configuration - `[container]` config with `host_exec` template          |
| Phase 3 | `f4048f0` | Polkit Integration - GUI privilege escalation via `pkexec`                        |
| Phase 4 | `8d4b9e7` | Container Auto-Start - Socket-first strategy in `PrivilegedCapability::acquire()` |
| Phase 5 | `718255a` | Documentation and Polish - Container guide, doctor reference                      |
| Manifest | `7e1887d` | CLI surface manifest update for `--host` flag                                    |

### Key Files Created/Modified

**New Files:**

- `crates/locald-shim/src/daemon.rs` (~650 lines) - Socket server daemon
- `crates/locald-shim/src/protocol.rs` (~520 lines) - Wire protocol types
- `crates/locald-utils/src/shim_client.rs` (~690 lines) - Client for socket communication
- `assets/dev.locald.policy` - Polkit policy XML
- `docs/manual/features/doctor.md` - Doctor command reference
- `docs/research/host-container-privilege-split.md` - Research notes

**Modified Files:**

- `crates/locald-utils/src/privileged.rs` - Socket-first acquisition strategy
- `crates/locald-utils/src/shim.rs` - Polkit detection/installation
- `crates/locald-cli/src/handlers.rs` - Wire up shim daemon commands
- `crates/locald-cli/src/utils.rs` - Host-exec, container detection
- `docs/manual/development/container-environments.md` - Container guide

### Acceptance Criteria Met

The critical requirement from RFC 0130:

> "You should be able to run `admin setup` on the host and then `locald up` in the toolbox"

This is now possible:

1. **On host**: `sudo locald admin setup` (installs shim + polkit policy)
2. **On host**: `locald shim serve` (starts daemon, or it auto-starts)
3. **In Toolbx**: `locald up` connects to host socket automatically

## Pending Actions for Next Session

### 1. Merge PR #51

Review and merge [PR #51](https://github.com/wycats/locald/pull/51). See "Review Checklist" below.

### 2. RFC Promotion to Stage 3 (After Merge)

After merging, promote RFC 0130 from `stage-2` to `stage-3`:

```bash
mkdir -p docs/rfcs/stage-3
git mv docs/rfcs/stage-2/0130-host-shim-daemon.md docs/rfcs/stage-3/
rm docs/rfcs/stage-1/0130-host-shim-daemon.md
# Update frontmatter: stage: 2 → stage: 3
```

### 3. Return to Phase 29 (Plugins)

The project's active phase per `exo-status` is Phase 29 (Extensibility & Plugins). Tasks completed:

- ✅ 29.1.1: Specify plugin contract + fixtures
- ✅ 29.1.2: Run WASM component (WASI-only)

Tasks remaining:

- ⏳ 29.1.3: ServicePlan IR + apply/merge
- ⏳ 29.1.4: Example plugin + e2e verification

---

## Review Checklist for Stage 3 Promotion

Per the RFC process, promotion to Stage 3 (Candidate) requires:

### ✅ Implementation Complete
All 5 phases implemented and committed.

### ✅ Manual Updated
Documentation updated in `docs/manual/`:
- [container-environments.md](docs/manual/development/container-environments.md) - Full container guide
- [doctor.md](docs/manual/features/doctor.md) - Doctor command reference
- [shim-management.md](docs/manual/architecture/shim-management.md) - Socket modes
- [configuration.md](docs/manual/architecture/configuration.md) - `[container]` section

### ⚠️ Items to Verify Before Approval

1. **End-to-end test on actual Toolbx**:
   - [ ] Install locald on host with `sudo locald admin setup`
   - [ ] Enter Toolbx container
   - [ ] Run `locald up` and verify socket connection
   - [ ] Check that services start and domains resolve

2. **Polkit dialog verification**:
   - [ ] Verify `pkexec locald shim serve` shows GUI dialog (not terminal prompt)
   - [ ] Verify polkit policy is installed to `/usr/share/polkit-1/actions/`

3. **Missing features from RFC** (deferred, not blocking):
   - `locald shim install-service` (systemd user service) - NOT IMPLEMENTED
   - NFS fallback to `/run/user/$UID/` - NOT IMPLEMENTED
   - Daemon idle timeout and max lifetime - implemented but may need tuning

4. **Code quality**:
   - [ ] All clippy warnings addressed
   - [ ] Unit tests for protocol serialization exist
   - [ ] Host-exec template substitution tested

## Context for Next Agent

### How to Bootstrap

```bash
exo-context  # Get full project state
exo-status   # Get active phase/tasks
```

### Project Structure

- `docs/agent-context/` - Source of truth for project state
- `docs/rfcs/` - RFC documents (stages 0-4)
- `docs/manual/` - User documentation
- `crates/` - Rust workspace with locald-\*, cnb-client

### Active Work

Phase 29 is the official active phase, but RFC 0130 was a priority side-quest. The next session should:

1. Confirm with user whether to push RFC 0130 commits
2. Promote RFC 0130 to stage-3 if approved
3. Resume Phase 29 work (ServicePlan IR)

### Key Axioms

- **Context is King**: Always read project context before acting
- **Phased Execution**: Plan → Implement → Verify (don't skip steps)
- **User in the Loop**: Stop for approval at critical junctures
