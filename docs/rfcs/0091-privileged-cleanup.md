---
title: "Privileged Cleanup via Shim"
stage: 3 # Recommended
feature: Architecture
---

# RFC 0091: Privileged Cleanup via Shim

## 1. Summary

This RFC introduces a mechanism for `locald` to clean up root-owned files created by container runtimes within the user's project directory. This is achieved by adding a restricted `admin cleanup` command to the privileged `locald-shim`.

## 2. Motivation

`locald` uses `runc` to execute containers. Even when using User Namespaces, `runc` (running as root via the shim) may create directories on the host filesystem (e.g., for bind mounts or `tmpfs` mount points like `/run/shm`) that are owned by `root:root`.

When `locald` (running as an unprivileged user) attempts to clean up these directories during a service restart or project reset, it fails with `Permission denied (os error 13)`. This leaves the system in a broken state where the service cannot be restarted.

Example crash:

```
Error: Failed to remove data directory
Caused by: Permission denied (os error 13)
```

## 3. Detailed Design

### 3.1 The Problem: Mixed Ownership

The `.locald` directory in a user's project is intended to be managed by the user. However, the OCI runtime spec allows defining mount points. If a mount point (like `/run/shm`) does not exist in the rootfs, the runtime (running as root) creates it.

If the runtime crashes or is killed before it can clean up, these root-owned directories persist.

### 3.2 The Solution: Delegated Cleanup

We extend `locald-shim` with a new subcommand: `admin cleanup`.

#### Shim Command

```bash
locald-shim admin cleanup <absolute-path>
```

**Security Constraints:**
To prevent arbitrary file deletion (e.g., `locald-shim admin cleanup /etc`), the shim enforces strict validation:

1.  **Absolute Path**: The path must be absolute.
2.  **Scope Restriction**: The path must contain `.locald` as a path segment. This ensures we only delete files within `locald`'s managed directories.
3.  **No Traversal**: The path must not contain `..`.

If validation passes, the shim performs a recursive removal (`rm -rf`) of the target path using its root privileges.

#### Server Logic

The `locald-server` logic for directory removal is updated to use a "Try-Catch-Delegate" pattern:

1.  **Try**: Attempt standard `tokio::fs::remove_dir_all(path)`.
2.  **Catch**: If it fails with `PermissionDenied`...
3.  **Delegate**: Call `locald-shim admin cleanup <path>`.

### 3.3 Implementation Details

**Shim (`locald-shim/src/main.rs`)**:

```rust
if command == "admin" && args.get(1) == Some("cleanup") {
    let path_str = args.get(2).ok_or(...)?;
    let path = Path::new(path_str);

    // Security Checks
    if !path.is_absolute() { bail!("Path must be absolute"); }
    if !path.components().any(|c| c.as_os_str() == ".locald") {
        bail!("Path must be within a .locald directory");
    }

    // Execute
    std::fs::remove_dir_all(path)?;
}
```

**Server (`locald-server`)**:
The `ProcessManager` and `ContainerManager` will use a helper function `locald_utils::fs::force_remove_dir_all` (or similar) that encapsulates this logic.

## 4. Alternatives Considered

- **`sudo rm -rf`**: Requires interactive password entry, breaking the daemon's autonomy.
- **Rootless `runc`**: While desirable, true rootless `runc` requires complex host configuration (`/etc/subuid`, `newuidmap`) that `locald` aims to avoid requiring.
- **Namespace Cleanup**: We could try to enter the mount namespace to clean up, but if the process is dead, the namespace is gone, leaving the artifacts on the host.

## 5. Security Implications

This feature gives the unprivileged user the ability to delete root-owned files. The security relies entirely on the **Scope Restriction**.

- **Risk**: If a user can create a directory named `.locald` in `/etc` (e.g., `/etc/.locald`), they could delete it.
- **Mitigation**: Regular users cannot create directories in `/etc`.
- **Risk**: Symlink attacks.
- **Mitigation**: `remove_dir_all` in Rust is generally safe against symlink races, but we should ensure we don't follow symlinks out of the allowed area. The primary protection is that we are deleting _within_ a user-controlled area. If the user symlinks `.locald/foo -> /etc`, and we delete `.locald/foo`, we remove the symlink, not the target.

## 6. Future Work

- **Scoped Sandbox**: Restrict cleanup to specific known paths (e.g., `~/.local/share/locald` and registered project paths).
