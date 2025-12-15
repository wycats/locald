# Phase 32 Implementation Plan: Sandbox Environments

**RFC**: [docs/rfcs/0031-sandbox-environments.md](../../rfcs/0031-sandbox-environments.md)

## Goal

Enable isolated `locald` instances for testing, CI, and parallel development without conflicting with the main system instance.

## 1. The `--sandbox` Flag

- **Objective**: Allow running `locald` in an isolated environment via a CLI flag.
- **Tasks**:
  - Add `--sandbox <NAME>` global flag to `Cli` struct.
  - Implement `setup_sandbox(name)` to configure `XDG_*` and `LOCALD_SOCKET` paths.
  - Ensure `LOCALD_SOCKET` env var is ignored unless `LOCALD_SANDBOX_ACTIVE` is set (Safety).

## 2. Safety Mechanisms

- **Objective**: Prevent accidental leakage of environment variables into the main daemon.
- **Tasks**:
  - Set `LOCALD_SANDBOX_ACTIVE=1` when running in sandbox mode.
  - Update `locald-core` to panic/exit if `LOCALD_SOCKET` is set but `LOCALD_SANDBOX_ACTIVE` is missing.
  - Update `AGENTS.md` to mandate sandbox usage for testing.

## 3. Verification

- **Objective**: Ensure the sandbox works as expected.
- **Tasks**:
  - Update integration tests (`postgres_test`, `dependency_test`) to use the sandbox mechanism.
  - Verify manual usage.
