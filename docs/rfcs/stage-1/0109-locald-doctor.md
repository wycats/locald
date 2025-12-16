---
title: locald doctor
stage: 1
feature: Installation & Update Experience
---

# RFC 0109: locald doctor

## Status

- **Status**: Proposed (Stage 1)
- **Owner**: TBD

## 1. Summary

Add a `locald doctor` command that diagnoses host readiness for running `locald` and prints actionable fixes.

Initial scope focuses on the most common root causes of “mysterious” failures or degraded behavior:

- Privileged shim availability (`locald-shim` discovery + permissions + version)
- cgroup v2 availability and Phase 99 (RFC 0099) readiness
- Optional “runtime basics” checks (daemon reachability / sandbox environment clarity)

## 2. Motivation

`locald` increasingly relies on privileged and host-dependent features:

- `locald-shim` must exist, be root-owned, and be setuid for privileged operations.
- Container execution relies on the shim.
- Phase 99 cgroup enforcement relies on cgroup v2 and a configured locald cgroup root.

When these prerequisites are missing, users often see confusing partial failures (or silent fallbacks). A single command that diagnoses the environment and points to the exact fix reduces support burden and builds trust.

## 3. Goals

- Provide a single, obvious entry point to validate a `locald` installation.
- Make “no privileged shim available” a first-class, top-priority diagnosis.
- Report cgroup v2 availability and Phase 99 readiness.
- Print actionable “do this next” commands.
- Support machine-readable output for CI / bug reports.

## 4. Non-Goals

- Auto-fixing privileged issues by default.
- Making the daemon run as root.
- Replacing existing `sudo locald admin setup` behavior.
- Becoming a comprehensive OS diagnostics tool (system-wide kernel audit, etc.).

## 5. User Experience

### 5.1 CLI Surface

- `locald doctor`
  - Human-oriented output.
  - Exit non-zero if any **critical** checks fail.

Options:

- `locald doctor --json`
  - Machine-readable output.
- `locald doctor --verbose`
  - Includes extra environment details useful for debugging.

### 5.2 Output Principles

- Output is grouped by category.
- Each failed check includes:
  - What is wrong
  - Why it matters
  - The exact fix command(s)

## 6. Checks

### 6.1 Shim Readiness (critical)

Checks:

- Can we locate a privileged shim using the strict discovery rules?
- Is it root-owned and setuid?
- Does the shim version match what `locald` expects?

If any of these fail, doctor should recommend:

- `sudo locald admin setup`

### 6.2 Cgroup Readiness (critical for Phase 99 enforcement)

Checks:

- Is cgroup v2 available? (e.g. `/sys/fs/cgroup/cgroup.controllers` exists)
- Is the expected locald cgroup root present?
  - systemd strategy: `/sys/fs/cgroup/locald.slice` exists
  - direct strategy: `/sys/fs/cgroup/locald` exists

If this fails, doctor should recommend:

- `sudo locald admin setup`

### 6.3 Runtime Basics (non-critical initially)

Initial (optional) checks:

- Is the daemon reachable? (socket present / ping works)
- If sandbox mode is active, report sandbox identity (e.g. `LOCALD_SANDBOX_NAME`) to aid reproductions.

## 7. Exit Codes

- Exit `0` when all **critical** checks pass.
- Exit non-zero when any **critical** check fails.

In JSON mode, the exit code behavior is the same.

## 8. Data Model (for --json)

The JSON output should be a stable, structured report.

Proposed shape:

- `version`: locald version string (if available)
- `checks[]`:
  - `id`: stable string id (e.g. `shim.present`, `shim.permissions`, `cgroup.v2`, `cgroup.root_ready`)
  - `severity`: `critical | warning | info`
  - `status`: `pass | fail | skip`
  - `summary`: one-line summary
  - `details`: optional longer text
  - `remediation[]`: zero or more recommended commands (strings)

## 9. Implementation Plan (Stage 2)

- Implement `locald doctor` in `locald-cli`.
- Reuse existing logic:
  - shim discovery + version checks
  - cgroup detection + root readiness checks
  - daemon reachability/ping (if already available)
- Render either human text or JSON from a single internal report struct.

## 10. Drawbacks

- Adds a new “front door” command that must be kept accurate as features evolve.

## 11. Alternatives

- Rely on ad-hoc error messages scattered across commands.
- Add a one-shot diagnostic to `locald admin setup` only (but that’s less discoverable).

## 12. Unresolved Questions

- Should `locald doctor` support an explicit `--fix` mode in the future?
- Should we expand checks to include deeper kernel/runtime prerequisites for `libcontainer` (namespaces, mount options), or keep the scope intentionally narrow?
