# RFC 0110 (Stage 0): Privileged Capability Acquisition

## Summary

Introduce a project-wide design rule for privileged effects:

- All privileged operations (root-owned filesystem mutations, cgroup setup/kill, privileged binds, etc.) are only available through a *capability object* that can be acquired only after a readiness check.

This makes “shim not ready” failures correct-by-construction and keeps user-facing remediation simple (usually: `sudo locald admin setup`).

## Motivation

`locald` relies on a privileged helper (`locald-shim`) for security-critical operations. The project repeatedly encounters a failure pattern:

- callers drift into “best effort” privileged behavior
- readiness checks are repeated inconsistently
- the system silently degrades when prerequisites are missing

We want a single, consistent contract:

- if an operation needs privilege, you must prove privilege is available up-front
- if privilege is not available, errors must be explicit and actionable

## Goals

- Make privileged effects impossible to call without passing readiness checks.
- Centralize shim discovery/version/permission/usability checks.
- Enforce consistent remediation guidance (prefer a single canonical fix).
- Make it cheap to do the right thing for future privileged features.

## Non-Goals

- Automatically fixing privileged issues by default.
- Expanding scope into broad OS diagnosis.

## Design

### Axiom

**Privileged effects must go through an acquired capability.**

Corollaries:

- Code must not invoke privileged shim operations “inline” without first acquiring the capability.
- Any code path that requires privilege must fail early with a clear remediation message when capability acquisition fails.

### Capability API Shape (sketch)

- `PrivilegedShim::acquire() -> Result<PrivilegedShim, NotReady>`
  - checks:
    - strict shim discovery
    - version match (as appropriate)
    - root-owned + setuid
    - *usability smoke test* (proves real privilege; catches `nosuid` mounts, policy blocks)

- privileged operations become methods:
  - `PrivilegedShim::setup_cgroup_root()`
  - `PrivilegedShim::kill_cgroup(path)`
  - `PrivilegedShim::bind_privileged_port(...)`

### Readiness Error Contract

When acquisition fails:

- the primary remediation should be the canonical fix when applicable:
  - `sudo locald admin setup`
- diagnostics should provide enough detail to distinguish common “exists but unusable” cases:
  - `nosuid` mount preventing setuid elevation
  - security policy denial (SELinux/AppArmor)
  - systemd strategy selected when systemd is not PID 1 (CI/container heuristics)

## Relationship to `locald doctor`

`locald doctor` should reuse the same acquisition logic (and ideally the same underlying checks) so that:

- “doctor says OK” implies acquisition succeeds
- “doctor says FAIL” implies acquisition fails with the same remediation

This prevents drift between diagnostics and runtime behavior.

## Open Questions

- Where should capability acquisition live (e.g. `locald-utils`, `locald-cli`, or `locald-core`)?
- Should the smoke test be a dedicated shim subcommand (preferred) or a non-destructive privileged probe assembled by callers?
- How should the system report “cannot be fixed here” cases (e.g. unprivileged container constraints)?

## Acceptance Criteria

- There exists a single acquisition function that gates privileged operations.
- At least one existing privileged feature is refactored to use the capability object.
- Failure messages consistently point to the canonical remediation when appropriate.
