# Plugins

`locald` supports WASM component plugins that can inspect a service specification and produce a plan (an IR) describing operations to perform (e.g. pull an OCI image, allocate a port, declare a service).

This is currently exposed as tooling via the CLI and is implemented in the daemon crate.

## Where it lives

- **Host runtime / IR / validation**: `locald-server/src/plugins/`
- **CLI commands**: `locald-cli/src/plugin.rs` and `locald-cli/src/cli.rs`

## Plugin lifecycle

A plugin is a WASM component that implements two main operations:

- **detect**: Optional capability detection / reporting.
- **apply**: Produces a plan (or diagnostics).

On the host side, the CLI:

1. Resolves a plugin by name or path.
2. Runs `detect` and prints its result.
3. Runs `apply` to obtain a plan.
4. Validates the plan against granted host capabilities.
5. Prints a normalized debug JSON form of the plan for inspection.

## Installing plugins

The CLI supports installing a plugin from a local path or URL.

- User-local install: under the XDG data directory (e.g. `$XDG_DATA_HOME/.../plugins`).
- Project-local install: `./.local/plugins`.

## Current scope

Plugins are currently a developer-facing extension mechanism and inspection tool. The core service execution path remains implemented in the daemon’s service controllers and runtime integration.
