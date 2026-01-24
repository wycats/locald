# RFC 0101: Arc 2 - Runtime Isolation (The Driver Pattern)

- **Status**: Strawman
- **Date**: 2025-12-12
- **Author**: GitHub Copilot
- **Phase**: Arc 2

## Summary

This RFC defines the architectural strategy for **Arc 2: Runtime Isolation**. The core decision is to adopt the **Driver Pattern**, where `locald` acts as the Virtual Machine Monitor (VMM) directly, embedding hypervisor logic rather than shelling out to external binaries like QEMU or Firecracker.

## Motivation

To achieve our goal of a "Zero-Dependency" developer experience that is secure and consistent across platforms, we need strong isolation for services.

- **Linux**: Containers (Namespaces/Cgroups) are native, but VMs offer stronger isolation and are required for some workloads (e.g., running different kernels).
- **macOS**: Native containers do not exist. Docker Desktop and others use a hidden Linux VM. To provide a seamless experience without requiring Docker Desktop, `locald` must manage its own lightweight Linux VM.

### The Problem with External VMMs

Using `qemu` or `firecracker` as external binaries introduces:

1.  **Dependency Hell**: Users must install these tools, or we must bundle large binaries.
2.  **Opaque Lifecycle**: We lose visibility into the VM's internal state (boot process, panic logs) unless we parse complex serial output.
3.  **API Friction**: Controlling them requires speaking their specific CLI or API dialects (e.g., Firecracker's HTTP API).

## The Solution: The Driver Pattern

We invert the relationship. Instead of `locald` _managing_ a VMM process, `locald` **is** the VMM process (or spawns a child that is).

We use Rust libraries to interact directly with the OS hypervisor APIs:

- **Linux**: `kvm-ioctls` (interacts with `/dev/kvm`).
- **macOS**: `objc2-virtualization` (interacts with `Virtualization.framework`).

### Benefits

1.  **Single Binary**: The VMM logic is compiled into `locald`. No extra installs.
2.  **Deep Integration**: We can map memory, intercept IO, and handle exits directly in Rust code.
3.  **Performance**: Direct ioctl/syscall access minimizes overhead.
4.  **Customization**: We build exactly the "MicroVM" we need, nothing more.

## Architecture

### `locald-vmm` Crate

A new crate `locald-vmm` encapsulates the platform differences.

```rust
pub struct VirtualMachine {
    // Platform-specific fields
    #[cfg(target_os = "linux")]
    kvm: kvm_ioctls::Kvm,

    #[cfg(target_os = "macos")]
    vz: objc2_virtualization::VZVirtualMachine,
}

impl VirtualMachine {
    pub fn new(config: VmConfig) -> Result<Self>;
    pub fn run(&mut self) -> Result<()>;
}
```

### Linux Implementation (KVM)

- **Mode**: "Crate Mode". We leverage the Rust VMM ecosystem.
- **Bootloader**: Use `linux-loader` to handle the Linux Boot Protocol (loading `vmlinux`/`bzImage`, setting up `boot_params`).
- **Memory**: Use `vm-memory` traits to manage guest memory regions safely.
- **Kernel**: We download a known-good microVM kernel (e.g., from Firecracker) for reproducible builds.
- **Success Criteria**: The VM must boot and reach a minimal userspace (init process).

### macOS Implementation (Virtualization.framework)

- **Mode**: "Easy Mode". Apple's framework provides high-level abstractions (`VZLinuxBootLoader`, `VZVirtioSocketDevice`).
- **Kernel**: Standard Linux kernel.
- **Rosetta**: We can leverage Rosetta for Linux to run x86_64 binaries on Apple Silicon with near-native speed.

## Roadmap

1.  **Phase 24 (Done)**: Prototype KVM booting raw assembly.
2.  **Phase 25**: Boot a real Linux Kernel on KVM (Serial output).
3.  **Phase 26**: Implement macOS `Virtualization.framework` support.
4.  **Phase 27**: VirtIO Networking (TAP/TUN on Linux, VZNAT on macOS).
5.  **Phase 28**: File Sharing (VirtioFS).

## Alternatives Considered

- **Firecracker**: Great, but Linux-only. Would require a different strategy for macOS.
- **QEMU**: Too heavy, hard to bundle.
- **Cloud Hypervisor**: Good reference, but we want tighter integration.

## Conclusion

The Driver Pattern aligns perfectly with `locald`'s philosophy of being a self-contained, powerful tool. It moves complexity from "User Configuration" to "Internal Implementation", which is the right trade-off for a developer tool.
