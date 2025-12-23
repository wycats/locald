# locald-server

**Vision**: The brain of the operation.

## Purpose

`locald-server` is the daemon implementation for `locald`: it orchestrates services, runs the IPC control plane, and provides the HTTP/HTTPS proxy layer for `*.localhost`.

This crate is designed to be invoked by the `locald` binary (from `locald-cli`). It is a **library crate** in this workspace (there is no `locald-server` standalone binary).

## Key Components (as implemented)

- **Service Manager**: Service lifecycle and runtime state (via `ProcessManager` and service controllers).
- **IPC Server**: CLI/server control plane over a Unix socket (typed requests/responses from `locald-core`).
- **HTTP/HTTPS Proxy**: Axum-based proxy for `*.localhost`, including embedded assets (dashboard/docs) and API routes.
- **Config Loader**: Loads global config and project config; supports sandboxed paths via `LOCALD_SOCKET` and XDG env vars.
- **Runtime Integration**:
  - Uses `locald-shim` for privileged operations (e.g. privileged port binding via FD passing).
  - Uses the “fat shim” container path (bundle-based execution) for container workloads.
- **Plugins**: Hosts WASM component plugins (detect/apply) that can generate and validate service plans.
- **Embedded UI Assets**: Serves embedded dashboard/docs assets when built with UI enabled; otherwise returns helpful fallback pages.

## How It’s Run

- Foreground (debug): `locald server start`
- Background (typical UX): `locald up` (spawns `locald server start` in a separate process and then talks to it over IPC)
