---
title: "Embedded Shim Distribution"
stage: 3 # Recommended
feature: Architecture
---

# RFC 0078: Embedded Shim Distribution

## 1. Summary

This RFC proposes embedding the `locald-shim` binary directly into the `locald` CLI executable at compile time. This ensures that the correct, compatible version of the shim is always available for installation, eliminating the "stale shim" problem caused by `cargo install`'s single-binary limitation.

## 2. Motivation

### The Problem

`locald` relies on a privileged helper binary, `locald-shim`, for operations like binding to port 80 or creating container namespaces.

Previously, `locald` and `locald-shim` were distributed as separate binaries. When a user ran `cargo install --path .`, Cargo would only install the main `locald` binary. The `locald-shim` binary would either be missing or, worse, remain as an outdated version from a previous installation.

This led to:

1.  **Version Mismatches**: The new `locald` would try to use an old `locald-shim`, leading to protocol errors or missing features.
2.  **Deployment Friction**: Users had to manually install the shim or rely on complex scripts to find the build artifacts.
3.  **Debug Noise**: Stale shims with debug logging enabled would pollute the output of the new daemon.

### The Solution

By embedding the compiled `locald-shim` binary into `locald` itself, we guarantee that every `locald` executable carries its own perfectly matching shim.

## 3. Detailed Design

### 3.1 Build Process

1.  **Build Shim**: The `locald-cli/build.rs` script compiles `locald-shim` in release mode.
2.  **Embed**: The path to the resulting binary is passed to the compiler via the `LOCALD_SHIM_PATH` environment variable.
3.  **Include**: The `locald-cli` source code uses `include_bytes!(env!("LOCALD_SHIM_PATH"))` to embed the binary data into the executable's `.rodata` section.

### 3.2 Installation Workflow (`admin setup`)

The `locald admin setup` command is updated to extract this embedded binary:

1.  **Check Environment**:
    - If `LOCALD_SHIM_PATH` is set (e.g., during development), it uses that external file.
    - Otherwise, it uses the embedded bytes.
2.  **Write File**: It writes the embedded bytes to the target location (e.g., `~/.cargo/bin/locald-shim`), overwriting any existing file.
3.  **Set Permissions**: It performs the necessary `chown root:root` and `chmod 4755` operations.

### 3.3 Benefits

- **Atomic Updates**: Updating `locald` (via `cargo install`) implicitly updates the "payload" for the shim.
- **Simplified UX**: The user only needs to run `sudo locald admin setup` to apply the update. No need to find or build the shim separately.
- **Guaranteed Compatibility**: The embedded shim is built from the same source tree commit as the CLI.
- **Cross-Platform**: The same embedding mechanism works on both Linux and macOS.

### 3.4 Platform Support

**Implementation Decision**: The embedded shim works on both Linux and macOS.

| Platform | Shim Binary     | Installation Path                       | Permissions    |
| -------- | --------------- | --------------------------------------- | -------------- |
| Linux    | ELF x86_64      | `~/.local/share/locald/bin/locald-shim` | `root:root 4755` |
| macOS    | Mach-O arm64    | `~/.local/share/locald/bin/locald-shim` | `root:wheel 4755` |

**Build Process**:
- On Linux CI: Build produces Linux ELF shim, embedded in Linux CLI
- On macOS CI: Build produces macOS Mach-O shim, embedded in macOS CLI
- Cross-compilation is not required; each platform builds its own shim

**Container Features**: Linux-only container commands (libcontainer, cgroups) are compile-time gated using `#[cfg(target_os = "linux")]`. The macOS shim binary simply doesn't include this code.

### 3.5 Upgrade Lifecycle & Versioning

This design works in tandem with **RFC 0045 (Shim Versioning)** to minimize the need for `sudo`.

1.  **Routine Updates**: When the user updates `locald` (e.g., `cargo install`), they get the new CLI and the new embedded shim payload.
2.  **Version Check**: On startup, `locald` checks the version of the _installed_ shim (in `~/.cargo/bin`) against its expected version.
3.  **Conditional Prompt**:
    - **Match**: If the installed shim version matches the expected version, `locald` proceeds. The user **does not** need to run `sudo locald admin setup`, even if the `locald` binary itself changed.
    - **Mismatch**: If the versions differ (indicating a capability change or security fix), `locald` halts and prompts the user to run `sudo locald admin setup`.
4.  **Execution**: When the user runs the setup command, the new embedded payload is extracted, ensuring the shim is brought up to date.

This ensures that `sudo` is only required when the shim's capabilities actually change, not for every minor CLI update.

## 4. Alternatives Considered

- **`cargo install` multiple binaries**: Cargo supports installing multiple binaries, but it doesn't support post-install hooks to set permissions or move files.
- **Package Managers (DEB/RPM)**: While good for production, they don't help the `cargo install` developer workflow.
- **Download on Demand**: The CLI could download the shim from GitHub Releases. This introduces a network dependency and versioning complexity.

## 5. Implementation Status

This design is implemented in `locald-cli`.

- `build.rs`: Builds the shim.
- `src/handlers.rs`: Implements the extraction logic in `AdminCommands::Setup`.
