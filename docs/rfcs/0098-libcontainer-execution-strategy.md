---
title: "Libcontainer Execution Strategy"
stage: 3 # Recommended
feature: Core Architecture
---

# RFC 0098: Libcontainer Execution Strategy

- **Date**: 2025-12-12
- **Supersedes**: [RFC 0052: Runc Execution Strategy](0052-runc-execution-strategy.md)

## Summary

We will replace the external `runc` binary dependency with an embedded `libcontainer` implementation inside `locald-shim`. This transforms `locald-shim` from a "Thin Shim" (privilege escalator + exec) into a "Fat Shim" (privilege escalator + container runtime).

## Motivation

Our previous decision to use `runc` (RFC 0052) was based on two primary assumptions:

1.  **Reliability**: `libcontainer` failed in rootless environments, whereas `runc` was assumed to be more reliable.
2.  **Maintenance**: Using `runc` avoided "reinventing the wheel."

Upon re-evaluation, these assumptions have proven to be either false dichotomies or trade-offs that favor accidental complexity over domain complexity.

### 1. The "Reliability" Red Herring

The failure of `libcontainer` in early tests was due to the **execution context** (unprivileged user), not the library itself. We solved the context problem by introducing `locald-shim` as a setuid root binary.

Once we are running as root (inside the shim), `libcontainer` has the exact same capabilities as `runc`. The reliability gap does not exist in the privileged context.

### 2. The "CLI Wrapper" Tax

By choosing `runc`, we traded the complexity of _implementing_ a runtime for the complexity of _orchestrating_ a CLI tool. This incurred significant "Accidental Complexity":

- **Distribution**: We must ensure `runc` is installed, compatible, and on the PATH.
- **Fragility**: We construct CLI arguments strings and parse unstructured stderr logs.
- **Signal Handling**: We manage a fragile double-fork chain (`locald` -> `shim` -> `runc` -> `container`).
- **Opacity**: We cannot easily introspect the state of the container creation process; we only get what `runc` decides to print.

### 3. Distribution & UX

Embedding `libcontainer` allows `locald` to be truly self-contained. The `locald-shim` binary becomes the only requirement for container execution. This eliminates an entire class of "Environment Setup" errors for users.

## Design

### The "Fat Shim" Architecture

The `locald-shim` binary will link against the `libcontainer` crate (from the Youki project or similar OCI-compliant Rust implementation).

**Current Flow (RFC 0052):**

1.  `locald` spawns `locald-shim` (setuid).
2.  `locald-shim` drops privileges to target user (partially) or retains root.
3.  `locald-shim` `exec`s `runc create ...`.
4.  `runc` parses config, sets up namespaces, and spawns the container process.

**New Flow:**

1.  `locald` spawns `locald-shim` (setuid).
2.  `locald-shim` uses `libcontainer` APIs directly to:
    - Load the `config.json`.
    - Set up cgroups and namespaces.
    - Pivot root.
    - Exec the user process.

### Dependencies

We will use the `libcontainer` crate, which is the core logic behind [Youki](https://github.com/containers/youki), a Rust-based OCI runtime.

### Platform Gating

**Implementation Decision**: Container-related code is gated at compile time using `#[cfg(target_os = "linux")]`.

```rust
// In locald-shim/src/commands/container.rs
#[cfg(target_os = "linux")]
pub mod container {
    use libcontainer::container::Container;
    // ... libcontainer integration
}

#[cfg(not(target_os = "linux"))]
pub mod container {
    pub fn run_container(_: &Path) -> Result<(), Error> {
        Err(Error::UnsupportedPlatform(
            "Container execution requires Linux. Use Lima for macOS container support."
        ))
    }
}
```

**Rationale**:
- **Single Binary**: No separate "macOS edition" of `locald-shim`
- **Clear Errors**: Compile-time errors if Linux-only code is accidentally used
- **Future Path**: Lima integration (RFC 0047) will provide macOS container support alongside the native Linux path

### Dependency Management

To maintain the "Self-Contained" axiom, `locald-shim` must not depend on dynamic system libraries that might be missing on the user's host (e.g., `libseccomp.so`).

- **Requirement**: Dependencies must be statically linked or disabled via feature flags.
- **Initial Phase**: We will disable `seccomp` and `systemd` features in `libcontainer` to ensure portability and avoid linker errors.
  ```toml
  [dependencies]
  libcontainer = { version = "...", default-features = false, features = ["v2"] }
  ```

## Architecture: Caller-Generates / Shim-Executes

To maintain separation of concerns, the Shim must remain a "dumb" OCI Runtime.

1.  **The Caller (Daemon/Test Harness)**:
    - Responsible for **generating** the OCI Bundle.
    - Creates the `config.json` (using `oci-spec`) describing the workload (args, env, mounts).
    - Creates the rootfs directory.
2.  **The Shim (Runtime)**:
    - Responsible for **executing** the provided Bundle.
    - Accepts a path to the Bundle directory.
    - Reads `config.json` and invokes `libcontainer`.
    - **Constraint**: The Shim must NOT generate `config.json` or assume specific workload details.

## Scrutiny & Risks

This decision is not without cost. We are trading one set of problems for another.

### 1. Security Surface Area

- **Risk**: `locald-shim` runs as root. Previously, it was a tiny (~200 LOC) program that did one thing: exec `runc`. Now, it will contain a complex container runtime library.
- **Mitigation**: The `libcontainer` code is compiled into the shim. Any vulnerability in the library becomes a vulnerability in our setuid binary. We must be vigilant about updating dependencies and auditing the shim.

### 2. Maintenance Burden

- **Risk**: We are now "building a wheel." If `libcontainer` has a bug in how it handles cgroup v2 on Fedora, _we_ have a bug. With `runc`, we could blame the distro package.
- **Rebuttal**: We were already debugging `runc` interactions. Owning the stack means we can fix bugs directly rather than working around them with CLI flags.

### 3. Binary Size

- **Risk**: `locald-shim` will grow from ~2MB to potentially 10MB+.
- **Impact**: Negligible on modern systems.

### 4. Maturity

- **Risk**: `runc` is the industry standard. `libcontainer` (Rust) is newer.
- **Mitigation**: Youki is OCI compliant and passes the OCI integration tests. The ecosystem is maturing rapidly.

## Conclusion

The "CLI Wrapper Tax" of managing `runc` externally is higher than the "Maintenance Tax" of embedding `libcontainer`. By moving to an embedded runtime, we gain control, observability, and distribution simplicity, at the cost of a larger binary and increased responsibility for low-level container details.
