# RFC 0109 (Stage 0): locald doctor

## Summary

Introduce a `locald doctor` command that reports host readiness and actionable fixes for running `locald` reliably, especially for privileged operations (shim availability, container execution, and cgroup hierarchy enforcement).

This RFC is Stage 0: an idea for review and refinement.

## Motivation

A growing number of `locald` features depend on a correctly installed privileged helper:

- `locald-shim` must be present, owned by root, and setuid (the “Leaf Node” privileged helper).
- Container execution relies on the shim.
- Phase 99 (RFC 0099) cgroup enforcement relies on a configured cgroup root and a privileged shim.

Today, when these prerequisites are missing, users can experience confusing partial failures (or silent degraded behavior). A single command that diagnoses the environment and points to the exact fix would reduce support burden and improve trust.

## Goals

- Provide a single, obvious entry point to validate the locald environment.
- Make “no privileged shim available” the first-class, top-priority diagnosis.
- Report cgroup v2 availability and Phase 99 readiness.
- Print actionable commands to fix issues.
- Support non-interactive output suitable for CI or bug reports.

## Non-Goals

- Automatically performing privileged fixes from `locald doctor` by default.
- Making the daemon run as root.
- Replacing existing `locald admin setup` behavior.

## Proposed UX

- `locald doctor`
  - Human-oriented output (grouped checks, clear PASS/FAIL).
  - Exit code non-zero if any critical check fails.

Optional extensions:

- `locald doctor --json`
  - Machine-readable results.
- `locald doctor --verbose`
  - Includes extra environment details.

## Checks (Initial Set)

### Shim readiness (critical)

- Can we locate a shim using the strict discovery rules?
- Is it owned by root and setuid?
- Does the shim version match what locald expects?

If this fails, doctor should recommend:

- sudo locald admin setup

### Cgroup readiness (critical for Phase 99 enforcement)

- Is cgroup v2 available? (e.g. /sys/fs/cgroup/cgroup.controllers exists)
- Is the expected locald cgroup root present?
  - systemd strategy: /sys/fs/cgroup/locald.slice exists
  - direct strategy: /sys/fs/cgroup/locald exists

If this fails, doctor should recommend:

- sudo locald admin setup

### Optional: runtime basics

- Is the daemon reachable? (socket present / ping ok)
- Is sandbox mode configured as expected? (when relevant)

## Open Questions

- Should `locald doctor` attempt safe auto-fixes when running as root?
  - Example: if shim exists but permissions are wrong, can it guide or repair?
- Should it check for additional kernel features needed by `libcontainer` execution (mount options, namespaces), or keep scope narrow initially?
- Where should “doctor” live relative to existing admin commands?

## Implementation Sketch

- Add a new CLI subcommand `doctor`.
- Reuse existing shim discovery and version checks.
- Reuse existing cgroup strategy detection and root readiness checks.
- Emit a structured internal report that can be rendered as text or JSON.

## Acceptance Criteria

- On a host with no shim configured, `locald doctor` clearly indicates that the shim is missing/not privileged and recommends running sudo locald admin setup.
- On a host with shim configured but cgroup root missing, `locald doctor` indicates cgroup root is not ready and recommends sudo locald admin setup.
- On a fully configured host, `locald doctor` reports all checks passing and exits 0.
