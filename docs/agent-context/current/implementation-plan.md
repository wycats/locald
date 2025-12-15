# Implementation Plan - Phase 97: Remove `runc` (Libcontainer “Fat Shim”)

**Goal**: Remove the external `runc` dependency and execute OCI bundles using an embedded `libcontainer` runtime inside `locald-shim`, per RFC 0098.

**Primary RFC**: [RFC 0098: Libcontainer Execution Strategy](../../rfcs/0098-libcontainer-execution-strategy.md)

**Supporting RFCs / Constraints**:

- [RFC 0096: Shim Execution Safety (Leaf Node Axiom)](../../rfcs/0096-shim-execution-safety.md)
- [RFC 0097: Strict Shim Discovery](../../rfcs/0097-strict-shim-discovery.md)

## 0. Scope

This phase is about **container execution plumbing** (shim/runtime/callers), not UI polish.

In-scope:

- Replace shim subcommands that shell out to `runc` with `libcontainer`-based execution.
- Update all callers (daemon + CNB/build tooling + tests/examples) to use the new shim interface.
- Ensure the system works on a machine **without** `runc` installed.

Out-of-scope:

- VMM work.
- Hot reload / config hierarchy upgrades (those are separate RFC/phase candidates).

## 1. Target Architecture

**Caller-Generates / Shim-Executes**:

- **Caller** (daemon/test harness) generates OCI bundle: `config.json` + `rootfs/` + state directory.
- **Shim** (setuid root leaf node) executes the bundle via embedded `libcontainer`.

**Privilege model**:

- Daemon must never prompt for sudo. If shim is not privileged/installed, daemon errors with an actionable message.
- Interactive commands may trigger inline sudo prompts for shim install/permission repair.

## 2. Shim Interface Design (minimal + explicit)

We want a small, stable surface area that does not leak accidental complexity from an external runtime.

Proposed shim commands (approved: Option A):

- `locald-shim bundle run --bundle <PATH> --id <ID>`

Notes:

- No `bundle delete` subcommand in the initial cut; rely on state-dir conventions + existing privileged cleanup.

Notes:

- We should keep the current “bind privileged port” behavior untouched (it’s already shim-native).
- Keep strict discovery rules: sibling/parent-only.

## 3. Work Breakdown

### 3.1 Inventory

- Identify every `runc` call site (daemon runtime, CNB runtime, examples, e2e tests).
- Decide what is “legacy compatibility” vs “delete now”.

### 3.2 Implement `libcontainer` in `locald-shim`

- Add `libcontainer` (Youki) as a dependency with conservative features.
- Implement bundle execution + lifecycle management.
- Preserve Leaf Node constraints (no `locald` re-exec; minimal shellouts).

### 3.3 Switch callers

- Update daemon container execution to call new shim bundle subcommands.
- Update CNB execution path (`cnb-client`) to use the same shim interface.

### 3.4 Verification

- Run existing e2e tests.
- Add/adjust an integration test that asserts execution succeeds when `runc` is absent.

## 4. Acceptance Criteria

- `locald` container execution succeeds on a host without `runc`.
- `locald-shim` remains a Leaf Node.
- Shim discovery remains strict; daemon never blocks on interactive sudo.
- CI/e2e coverage exercises the new path.
