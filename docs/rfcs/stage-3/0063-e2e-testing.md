---
title: "E2E Testing Infrastructure"
stage: 3
feature: Testing
---

# Design: E2E Testing Infrastructure

## Context

We currently rely on ad-hoc bash scripts (`scripts/verify-*.sh`) and unit/integration tests (`locald-cli/tests/*.rs`) to verify functionality. However, as the system grows (CNB, Shim, Daemon, CLI), we need a more robust, unified way to verify "End-to-End" scenarios that involve the full stack in a production-like environment.

The current "integration tests" in `locald-cli` are good, but they often mock parts of the system or run in a way that doesn't fully replicate the user experience (e.g., they might not use the actual `locald-shim` or `runc` in the same way).

## Goals

1.  **Full Stack Verification**: Test the actual release binaries (`locald`, `locald-shim`) interacting with the OS.
2.  **Scenario-Based**: Define tests as "User Stories" (e.g., "User updates binary", "User runs CNB build").
3.  **CI Integration**: Run these tests reliably in GitHub Actions.
4.  **Debuggability**: When a test fails, preserve logs and state for inspection.

## CI Integration Notes

E2E tests run in CI under coverage instrumentation (`-C instrument-coverage`). This has a couple of practical consequences:

1.  **Privileged Shim Setup**: CI must install the setuid shim (`locald admin setup`) before running scenarios that require privileged operations.
2.  **Profile Integrity**: Instrumented processes write `*.profraw` on exit. If tests terminate background daemons abruptly (e.g. via SIGKILL), LLVM profiles can be truncated and become unreadable.

**Expectation**: E2E harnesses and integration tests should shut down any spawned `locald` daemons and service processes cleanly (signal + wait) so coverage profiles flush correctly.

**Enforcement**: CI treats corrupt `*.profraw` as a failure. If a test suite leaves behind truncated profiles, the coverage job will fail to force us to fix teardown semantics rather than masking the issue.

## Proposed Architecture

We will build a Rust-based E2E test harness (likely in a new `locald-e2e` crate or just `tests/e2e`) that:

1.  **Builds Artifacts**: Compiles `locald` and `locald-shim` in release mode (or uses pre-built ones).
2.  **Sets up Sandbox**: Creates a temporary directory structure mimicking `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, etc.
3.  **Manages Lifecycle**: Spawns the daemon, waits for readiness, runs CLI commands, and ensures cleanup.
4.  **Assertions**: Verifies stdout/stderr, exit codes, and side effects (files created, ports listening).

### Key Components

- **`TestContext`**: A struct that holds the temp dir, paths to binaries, and handles cleanup (Drop trait).
- **`LocaldRunner`**: A helper to run `locald` commands within the context.
- **`Scenario`**: A trait or pattern for defining multi-step tests.

## Scenarios to Cover

1.  **Basic Lifecycle**: `init` -> `up` -> `ping` -> `stop`.
2.  **Auto-Update**: The scenario we just fixed (binary update triggers restart).
3.  **CNB Build**: `locald build` with a sample app.
4.  **Shim Verification**: Ensure `locald-shim` is actually used for privileged ops.
5.  **Crash Recovery**: Kill the daemon and ensure it recovers state.

## Implementation Plan

- [ ] Create `locald-e2e` crate (or `tests/` folder).
- [ ] Implement `TestContext` and build logic.
- [ ] Port `scripts/verify-update.sh` to a Rust test.
- [ ] Add to CI pipeline.

```

```
