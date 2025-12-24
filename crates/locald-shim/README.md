# locald-shim

**Vision**: The reliable anchor for ephemeral processes.

## Purpose

`locald-shim` is a small, privileged helper used by `locald` to perform operations that require elevated permissions.

On Linux it is typically installed as **setuid root**.

Design constraint: the shim is a **leaf node**. It must not `exec` the `locald` binary (to avoid recursion and privilege escalation).

## Key Components

- **Privileged Port Binding**: `bind` binds a privileged TCP port (e.g. 80/443) and passes the open FD back to `locald` over a Unix socket.
- **Hosts Management**: `admin sync-hosts` updates the `/etc/hosts` block managed by `locald`.
- **Container Execution (Fat Shim)**: `bundle run` boots an OCI bundle via `libcontainer`.
- **Self-Reporting**: `--shim-version` prints the shim version for compatibility checks.

## Interaction

- **`locald` (CLI/server)**: Invokes the shim for privileged operations.
- **IPC/FD Passing**: Privileged bind uses `SCM_RIGHTS` to pass a listening socket FD to the daemon.

## Standalone Usage

The shim is a standalone binary, but it is primarily designed to be invoked by `locald`.

```bash
# Print version
locald-shim --shim-version

# Bind a privileged port and pass the FD to locald
locald-shim bind 80 /path/to/unix.sock

# Update hosts block
locald-shim admin sync-hosts app.localhost api.localhost

# Boot an OCI bundle (preferred)
locald-shim bundle run --bundle /path/to/bundle --id my-container-id

# Boot an OCI bundle (legacy; still supported)
locald-shim bundle /path/to/bundle
```
