# Shim Versioning & Upgrades

`locald` uses a privilege separation architecture where a small, root-owned binary (`locald-shim`) handles privileged operations (like binding port 80) before handing control to the unprivileged `locald` daemon.

To ensure security and stability, `locald` enforces a strict version handshake with the shim.

## The Problem

Because `locald-shim` is owned by root (setuid), it cannot be updated by the regular user during a standard `locald` update. This creates a synchronization problem where the daemon might be newer than the shim, leading to protocol mismatches or security vulnerabilities.

## The Solution

`locald` implements a strict version handshake:

1.  **Shim Self-Reporting**: The `locald-shim` binary handles a `--shim-version` flag that prints its own version (defined in its `Cargo.toml`) and exits immediately without delegating to the daemon.
2.  **Build-Time Embedding**: The `locald` CLI embeds the expected shim version at build time (by reading `crates/locald-shim/Cargo.toml`).
3.  **Runtime Verification**: When `locald` starts up and prepares to use privileged features, it executes `locald-shim --shim-version` and compares the output against the expected version.
4.  **Strict Enforcement**: If the versions do not match, `locald` aborts the operation and instructs the user to run `sudo locald admin setup` to update the shim.

## Versioning Policy

The shim version is independent of the `locald` product version. It is bumped only when:

1.  **Protocol Change**: The arguments passed from `locald` to `locald-shim` change.
2.  **Capability Change**: The set of Linux capabilities raised or dropped by the shim changes.
3.  **Security Fix**: A vulnerability is patched in the shim code itself.
4.  **Logic Change**: Any change to the internal logic of the shim.

Currently, `locald` enforces **strict equality** between the expected and installed shim versions.
