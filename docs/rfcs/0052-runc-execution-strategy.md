---
title: "Runc Execution Strategy"
stage: 1 # Superseded by RFC 0098
feature: Core Architecture
---

> **Note**: This RFC has been superseded by [RFC 0098: Libcontainer Execution Strategy](0098-libcontainer-execution-strategy.md). The `runc` strategy was implemented but later replaced by the embedded `libcontainer` strategy.

---

# RFC 0052: Runc Execution Strategy

## 1. Summary

This RFC proposes using the `runc` binary as the primary mechanism for executing OCI containers and Cloud Native Buildpacks (CNB) on Linux.

To ensure reliable execution across diverse environments (including those with SELinux or restricted user namespaces), `locald` will invoke `runc` via the privileged `locald-shim`.

This supersedes the "Native Execution" and "Libcontainer Crate" strategies proposed in RFC 0043 and RFC 0047.

**Policy Change**: We are explicitly abandoning "Native Execution" (running binaries directly on the host) as a supported strategy. All workloads (CNB lifecycles, services) must run in isolated OCI containers.

## 2. Motivation

### The Failure of "Native" Execution

Our initial attempt to run CNB `lifecycle` binaries directly on the host ("Native" mode) proved too brittle.

- **Path Mismatches**: Buildpacks often assume they are running in a container with specific paths (e.g., `/workspace`, `/cnb`). Mapping these to arbitrary host directories causes failures.
- **Environment Leakage**: Host environment variables and system libraries bleed into the build process.
- **Cleanup**: Processes running on the host leave behind artifacts and are harder to clean up reliably than ephemeral containers.

### The Failure of "Rootless" Execution

We attempted to run `runc` in "Rootless" mode (invoked directly by the unprivileged user). This failed on systems with strict security policies (SELinux, restricted User Namespaces), resulting in `Operation not permitted` errors when creating namespaces.

### The Solution: `runc` via `locald-shim`

By routing the execution through our existing `locald-shim` (which is setuid root), we can:

1.  **Bypass Restrictions**: Run `runc` with root privileges, allowing it to create all necessary namespaces and cgroups.
2.  **Maintain Security**: Use `runc`'s built-in user mapping to ensure the process _inside_ the container runs as the user (or maps to the user), preventing root-owned files from polluting the user's workspace.
3.  **Standardize**: Use the industry-standard OCI runtime without reinventing the wheel.

## 3. Detailed Design

### Architecture

1.  **Prepare Bundle**:
    - `locald` (User) creates a temporary directory and extracts the OCI rootfs.
    - `locald` generates a `config.json` using `oci-spec`.
2.  **Execute**:
    - `locald` calls `locald-shim runc run <id> --bundle <path>`.
    - `locald-shim` (Root) verifies the command and executes `runc`.
    - `runc` (Root -> Container User) starts the container.
3.  **Cleanup**:
    - `locald` deletes the bundle.

### Shim Updates

The `locald-shim` must be updated to accept a new `runc` subcommand.

- **Command**: `locald-shim runc [args...]`
- **Behavior**: The shim will execute `runc` with the provided arguments. It should strictly validate that it is indeed running `runc` and not an arbitrary binary.

### Environment Requirements

To ensure compatibility with Cloud Native Buildpacks (CNB) and standard OCI expectations, the runtime must inject specific environment variables into the container:

1.  **`CNB_PLATFORM_API`**: Required by the CNB Lifecycle (Launcher). Must be set to a compatible version (e.g., `0.12`).
2.  **`PATH`**: `runc` does not inherit the host `PATH` or provide a default. The runtime must inject a standard Linux path (e.g., `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`) or derive it from the OCI Image Config.

### Cross-Platform Interface

To support macOS (via Lima) and Windows (via WSL) in the future, we will define a `Runtime` trait in `locald-core` (as originally proposed in RFC 0047).

```rust
#[async_trait]
pub trait ContainerRuntime {
    /// Prepare the OCI bundle (extract rootfs, generate config)
    async fn prepare_bundle(&self, image: &str, bundle_path: &Path) -> Result<()>;

    /// Execute the container
    async fn run_container(&self, bundle_path: &Path, container_id: &str) -> Result<()>;
}
```

- **Linux Implementation**: The `LinuxRuntime` struct will implement this trait using the `locald-shim` -> `runc` strategy described in this RFC.
- **macOS Implementation**: A future `LimaRuntime` will implement this by proxying commands to a Lima VM.
- **Windows Implementation**: A future `WslRuntime` will proxy commands to the WSL2 distribution.

### Versioning Requirement

This change requires a modification to the `locald-shim` binary. Therefore, per **RFC 0045 (Shim Versioning)**, we must:

1.  Bump the version of `locald-shim` (e.g., to `0.2.0`).
2.  Update `locald` to require this new version.
3.  Prompt the user to run `sudo locald admin setup` if their shim is outdated.

## 4. Implementation Plan (Stage 2)

- [x] **Update Shim**:
  - Add `runc` subcommand to `locald-shim`.
  - Bump version in `locald-shim/Cargo.toml`.
- [x] **Update Builder**:
  - Remove "Native" execution code from `locald-builder`.
  - Implement `ShimRuntime` to invoke `locald-shim runc`.
  - Ensure `config.json` is generated with appropriate user mappings.
- [x] **Update Daemon**:
  - Integrate `locald-builder` into `locald-server`.
  - Replace "Process Service" logic with containerized execution.
- [x] **Verify**:
  - Test on the restricted environment (SELinux enabled) to confirm success.

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/architecture/runtime.md`.
- [ ] Update `docs/manual/architecture/security.md` to document the new privileged path for containers.

## 6. Drawbacks

- **Privilege Escalation Risk**: Any bug in `runc` or our shim could theoretically be exploited. However, `runc` is battle-tested, and our shim is minimal.
- **User Friction**: Users must run `sudo locald admin setup` to update the shim.

## 7. Alternatives

- **Podman**: Requires installing another tool.
- **Docker**: Too heavy.
