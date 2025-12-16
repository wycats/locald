---
title: "Cross-Platform Container Runtime Strategy"
stage: 1 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Core Architecture
---

# RFC 0047: Cross-Platform Container Runtime Strategy

## 1. Summary

This RFC defines the architectural strategy for running OCI containers and CNB lifecycles across Linux, macOS, and Windows _without_ requiring Docker Desktop.

The strategy is **Hybrid**:

1.  **Linux**: Native execution. Initially process-based, evolving to `libcontainer` (Rust) for isolation.
2.  **macOS**: Virtualized execution via an embedded **Lima** instance managed by `locald`.
3.  **Windows**: Native execution inside **WSL2**.

## 2. Motivation

`locald` aims to be a self-contained developer platform. Relying on the user to install and manage Docker Desktop (or even a manual Podman setup) creates friction and licensing headaches.

To support features like **CNB Builds** (RFC 0043) and future **Service Isolation**, `locald` needs a reliable way to run Linux binaries and OCI containers on all supported OSs.

## 3. Detailed Design

### 3.1 The "Runtime" Abstraction

We will introduce a `Runtime` trait in `locald-core` that abstracts the underlying execution environment.

```rust
trait Runtime {
    async fn run_command(&self, cmd: Command) -> Result<Output>;
    async fn run_container(&self, spec: ContainerSpec) -> Result<ContainerHandle>;
    // ...
}
```

### 3.2 Linux: The "Native" Path

On Linux, `locald` is already running on the target kernel.

- **Phase 1 (Process Isolation)**: For CNB, we run the `lifecycle` binaries directly as subprocesses. This is fast but lacks isolation.
- **Phase 2 (Libcontainer)**: We will integrate the `libcontainer` crate (from the Youki project). This allows `locald` to spawn true OCI containers (Namespaces, Cgroups) directly from Rust code.
  - _Benefit_: Zero external dependencies (no `runc` binary needed).
  - _Benefit_: "Safe Mode" for untrusted workloads.

### 3.3 macOS: The "Embedded Lima" Path

macOS cannot run Linux binaries (ELF) natively. We need a Linux kernel.

- **Tool Selection**: **Lima** (Linux Machines). It is open-source, lightweight, and supports macOS native virtualization (`vz` framework) and file sharing (`virtiofs`).
- **Management**:
  - `locald` will _not_ ask the user to `brew install lima`.
  - `locald` will download a pinned version of the `lima` binary to `~/.local/share/locald/tools/`.
  - `locald` will initialize a dedicated VM (e.g., `locald-vm`).
- **Execution**:
  - When a command requires Linux (e.g., `locald build`), `locald` proxies the command into the VM via `lima shell`.
  - We may eventually run a `locald-agent` inside the VM to handle complex orchestration.

### 3.4 Windows: The WSL2 Path

Windows Subsystem for Linux 2 (WSL2) provides a real Linux kernel.

- **Detection**: `locald` detects if it is running inside WSL2.
- **Execution**: If inside WSL2, it behaves exactly like the **Linux** path.
- **Host Interop**: If `locald.exe` is run from Windows (PowerShell), it should transparently proxy commands to the WSL2 instance (similar to how the `docker` CLI works).

## 4. Implementation Plan

### Phase 1: Linux Native (Current)

- [ ] Implement the `Runtime` trait for local process execution.
- [ ] Validate CNB lifecycle execution on host.

### Phase 2: macOS Shim

- [ ] Implement `LimaRuntime` struct.
- [ ] Add logic to download/verify `lima` binaries.
- [ ] Add logic to start/stop the `locald-vm`.

### Phase 3: Libcontainer (Linux)

- [ ] Prototype `libcontainer` integration in a standalone example.
- [ ] Integrate into `locald` as the "Secure" runtime option.

## 5. Alternatives Considered

- **Bundling QEMU**: Too heavy and complex to manage.
- **WASM**: Not mature enough for full buildpack/service compatibility yet.
- **Docker Requirement**: Rejects the "Zero Dependency" goal.
