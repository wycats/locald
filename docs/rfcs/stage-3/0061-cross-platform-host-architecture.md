---
title: "Cross-Platform Host Architecture"
stage: 1 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Core Architecture
---

# RFC 0061: Cross-Platform Host Architecture

## 1. Summary

This RFC defines the architectural strategy for supporting `locald` on Linux, macOS, and Windows. It explicitly tracks the "Host Capabilities" required by `locald` (Process Execution, Filesystem, Networking) and maps how each capability is achieved on each platform, specifically focusing on the use of **Lima** for macOS and **WSL2** for Windows.

## 2. Motivation

`locald` is designed as a "Native" tool, but it relies heavily on Linux-specific primitives (Namespaces, Cgroups, OverlayFS, OCI Runtimes) to provide predictable, containerized environments.

To support macOS and Windows without rewriting the entire world or forcing users to install Docker Desktop, we need a unified strategy for bridging the "Host" (where the user types commands) and the "Runtime" (where the code executes).

This RFC serves as the central registry for these portability constraints, ensuring that architectural decisions (like "Zero-Copy Build Context" in RFC 0060) are feasible across all supported platforms.

## 3. Detailed Design

### 3.1. The "Host-Runtime" Model

We define two distinct contexts:

1.  **The User Host**: The OS where the user runs the CLI (`locald up`).
2.  **The Execution Runtime**: The Linux environment where processes actually run.

| Platform | User Host | Execution Runtime | Bridge Technology |
| :--- | :--- | :--- | :--- |
| **Linux** | Linux | Linux (Same) | Direct / Namespaces |
| **macOS** | macOS (Darwin) | Linux VM | **Lima** (via `virtiofs` + SSH/Socket) |
| **Windows** | Windows (NT) | Linux VM | **WSL2** (via Plan 9 / DrvFs) |

### 3.2. Host Capabilities Matrix

This matrix tracks the specific primitives `locald` requires and how they are implemented per platform.

#### A. Process Execution (The "Shim")

*Requirement*: Execute a binary (CNB Lifecycle, Service) in a Linux environment.

-   **Linux**: `fork` / `exec` directly on the host.
-   **macOS**: `locald` spawns a `lima` command (or connects to the Lima socket) to execute the binary inside the VM.
-   **Windows**: `locald` (running in Windows) spawns a `wsl.exe` command, or `locald` runs *inside* WSL2 directly (Recommended).

#### B. Filesystem: Source Code Access

*Requirement*: The Runtime must access the user's source code at high speed.

-   **Linux**: Direct filesystem access.
-   **macOS**: **VirtioFS**. Lima mounts the host user's home directory into the VM using VirtioFS. This allows the Linux VM to read/write host files with near-native performance.
-   **Windows**: **WSL2 Mounts**. WSL2 automatically mounts Windows drives (`C:\`) at `/mnt/c`.

#### C. Filesystem: Build Context (OverlayFS)

*Requirement*: Create a "Zero-Copy" build context (RFC 0060) using `fuse-overlayfs`.

-   **Linux**: `fuse-overlayfs` with `lowerdir=/path/to/source`.
-   **macOS**: `fuse-overlayfs` inside the VM.
    -   `lowerdir`: The VirtioFS mount of the source (e.g., `/Users/me/project`).
    -   `upperdir`: Ephemeral VM storage (e.g., `/tmp/locald/build`).
-   **Windows**: `fuse-overlayfs` inside WSL2.
    -   `lowerdir`: The WSL mount (e.g., `/mnt/c/Users/me/project`).
    -   `upperdir`: Ephemeral WSL storage.

#### D. Networking: Port Forwarding

*Requirement*: Services running in the Runtime must be accessible via `localhost` on the User Host.

-   **Linux**: Direct binding to `127.0.0.1`.
-   **macOS**: **Lima Network Forwarding**. Lima automatically forwards ports bound to `0.0.0.0` or `127.0.0.1` inside the VM to the macOS host.
-   **Windows**: **WSL2 Localhost Forwarding**. Windows automatically forwards ports bound in WSL2 to the Windows host (with some caveats regarding `::1` vs `127.0.0.1`).

#### E. Networking: Service-to-Service

*Requirement*: Services must talk to each other via DNS/IP.

-   **All Platforms**: This happens entirely *inside* the Execution Runtime (Linux). We use a shared bridge network or CNI plugin. Since the Runtime is always Linux, this code is 100% portable.

### 3.3. Platform-Specific Implementation Plans

#### macOS: The "Embedded Lima" Strategy

**Why not native Rust yet?**
While crates like `virt-fwk` exist to bind to Apple's `Virtualization.framework`, they are currently immature compared to the Go ecosystem (Lima/Colima).
- **VirtioFS on macOS**: Apple's framework provides the *server* implementation of VirtioFS. We just need to configure it via the `VZVirtioFileSystemDeviceConfiguration` API.
- **Decision**: We will use **Lima** (managed by `locald`) to handle the VM lifecycle and VirtioFS configuration for now.
- **Future**: We can replace the `lima` binary with a native Rust implementation using `objc2` bindings to `Virtualization.framework` once we stabilize the core logic.

#### Windows: The "WSL First" Strategy

**Architecture Confirmation**:
- **Mounting**: WSL2 automatically handles mounting Windows drives (e.g., `C:\` -> `/mnt/c`) using the DrvFs/Plan9 protocol. `locald` does *not* need to manage this.
- **Overlay**: `locald` (running inside WSL2) simply treats `/mnt/c/...` as the **LowerDir** for its OverlayFS.
- **Result**: We get "Zero-Copy" access to Windows files for free, and we only need to manage the Linux-side overlay (using `fuse-backend-rs`).

## 4. Implementation Plan (Stage 2)

-   [ ] **Abstract the "Host" Trait**: Refactor `locald-server` to put OS-specific operations (Command execution, FS paths) behind a Rust trait (`HostProvider`).
-   [ ] **Implement `LinuxHost`**: The current implementation.
-   [ ] **Implement `LimaHost`**:
    -   [ ] Logic to download/check Lima.
    -   [ ] Logic to generate `locald-vm.yaml`.
    -   [ ] Logic to wrap commands in `limactl shell locald-vm ...`.
-   [ ] **Implement `WindowsHost`**:
    -   [ ] Logic to detect WSL2.
    -   [ ] Logic to bridge commands.

## 5. Context Updates (Stage 3)

-   [ ] Create `docs/manual/architecture/cross-platform.md`.
-   [ ] Update `docs/manual/installation.md` with platform-specific notes.

## 6. Drawbacks

-   **Complexity**: Managing a VM lifecycle (Lima) adds significant complexity to the `locald` daemon (start, stop, recover, upgrade).
-   **Performance**: Even with VirtioFS, cross-OS file access is slower than native.

## 7. Alternatives

-   **Docker Desktop**: We could just require Docker.
    -   *Rejection*: We want `locald` to be a standalone, "batteries-included" tool that doesn't depend on a paid/heavy external product.
-   **Wasm**: Compile everything to Wasm.
    -   *Rejection*: The ecosystem (Node, Python, Postgres) isn't ready for full Wasm-only execution yet.

## 8. Future Possibilities

-   **Native Hypervisors**: Use `virt-fwk` (macOS) and `Hyper-V` APIs (Windows) directly from Rust to boot a micro-kernel, removing the Lima/WSL dependency for a truly single-binary experience.
