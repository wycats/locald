# Implementation Plan - Phase 97: Libcontainer Shim (Fat Shim)

**Goal**: Replace the external `runc` dependency with an embedded `libcontainer` runtime in `locald-shim`. This ensures reliable, self-contained execution and eliminates the "Rootless Fragility" issues encountered in previous phases.

**RFC**: [RFC 0098: Libcontainer Execution Strategy](../../rfcs/0098-libcontainer-execution-strategy.md)

## 1. Architecture

- **Caller-Generates**: The Daemon (or Test Harness) is responsible for generating the OCI Bundle (directory containing `config.json` and `rootfs`).
- **Shim-Executes**: The Shim (`locald-shim`) is a "dumb" runtime that accepts a path to the Bundle and executes it using `libcontainer`.

## 2. Implementation Steps

### Step 1: Shim Refactor (Runtime)

- Modify `locald-shim` CLI to accept a `bundle` subcommand (or argument).
- Use `libcontainer` crate to load the spec and start the container.
- **Constraint**: Must be statically linked (or feature-disabled) to avoid `libseccomp` dependency issues.

### Step 2: Daemon Update (Caller)

- Update `locald-server` to use `oci-spec` crate to generate `config.json`.
- Create a temporary directory for each container execution to serve as the Bundle.
- Invoke `locald-shim` pointing to this directory.

### Step 3: Verification

- Run `locald run alpine echo hello` to verify end-to-end execution.
- Verify that no external `runc` binary is required.

## 3. Risks

- **Security**: The shim runs as setuid root. We must ensure `libcontainer` is invoked securely.
- **Complexity**: Debugging `libcontainer` internals is harder than debugging `runc` CLI.
