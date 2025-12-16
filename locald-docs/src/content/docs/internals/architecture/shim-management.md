---
title: Shim Management
---

# Shim Management

The `locald-shim` is a critical security component that allows `locald` to perform privileged operations (like binding ports < 1024 or creating containers) without running the entire daemon as root.

## The Golden Rule

**The `locald-shim` binary must be owned by `root` and have the setuid bit set (`chmod u+s`).**

## Development Workflow

When developing `locald`, you will frequently rebuild binaries using `cargo run` or `cargo build`. This creates a conflict:

1.  **Cargo's Behavior**: Cargo rebuilds binaries in `target/debug/` or `target/debug/build/...`. These new binaries are owned by your user and **do not** have the setuid bit.
2.  **Locald's Requirement**: `locald` needs a setuid shim to function correctly for privileged tasks.

### How `locald` Finds the Shim

To solve this, `locald` uses the following discovery logic:

1.  **Environment Variable (`LOCALD_SHIM_PATH`)**:

    - Used primarily by `cargo run` (via `build.rs`).
    - **Validation**: `locald` checks if the file at this path is setuid root.
    - **Behavior**:
      - If valid (setuid): Uses it.
      - If invalid (not setuid): **Ignores it** and logs a warning.

2.  **Sibling Discovery**:

    - Checks for `locald-shim` in the same directory as the `locald` executable.
    - **Validation**: Checks if the file is setuid root.
    - **Behavior**: If valid, uses it.

3.  **Path Lookup**:
    - Falls back to searching `PATH` for `locald-shim`.

### The "Shim Dance"

When you modify `locald-shim` source code:

1.  **Rebuild**: `cargo build` will create a new, non-setuid binary.
2.  **Setup**: You must manually (or via helper) update the permissions of this new binary.

**Recommended Command**:

```bash
# Rebuilds and sets up permissions for the debug binary
sudo target/debug/locald admin setup
```

### Agent Protocol

Agents must adhere to the following protocol to avoid "it works on my machine" (or "it works in dev but fails in prod") issues:

1.  **Never assume `cargo run` sets up permissions.** It does not.
2.  **If `locald-shim` is modified**, explicitly run the setup command.
3.  **When debugging shim issues**, verify permissions first: `ls -l path/to/shim`.

## Shim Capabilities

The shim is not just for port binding. It is a general-purpose privileged helper that supports multiple subcommands:

### 1. `bind` (Port Binding)

Allows `locald` to bind to privileged ports (80, 443) and pass the file descriptor back to the daemon.

### 2. `bundle` (Container Execution)

Allows `locald` to execute OCI bundles using an embedded container runtime.

- **Why**: Namespace/cgroup setup often requires root privileges, especially on systems with SELinux.
- **Safety**: The shim is the privileged leaf node; it runs the embedded runtime and applies user mapping via the OCI spec.

## Versioning

The shim protocol is versioned. `locald` checks the version of the shim on startup to ensure compatibility.

- **Command**: `locald-shim --shim-version`
- **Check**: If the shim version is older than what `locald` requires, `locald` will refuse to start and instruct the user to run `sudo locald admin setup` to update the shim.
