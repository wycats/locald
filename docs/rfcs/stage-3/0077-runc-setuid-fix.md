# RFC 0077: Runc Setuid Rootless Detection Fix

## Status

- **Status**: Recommended
- **Date**: 2025-12-08
- **Author**: GitHub Copilot

## Context

`locald` uses `runc` to execute containers. To support privileged operations (like binding ports < 1024 or creating namespaces) without requiring the entire daemon to run as root, we use a setuid helper binary called `locald-shim`.

The shim is designed to:

1.  Be owned by `root:root`.
2.  Have the setuid bit set (`chmod u+s`).
3.  Execute `runc` with effective root privileges.

## The Problem

When `locald-shim` executed `runc`, `runc` failed with the error:
`rootless container requires user namespaces`

This occurred even though `locald-shim` was running with Effective UID (EUID) 0.

## Investigation

1.  **Instrumentation**: We added logging to `locald-shim` and confirmed it was running with `UID=1000` (Real) and `EUID=0` (Effective).
2.  **The "Uncanny Valley" of Permissions**: `runc` (specifically its C-based `nsexec` stage) is designed to be secure. If it detects a mismatch between Real UID and Effective UID (a "setuid" state), it often drops privileges to the Real UID early in the initialization process to avoid accidentally running with elevated privileges in an ambiguous context.
3.  **The Failure Chain**:
    - `locald-shim` starts `runc` with `EUID=0` (Root) but `RUID=1000` (User).
    - `runc` initializes and detects the mismatch.
    - `runc` drops privileges to match the Real UID (`EUID` becomes 1000).
    - `runc` checks `isRootless()` (which checks `geteuid() != 0`). It returns `true`.
    - `runc` enters "Rootless Mode" logic.
    - It validates the container configuration. Since `locald` requested User Namespaces (expecting root privileges to set them up), `runc` checks if it can satisfy this in rootless mode.
    - It fails because the user environment lacks the necessary rootless configuration (e.g., `/etc/subuid`), resulting in the error: `rootless container requires user namespaces`.

## The Solution

We modified `locald-shim` to explicitly promote the Real UID to root before executing `runc`.

```rust
// locald-shim/src/main.rs

if command == "runc" {
    // Ensure we are fully root before calling runc
    if Uid::effective().is_root() {
        setgid(Gid::from_raw(0))?; // Set Real GID to 0
        setuid(Uid::from_raw(0))?; // Set Real UID to 0
    }
    // ... exec runc
}
```

By setting `RUID=0`, we align the process identity. `runc` sees `RUID=0` and `EUID=0`, treats the process as "Real Root", and does not drop privileges. It then correctly identifies that it is running in "Root Mode" and uses its privileges to apply the requested User Namespace mappings directly.

## References

- `runc` source code (`libcontainer/rootless_linux.go`) checks `os.Geteuid()`.
- The failure occurs because `runc` drops privileges to `RUID` before this check if it detects a setuid context.
- This highlights a core Unix principle: **EUID is for _doing_ things, RUID is for _being_ someone.** `runc` uses RUID to determine the identity and mode of the process.

## Impact

- **Security**: The shim is already setuid root. Promoting RUID to 0 does not grant _more_ privileges than EUID 0 already allows (since EUID 0 allows setting RUID to 0). It simply aligns the process state with `runc`'s expectations.
- **Reliability**: `runc` execution is now robust and does not depend on the host's rootless container configuration (`/etc/subuid`), making `locald` more portable.
