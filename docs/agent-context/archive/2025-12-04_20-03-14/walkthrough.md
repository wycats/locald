# Phase 32 Walkthrough: Sandbox Environments

**RFC**: [docs/rfcs/0031-sandbox-environments.md](../../rfcs/0031-sandbox-environments.md)

## Changes

### 1. The `--sandbox` Flag

We added a global `--sandbox <NAME>` flag to the `locald` CLI. When used, it:

1.  Resolves a sandbox directory at `~/.local/share/locald/sandboxes/<NAME>`.
2.  Creates `data`, `config`, and `state` subdirectories.
3.  Sets `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, and `LOCALD_SOCKET` to point to this sandbox.
4.  Sets `LOCALD_SANDBOX_ACTIVE=1` to indicate that we are in a safe, isolated mode.

### 2. Safety Enforcement

To prevent accidental environment leakage (e.g., a user setting `LOCALD_SOCKET` manually and forgetting about it, or an agent doing so), we implemented a strict check in `locald-core`:

- If `LOCALD_SOCKET` is set in the environment...
- ...BUT `LOCALD_SANDBOX_ACTIVE` is NOT set...
- ...The process will print an error and exit with code 1.

This ensures that custom sockets can *only* be used when the `--sandbox` flag (which sets `LOCALD_SANDBOX_ACTIVE`) is explicitly provided.

### 3. Testing Updates

We updated `AGENTS.md` to mandate the use of `--sandbox` for all testing. We also updated the integration tests (`postgres_test.rs` and `dependency_test.rs`) to comply with the new safety requirements by setting `LOCALD_SANDBOX_ACTIVE=1` when they set up their test environments.
