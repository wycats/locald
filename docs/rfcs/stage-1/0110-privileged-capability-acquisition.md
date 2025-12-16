---
title: Privileged Capability Acquisition
stage: 1
feature: Engineering Excellence
---

# RFC 0110: Privileged Capability Acquisition

## Status

- **Status**: Proposed (Stage 1)
- **Owner**: TBD

## 1. Summary

Define a project-wide rule for privileged effects:

- Any operation that relies on the privileged helper (`locald-shim`) or root-only host mutations is only available through an acquired capability object.

This makes readiness failures *correct-by-construction* and ensures that:

- privileged call sites all share the same readiness semantics
- `locald doctor` is not a separate subsystem; it renders the same readiness report used for acquisition
- fixes are consolidatable (prefer a single canonical remediation when appropriate)

## 2. Motivation

We repeatedly hit the same failure patterns:

- readiness checks get duplicated inconsistently across call sites
- some code paths degrade silently when prerequisites are missing
- users see confusing partial failures (especially on CI/containerized hosts)

We want one boring, enforced contract:

- If an operation needs privilege, it must fail early unless a capability is acquired.
- If capability acquisition fails, the system must provide a consolidated, actionable remediation.

## 3. Goals

- Make privileged effects impossible to invoke without passing readiness checks.
- Centralize shim discovery, version match, permission checks, and a minimal usability smoke test.
- Provide a structured report type whose output can be deduplicated into a small set of canonical fixes.
- Keep shared crates free of UI formatting; CLI owns human rendering.

## 4. Non-Goals

- Automatically repairing privileged issues by default.
- Turning readiness into a broad OS diagnostics system.
- Adding a large branching troubleshooting tree.

## 5. Axiom

**Privileged effects must go through an acquired capability.**

Corollaries:

- Code must not spawn or invoke the shim directly outside the capability module.
- Any code path requiring privilege must either:
  - accept `&Privileged` (or equivalent) from its caller, or
  - attempt acquisition and return a structured “not ready” error.

## 6. Detailed Design

### 6.1 Module Placement

Recommended layout:

- `locald-utils` (shared):
  - readiness probes + capability acquisition
  - structured report types (data model)
  - no human formatting / printing
- `locald-cli` (UX):
  - human renderer for reports
  - `--json` output serialization

### 6.2 Capability API Shape

The core entry point:

- `Privileged::acquire() -> Result<Privileged, NotReady>`

`Privileged`:

- contains a non-extractable shim handle (private field)
- exposes privileged effects as methods, e.g.:
  - `setup_cgroup_root()`
  - `kill_cgroup(path)`
  - other existing privileged effects

### 6.3 Readiness Report and Fix Consolidation

Capability acquisition should produce a structured report that can be rendered by doctor and reused by privileged call sites.

Recommended structure:

- `DoctorReport`:
  - `strategy`: detected host strategy (e.g. cgroup root strategy) + “why”
  - `mode`: `enabled | degraded`
  - `problems: Vec<Problem>`
  - `fixes: Vec<FixKey>` (deduplicated + prioritized)

- `Problem`:
  - `id`: stable identifier (`shim.missing`, `shim.permissions`, `shim.usability`, `cgroup.v2_missing`, ...)
  - `severity`: `critical | warning | info`
  - `evidence`: structured data or strings sufficient for debugging
  - `fix`: a `FixKey` categorizing the remediation

- `FixKey`:
  - small enum of consolidatable actions, e.g.:
    - `RunAdminSetup`
    - `HostPolicyBlocksPrivilegedHelper` (covers `nosuid`, SELinux/AppArmor denials, etc.)
    - `UnsupportedEnvironment` (e.g. unprivileged container constraints)

This enables doctor output to:

1. summarize problems
2. show a small number of canonical remediations

### 6.4 Usability Smoke Test (required)

In addition to on-disk checks (exists/version/owner/setuid), acquisition must include a minimal non-destructive smoke test proving “shim can actually do privileged work”.

This catches cases like `nosuid` mounts or policy enforcement where the shim appears correct but cannot elevate or cannot execute required privileged operations.

Preferred mechanism:

- add a dedicated shim subcommand (e.g. `locald-shim admin self-check`) that:
  - verifies effective uid is root
  - verifies any required host capabilities for the selected strategy
  - does not mutate long-lived state

If a dedicated subcommand is not available yet, acquisition may use a minimal privileged probe assembled by the caller, but the long-term goal is a stable shim-level contract.

### 6.5 Compile-Time Enforcement (preferred over lints)

To prevent drift without relying on policing:

- keep the shim path/handle private inside `Privileged`
- do not expose a public `shim_path()` accessor
- require privileged effects to be methods on `Privileged` (or require a private `ShimHandle` newtype)

This makes direct shim invocation impossible in most call sites.

## 7. Relationship to `locald doctor`

`locald doctor` should reuse the same acquisition/report logic so that:

- “doctor says OK” implies `Privileged::acquire()` succeeds
- “doctor says FAIL” implies acquisition fails with the same consolidated fixes

Doctor should be mostly a renderer and orchestrator, not a separate diagnosis engine.

## 8. Unresolved Questions

- Where should the shared report types live if we want them reused by the daemon without pulling in extra dependencies?
- What exact smoke test provides maximum signal with minimal host assumptions?

## 9. Acceptance Criteria

- A capability acquisition API exists and is used by at least one privileged feature.
- A structured report with consolidatable fixes exists and is used both by doctor rendering and privileged call sites.
- Direct shim invocation becomes structurally difficult (preferably impossible) outside the capability module.
