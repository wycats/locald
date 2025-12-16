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

To ensure security and consistency, `locald` uses a **strict discovery logic**:

1.  **Sibling Discovery**:

    - Checks for `locald-shim` in the same directory as the `locald` executable.
    - **Validation**: Checks if the file is setuid root.
    - **Behavior**: If valid, uses it.

2.  **Parent Discovery (Testing Only)**:
    - Checks for `locald-shim` in the parent directory of the executable.
    - This is strictly to support `cargo test` where the test binary is in `target/debug/deps/` and the shim is in `target/debug/`.

**Note**: Previous versions supported a `LOCALD_SHIM_PATH` environment variable. This has been **removed** to prevent "it works on my machine" issues and security bypasses. The shim must be installed correctly relative to the binary.

## Embedded Distribution (RFC 0078)

To simplify installation and updates, the `locald-shim` binary is **embedded** directly into the `locald` executable at compile time.

### How it Works

1.  **Build Time**: `locald-cli/build.rs` compiles `locald-shim` in release mode and passes its path to the compiler. The `locald` binary includes these bytes via `include_bytes!`.
2.  **Installation**: When a user runs `sudo locald admin setup`:
    - It extracts the embedded shim binary to the same directory as the `locald` executable (e.g., `~/.cargo/bin/locald-shim`).
    - It sets the necessary permissions (`chown root:root`, `chmod 4755`).

### The "Shim Dance"

When you modify `locald-shim` source code:

1.  **Rebuild**: `cargo build` will create a new, non-setuid binary.
2.  **Setup**: You must manually (or via helper) update the permissions of this new binary.

**Recommended Command**:

```bash
# Rebuilds and sets up permissions for the debug binary
sudo target/debug/locald admin setup

This installs/repairs the privileged shim and also configures the cgroup root needed for Phase 99 cgroup lifecycle enforcement.
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

**Security Note**: The shim MUST set `FD_CLOEXEC` on the file descriptor before passing it (or the receiver must set it immediately). Failure to do so will cause the privileged port to leak into child processes spawned by `locald` (e.g., Postgres), preventing the port from being released even if `locald` restarts.

### 2. `bundle run` (Container Execution)

Allows `locald` to execute an OCI bundle using an embedded `libcontainer` runtime.

- **Command**: `locald-shim bundle run --bundle <PATH> --id <ID>`
- **Legacy (still supported)**: `locald-shim bundle <bundle-path>`

- **Why**: Container creation requires root privileges to set up namespaces and cgroups correctly.
- **Mechanism**:
  - The shim starts with `RUID=user`, `EUID=root`.
  - It accepts a path to an OCI Bundle (directory containing `config.json` and rootfs) and a stable container id.
  - It uses the `libcontainer` Rust crate to load the spec and execute the container directly.
  - **Pivot**: It handles the pivot from Root to the Container User securely.
- **Architecture**: This is the "Fat Shim" model (RFC 0098). The shim _is_ the runtime.

### 3. `admin sync-hosts` (Hosts File Management)

Allows `locald` to update `/etc/hosts` with domains for running services.

- **Why**: Writing to `/etc/hosts` requires root privileges.
- **Mechanism**:
  - The shim receives the list of domains as arguments.
  - It directly updates `/etc/hosts` (using internal logic) to map these domains to `127.0.0.1`.
  - It does **not** exec `locald` to do this (adhering to the Leaf Node Axiom).

### 4. `admin cleanup` (Privileged Cleanup)

Allows `locald` to recursively remove directories that contain root-owned files (created by containers).

- **Why**: Containers running as root (even in user namespaces) may create files or mount points (like `/run/shm`) owned by root. The unprivileged daemon cannot delete these.
- **Mechanism**:
  - The shim accepts an absolute path to delete.
  - **Security**: It enforces strict validation:
    - Path must be absolute.
    - Path must contain `.locald` as a path segment (scoping it to locald-managed directories).
  - It performs `rm -rf` on the target.

### 5. `admin cgroup` (Cgroup Root Setup + Kill)

Allows `locald` to establish the cgroup v2 root used by the “Tree of Life” hierarchy and to reliably kill/prune a service’s entire process tree.

- **Why**: cgroup hierarchy creation and cgroup-wide kill operations require privileged access on most hosts.
- **Commands**:
  - `locald-shim admin cgroup setup`
  - `locald-shim admin cgroup kill --path <absolute-cgroupsPath>`

These commands are invoked indirectly by `sudo locald admin setup` (for setup) and by service stop/restart paths (for kill/prune) when the host is configured.

## Architecture: The Leaf Node Axiom

As of RFC 0096, `locald-shim` is designed as a **Leaf Node**.

1.  **No Recursion**: The shim performs its task and exits. It **never** executes the `locald` binary.
2.  **No Wrapping**: `locald` (the daemon) runs as an unprivileged user process. It does **not** run inside the shim.
3.  **On-Demand Privilege**: When `locald` needs a privileged resource (like a port < 1024), it calls the shim specifically for that resource (e.g., via `bind` command which passes a file descriptor back).

This architecture prevents "fork bombs" and simplifies the privilege model. The daemon is always unprivileged; the shim is always short-lived and focused.

## Versioning

The shim protocol is versioned. `locald` checks the version of the shim on startup to ensure compatibility.

- **Command**: `locald-shim --shim-version`
- **Check**: If the shim version is older than what `locald` requires, `locald` will refuse to start and instruct the user to run `sudo locald admin setup` to update the shim.
