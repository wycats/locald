# Agent Handoff Document

**Created**: 2026-01-20
**Branch**: `phase-29.3-distributions`
**PR**: [#55 - docs: restructure around core value prop](https://github.com/wycats/locald/pull/55)
**Status**: All work committed and pushed, PR open and ready for review/merge

---

## Quick Start for New Session

```bash
# 1. Verify clean state
cd /var/home/wycats/Code/locald
git status  # Should show "nothing to commit, working tree clean"

# 2. Get project context
exo context  # Full context dump
exo status   # Quick snapshot

# 3. Run checks to verify everything still works
cargo xtask check  # Rust workspace checks
```

---

## Current State Summary

### Git State
- **Branch**: `phase-29.3-distributions` (up to date with `origin/phase-29.3-distributions`)
- **Working tree**: CLEAN - no uncommitted changes
- **Recent commits**:
  ```
  5869387 Fix duplicate distribution command
  b2c47d8 Stop committing UI assets
  ab25f9d chore: refresh embedded UI assets
  8547d8d proxy: add disabled-service landing page
  667f8a0 docs: clarify pin/monitor/disable + WSL scope
  ```

### PR #55 State
- **Title**: "docs: restructure around core value prop (HTTPS, experimental/roadmap split)"
- **State**: OPEN
- **Base**: `main`
- **Head**: `phase-29.3-distributions`
- **URL**: https://github.com/wycats/locald/pull/55
- **Action needed**: Review and merge (all checks should pass)

---

## What Was Accomplished This Session

### 1. Stopped Committing Built UI Assets
- Added `crates/locald-server/src/assets/` to `.gitignore`
- Removed files from git tracking (they still exist on disk but are ignored)
- Removed the `build-assets` pre-commit hook from `lefthook.yml`
- Normalized pnpm PATH in hooks to use proto shims

### 2. Added Dev Host Workflow
Modified `crates/locald-server/src/proxy.rs` to add dev-host routing:
- `dev.locald.localhost` → `localhost:5173` (Vite dev server for dashboard)
- `dev.docs.localhost` → `localhost:4321` (Astro dev server for docs)
- Both support WebSocket proxying for HMR
- Controlled by `DEV_UI=1` environment variable

### 3. Fixed Clippy Warning
Changed `map_or` to `is_ok_and` in `dev_ui_enabled()` function to satisfy clippy.

### 4. Resolved Rebase Conflicts
- Rebased onto `origin/main`
- Resolved merge conflict in `crates/locald-cli/src/main.rs` (comment duplication)
- Removed duplicate `DistributionCommands` enum from `crates/locald-cli/src/cli.rs` (35 lines)

---

## Key Files Modified (This Session)

| File | Change |
|------|--------|
| `.gitignore` | Added `crates/locald-server/src/assets/` |
| `lefthook.yml` | Removed `build-assets` hook, normalized pnpm PATH |
| `crates/locald-server/src/proxy.rs` | Dev host routing, clippy fix |
| `crates/locald-cli/src/main.rs` | Resolved merge conflict |
| `crates/locald-cli/src/cli.rs` | Removed duplicate enum |

---

## Project Context

### What is locald?
A daemon-first local development environment:
- Single `locald up` command starts all services
- Stable `*.localhost` domains with automatic HTTPS
- Built-in dashboard at `locald.localhost`
- Built-in docs at `docs.localhost`

### Active Phase
**Phase 29: Extensibility & Plugins** (RFC 0028)
- Task 29.1 (Plugin Mechanism): ✅ Complete
- Task 29.2 (Packaging): ✅ Complete
- Task 29.3 (Distributions): Pending

### Key Tools
- `exo` CLI for project management (context, phases, tasks)
- `cargo xtask check` for workspace validation
- `lefthook` for git hooks
- pnpm via proto shims at `$HOME/.proto/shims`

---

## Environment Notes

### Previous Environment (Toolbox)
This session ran in a Fedora Toolbox container. User is migrating off toolbox.

### Key Paths
- Workspace: `/var/home/wycats/Code/locald`
- Proto shims: `$HOME/.proto/shims` (for pnpm)
- Rust: `/home/wycats/.cargo/bin`

### Frontend Projects
| Project | Path | Dev Server |
|---------|------|------------|
| Dashboard | `locald-dashboard/` | port 5173 (Vite) |
| Docs | `locald-docs/` | port 4321 (Astro) |

---

## Verification Commands

```bash
# Full workspace check
cargo xtask check

# Individual checks
cargo fmt --all --check
cargo clippy -p locald-server -- -D warnings

# Dashboard
cd locald-dashboard && pnpm lint && pnpm check && pnpm build

# Docs
cd locald-docs && pnpm build
```

---

## Next Steps (User Decision Required)

1. **Merge PR #55** - All work is committed and pushed
2. **Continue Phase 29** - Distributions work (29.3.x tasks) is pending
3. **Or** - User may have different priorities post-migration

---

## Important Axioms (from AGENTS.md)

1. **Context is King** - Always read `docs/agent-context/` first
2. **Phased Execution** - Plan → Implement → Verify
3. **User in the Loop** - Stop for feedback at critical points
4. **Sandbox Always** - Use `--sandbox=<name>` for testing
5. **No Blunt Kill** - Use `locald shutdown/restart` not `kill`
6. **Read-Only TOML** - Use `exo` CLI to modify plan files, never edit directly

---

## Files to Know

| File | Purpose |
|------|---------|
| `AGENTS.md` | Agent workflow instructions |
| `docs/agent-context/plan.toml` | Project roadmap |
| `docs/agent-context/current/implementation-plan.toml` | Current phase tasks |
| `docs/agent-context/axioms.*.toml` | Project principles |
| `.github/copilot-instructions.md` | Copilot context management |

---

## Session Artifacts

All changes are committed. No temporary files or artifacts need cleanup.

The `.context/` directory (where this file lives) is for agent handoff/context artifacts.
