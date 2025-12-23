# locald-cli

**Vision**: The user's command center.

## Purpose

`locald-cli` builds the `locald` binary.

It contains the command parser, the IPC client, and the “daemon bootstrap” logic that starts the server when needed.

In this workspace, the server implementation lives in the `locald-server` crate and is invoked via `locald server start`.

## Key Components (as implemented)

- **Command Parser**: Clap-based command tree for `locald`.
- **IPC Client**: Talks to the daemon over a Unix socket using types in `locald-core`.
- **Daemon Bootstrap**:
  - `locald up` (and many other commands) auto-start the daemon if it’s not already running.
  - The daemon is started by spawning `locald server start` in a separate process.
- **Admin UX**: Commands like `locald admin setup` for installing/configuring the privileged shim.
- **Plugin Tooling**: `locald plugin install|inspect|validate` for working with WASM component plugins.

## Usage

```bash
locald up
locald status
locald logs --follow
locald server shutdown

# Manage plugins
locald plugin install ./my-plugin.component.wasm --project
locald plugin inspect my-plugin --kind redis
locald plugin validate my-plugin --kind redis
```

Most commands will start the daemon automatically if needed.
