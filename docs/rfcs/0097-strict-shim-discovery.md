# RFC 0097: Strict Shim Discovery

- **Status**: Accepted
- **Date**: 2025-12-11
- **Author**: Agent

## Context

The `locald-shim` is a critical security component. Historically, we allowed finding the shim via the `LOCALD_SHIM_PATH` environment variable to facilitate development (e.g., `cargo run` builds the shim in a different location than the CLI).

However, this flexibility has become a liability:

1.  **Configuration Drift**: Developers or users might set this variable and forget about it, leading to `locald` using an outdated or incompatible shim.
2.  **False Positives**: The CLI's integrity checks would fail or behave unpredictably because the environment variable was overriding the expected logic.
3.  **Security Risk**: Allowing an environment variable to dictate the location of a privileged binary is a potential attack vector (though the shim itself must still be setuid root, so the risk is mitigated, but not eliminated).
4.  **Debugging Complexity**: "It works on my machine" issues were frequent because of hidden environment variables.

## Decision

We are **removing** support for `LOCALD_SHIM_PATH` entirely.

### The New Discovery Logic

`locald` will only look for the shim in the following locations:

1.  **Sibling**: The `locald-shim` binary must exist in the same directory as the `locald` executable.
2.  **Parent (Test Only)**: To support `cargo test` (where the test binary is in `target/debug/deps/` and the shim is in `target/debug/`), we allow looking in the parent directory.

### Implications

1.  **Development**: When running `cargo run`, the shim will not be found automatically unless it is copied to `target/debug/locald-shim`.
    - **Mitigation**: The `admin setup` command (or a helper script) must be used to copy the shim to the correct location and set permissions.
2.  **Installation**: The `admin setup` command will extract the embedded shim to the sibling location.
3.  **Daemon Safety**: Background/daemon contexts must never trigger interactive privilege escalation.
    - The daemon will only use an **already-configured** (setuid root) `locald-shim`.
    - If the shim is missing or not privileged, the daemon will skip privileged “quality of life” actions (e.g. auto hosts sync) and instruct the user to run `sudo locald admin setup`.
4.  **Error Handling**: If the shim is not found or has incorrect permissions, `locald` will fail hard when a privileged operation is required.
    - **Interactive Mode**: It will attempt to auto-fix the issue by running `sudo locald admin setup`.
    - **Non-Interactive**: It will instruct the user to run `sudo locald admin setup` manually. It will not try to guess or fallback to other locations.

### Note: Hosts Sync

The daemon must not shell out to `locald admin sync-hosts` to update `/etc/hosts`.

- It should invoke `locald-shim admin sync-hosts ...` directly.
- If the shim is not available, it should log a single actionable message (run `sudo locald admin setup`) and continue.

## Benefits

- **Predictability**: There is only one place the shim can be.
- **Security**: Reduced attack surface.
- **Simplicity**: The code for finding the shim is trivial and easy to audit.
