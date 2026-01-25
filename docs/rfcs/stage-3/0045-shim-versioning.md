---
title: "Shim Versioning & Upgrades"
stage: 3
feature: Security
---

# Design: Shim Versioning & Upgrades

## The Problem

`locald` uses a privilege separation architecture where a small, root-owned binary (`locald-shim`) handles privileged operations (like binding port 80) before handing control to the unprivileged `locald` daemon.

Because `locald-shim` is owned by root (setuid), it cannot be updated by the regular user during a standard `locald` update (e.g., via `cargo install` or a user-level package manager).

This creates a synchronization problem:

1.  The user updates `locald` to a new version.
2.  The new `locald` expects the shim to behave in a certain way (e.g., a new IPC protocol, a new capability, or a security fix).
3.  The installed `locald-shim` is still the old version.
4.  **Result**: The system fails silently, behaves unpredictably, or remains vulnerable to security issues fixed in the new shim.

Currently, `locald-shim` delegates all unknown commands to `locald`. This means running `locald-shim --version` actually reports the _daemon's_ version, masking the fact that the shim itself might be outdated.

## The Solution

We will implement a strict version handshake between the `locald` daemon and the `locald-shim`.

### 1. Shim Self-Reporting

The `locald-shim` binary will be updated to handle a specific `version` command (or `--shim-version` flag) that:

1.  Prints its own version (defined in its `Cargo.toml`).
2.  Exits immediately.
3.  **Crucially**: Does _not_ delegate to `locald`.

### 2. Daemon Verification

When `locald` starts up (specifically when it prepares to use privileged features), it will:

1.  Check for the existence of the shim.
2.  Execute `locald-shim --shim-version`.
3.  Compare the output against a compiled-in `EXPECTED_SHIM_VERSION`.

### 3. User Notification & Auto-Fix

If the versions do not match (or if the shim is too old to support the version flag), `locald` will:

1.  **Attempt Auto-Fix** (Interactive Only):

    - If running in an interactive terminal, `locald` will attempt to run `sudo locald admin setup` automatically.
    - This prompts the user for their password to perform the update immediately.
    - If successful, execution continues seamlessly.

2.  **Fallback to Error**:
    - If the auto-fix fails or if running non-interactively (e.g., CI/CD), `locald` will **abort**.
    - **Error Message**: Display a clear, actionable error message instructing the user to update the shim.

> "The installed locald-shim (v0.1.0) is incompatible with this version of locald (requires v0.2.0). Please run `sudo locald admin setup` to update the shim."

## Versioning Strategy

We will use **Semantic Versioning** for the shim, but with a specific policy on when the version _must_ be bumped.

### When to Bump the Shim Version

The shim version is independent of the `locald` product version. It should only change when the _contract_ or _security properties_ of the shim change.

A version bump is **REQUIRED** if:

1.  **Protocol Change**: The arguments passed from `locald` to `locald-shim` change (e.g., adding a new subcommand like `locald-shim server reload`).
2.  **Capability Change**: The set of Linux capabilities raised or dropped by the shim changes.
3.  **Security Fix**: A vulnerability is patched in the shim code itself.
4.  **Logic Change**: Any change to the internal logic of the shim (e.g., how it checks for ports, how it handles UIDs).

A version bump is **NOT REQUIRED** if:

1.  Only the `locald` daemon logic changes.
2.  Documentation or comments in the shim source code change.

### Compatibility Policy

To minimize user friction (requiring `sudo`), we will strive for **backward compatibility** where possible, but default to **strict equality** for safety until we have a proven need for ranges.

- **Initial Phase**: Strict equality. `locald` vX expects Shim vY. If `shim_version != expected_version`, fail.
- **Future Phase**: SemVer ranges. `locald` vX expects Shim `^1.2.0`.

## Implementation Plan

1.  **Modify `locald-shim`**:

    - Add `clap` or manual arg parsing to handle `--shim-version`.
    - Output `CARGO_PKG_VERSION`.

2.  **Modify `locald-cli` Build Process**:

    - In `build.rs`, read `../locald-shim/Cargo.toml`.
    - Set `LOCALD_EXPECTED_SHIM_VERSION` env var.

3.  **Modify `locald-cli` Runtime**:

    - In `ServerCommands::Start`, before attempting to use the shim:
      - Run `locald-shim --shim-version`.
      - Check exit code and output.
      - If check fails, print the "Please run sudo locald admin setup" message and exit.

4.  **Update `admin setup`**:
    - Ensure `admin setup` always overwrites the binary, even if it looks present (to ensure upgrade).

## Context Updates (Stage 3)

List the changes required to `docs/agent-context/` to reflect this feature as "current reality".

- [ ] Create `docs/agent-context/features/shim-versioning.md`
- [ ] Update `docs/agent-context/architecture/security.md` (if exists) or create it.
- [ ] Update `docs/agent-context/plan-outline.md` to mark this work as complete.
