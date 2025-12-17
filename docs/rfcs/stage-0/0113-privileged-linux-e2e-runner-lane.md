---
title: Privileged Linux E2E Runner Lane
stage: 0
feature: CI
---


# RFC 0113: Privileged Linux E2E Runner Lane

## Idea

We have at least one integration test (Phase 99 cgroup cleanup) that only provides strong signal when it runs with real host privileges:

- writable cgroup v2
- a setuid-root `locald-shim`
- configured locald cgroup root (`sudo locald admin setup`)

Running this on generic hosted CI is unreliable because many environments disable setuid semantics, restrict `/sys/fs/cgroup` writes, or run inside containers where the host cgroup tree is not accessible.

This RFC proposes a dedicated, opt-in CI lane for privileged Linux E2E tests, backed by labeled self-hosted runners that we control.

## Goals

- Provide a stable place to run privileged tests that validate the real system contract (shim + cgroups + stop/cleanup).
- Keep normal CI fast and deterministic (privileged lane must not block typical PR workflows unless explicitly requested).
- Make the policy explicit: which tests require privileged runners, how we gate them, and who owns runner hygiene/security.

## Non-Goals

- Mandate implementation of self-hosted runners immediately.
- Require privileged tests to run on every PR by default.
- Solve every isolation/security concern in the first iteration.

## Proposed Policy

- Privileged E2E tests remain self-skipping by default in the codebase.
- A separate workflow/job runs them with an explicit force flag (e.g. `LOCALD_E2E_FORCE_CGROUP_CLEANUP=1`).
- That job targets runners labeled, for example: `self-hosted`, `linux`, `cgroupv2`, `privileged`.
- The job runs `sudo <locald> admin setup` (or an equivalent preparation script) before forced tests.

## Runner Requirements (Definition of “Known-Good”)

A runner is considered eligible for the privileged lane only if:

- Linux host (prefer VM, not container).
- cgroup v2 mounted and writable as root.
- Setuid semantics are honored for the shim location (no `nosuid` on relevant mounts).
- Either:
  - systemd is PID 1 (systemd strategy), or
  - direct cgroup driver mode is permitted (direct strategy).
- Exclusive job execution (or strong isolation) to avoid state/port collisions.

## Security & Hygiene

- The runner is treated as sensitive infrastructure (may execute arbitrary code from PRs depending on policy).
- Recommendation: only run the privileged lane on trusted branches / trusted PRs (e.g. maintainers) until a hardened approach is specified.
- Jobs must avoid leaving persistent state behind; if state is required, cleanup should be part of the workflow.

## Implementation Sketch

1. Add a CI workflow job `privileged-e2e` that is triggered by either:
   - `workflow_dispatch`, or
   - schedule/nightly, or
   - a PR label (maintainer-controlled).
2. Job runs on `runs-on: [self-hosted, linux, cgroupv2, privileged]`.
3. Build binaries, run `sudo ./target/<profile>/locald admin setup`, run forced tests.
4. Record pass/fail as a separate check that does not gate merges by default (initially).

## Open Questions

- Do we want the privileged lane to gate merges once stable?
- Do we need ephemeral runners (per-job VM) vs a long-lived host?
- What is the trust model for PRs (fork PRs, third-party contributions)?
