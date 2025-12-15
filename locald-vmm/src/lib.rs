//! # locald-vmm
//!
//! A lightweight Virtual Machine Monitor (VMM) library for `locald`.
//!
//! This crate provides a unified interface for running virtual machines
//! on Linux (via KVM) and macOS (via Virtualization.framework).
//!
//! ## Architecture
//!
//! - **Linux**: Uses `kvm-ioctls` to interact directly with `/dev/kvm`.
//! - **macOS**: Uses `objc2-virtualization` (planned) to use the native Apple Hypervisor.

#[cfg(target_os = "linux")]
/// Linux-specific KVM implementation.
pub mod linux;

/// VirtIO device emulation and transports.
pub mod virtio;

#[cfg(target_os = "macos")]
/// macOS-specific Virtualization.framework implementation.
pub mod macos;

#[cfg(target_os = "linux")]
pub use linux::VirtualMachine;

#[cfg(target_os = "macos")]
pub use macos::VirtualMachine;

#[cfg(test)]
mod tests;

/// Configuration for a Virtual Machine.
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Path to the kernel image to boot.
    pub kernel_path: std::path::PathBuf,
    /// Amount of RAM in Megabytes.
    pub memory_mb: u64,
}
