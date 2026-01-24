---
title: "Cloud Native Buildpacks (CNB) Integration"
stage: 3 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Build System
---

# RFC 0043: Cloud Native Buildpacks (CNB) Integration

## 1. Summary

This RFC proposes integrating Cloud Native Buildpacks (CNB) into `locald` to provide a zero-config build system and predictable development environments.

Unlike standard CNB implementations that rely heavily on Docker, `locald` will implement a **Hybrid Strategy**:

1.  **Linux**: Run CNB `lifecycle` binaries directly on the host ("Native Platform") for performance and simplicity.
2.  **macOS**: Orchestrate a lightweight Linux VM (via `lima`) to run the lifecycle.
3.  **Windows**: Leverage WSL2 to run the lifecycle natively.

## 2. Motivation

Currently, `locald` supports running local processes and pre-built Docker images. However, it lacks:

1.  **Source-to-Image**: A way to build images without writing `Dockerfiles`.
2.  **Dev Environments**: A way to provision a local shell with the correct tools (Node, Python, etc.) without polluting the global system or requiring a heavy container setup.
3.  **Rust Ecosystem Support**: A specific, high-priority goal is to support **Rust** development workflows. Rust builds can be complex (dependencies, compilation time, system libraries). CNB provides a standardized way to handle caching and toolchain management for Rust without requiring every user to manually manage `rustup` or system dependencies.

By integrating CNB, we can:

- Provide `locald build` to create OCI images.
- Provide `locald shell` (or implicit environment setup) to run apps using the buildpack-generated environment (via `launcher`).
- **Enable first-class Rust support**: Allow users to build and run Rust projects with zero configuration, leveraging community buildpacks (like Paketo) or custom ones.

## 3. Detailed Design

### 3.1 The "Containerized" Approach (Default)

To guarantee reproducibility and compatibility, `locald` must run the buildpacks in the environment they expect (the "Stack Image", e.g., Ubuntu 22.04).

**On Linux**, we will achieve this _without_ Docker by:

1.  **Pulling**: Downloading the Stack Image (Run Image + Build Image) using `oci-distribution`.
2.  **Unpacking**: Extracting the rootfs to a cache directory.
3.  **Isolating**: Using **`libcontainer`** (Rust) to execute the lifecycle binaries _inside_ this rootfs using User Namespaces (Rootless).

This ensures that even if the host is Arch Linux or Fedora, the buildpacks see a standard Ubuntu environment.

### 3.2 The "Native" Optimization (Opt-In)

We will retain the "Host Process" mode as an opt-in strategy for:

- **Performance**: Avoiding the overhead of namespaces/rootfs switching.
- **Tooling**: Users who want to use host-installed tools (e.g., a specific JDK) combined with buildpack logic.

### 3.3 Cross-Platform Strategy

Since CNB buildpacks (e.g., Heroku, Paketo) produce Linux binaries, they cannot run natively on macOS.

- **Linux**: Containerized execution (via `libcontainer`).
- **Windows (WSL2)**: Containerized execution (via `libcontainer` inside WSL2).
- **macOS**: `locald` will manage an embedded `lima` instance. `locald` commands on macOS will transparently proxy the build request to the `lima` VM, where the Linux logic will run.

### 3.4 Isolation & Safety

By defaulting to Containerized execution, we solve two problems at once:

1.  **Compatibility**: The stack is guaranteed.
2.  **Safety**: The build process is isolated from the user's `$HOME` (except for explicitly mounted source/cache directories).

### 3.5 Implementation Philosophy: "Orchestrate, Don't Reimplement"

A core principle of this integration is to **leverage existing tools and libraries** rather than reimplementing complex protocols from scratch.

- **Registry Interaction**: We will use the `oci-distribution` crate to handle authentication, manifest parsing, and layer pulling.
- **Layer Extraction**: We will use standard `tar` and `flate2` crates to extract buildpack layers.
- **Lifecycle Execution**: We use the official CNB `lifecycle` binaries extracted from the builder image. We do not reimplement the buildpack protocol logic itself.

### 3.6 Risks & Mitigations

#### Host Dependency Mismatch (Linux Native)

- **Risk**: Buildpacks (like `heroku/python`) expect a standard stack (e.g., Ubuntu 22.04) with specific system libraries (glibc, openssl, etc.). Running them directly on a random Linux host (e.g., Arch, Fedora, or a minimal distro) might fail if shared libraries are missing or incompatible.
- **Mitigation**:
  1.  **Documentation**: Clearly state that "Native Mode" requires a reasonably standard glibc environment.
  2.  **Fallback**: If native execution fails, the user can switch to the "Containerized" runtime (RFC 0047), which provides the exact stack environment.

## 4. Implementation Plan

### Phase 1: Research & Prototype (Done)

- [x] Research CNB Platform API.
- [x] Prototype OCI extraction in Rust (`oci-distribution`).
- [x] Verify `lifecycle/creator` execution on Linux host.
- [x] Verify `launcher` for environment setup.

### Phase 2: Core Integration (Current)

- [x] **`locald-builder` Crate**:
  - Implement `BuilderImage` struct to manage the OCI image (pull, cache, extract).
  - Implement `Lifecycle` struct to wrap the execution of lifecycle binaries.
- [x] **`locald build` Command**:
  - CLI command to trigger the build.
  - Configuration parsing (builder image selection).
- [ ] **Rust Support Verification**:
  - Verify `locald build` with a Rust project.
  - Identify and document working Rust builders (e.g., `heroku/builder:22` or Paketo).
- [ ] **Service Integration**:
  - Allow services to define `build` configuration in `locald.toml`.

### Phase 3: Cross-Platform Shim

- [ ] Implement `lima` management on macOS.
- [ ] Abstract the "Platform" trait to support Local vs. VM execution.

### Phase 4: Isolation

- [ ] Integrate `libcontainer` for safe execution on Linux.

## 5. Future Considerations

- **DevContainers**: Generate `devcontainer.json` that points to the CNB layers.
- **Remote Builds**: Offload builds to a remote cluster.
