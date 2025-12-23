# locald-vmm

**Vision**: A small, focused virtual machine monitor library for `locald`.

## Purpose

`locald-vmm` provides the low-level building blocks needed to run lightweight virtual machines in support of `locald` experimentation.

Today:

- **Linux**: KVM-backed implementation (currently x86/x86_64-centric).
- **Non-x86 Linux**: Compiles, but returns an “unsupported” error at runtime.
- **macOS**: A Virtualization.framework implementation is planned, but not yet shipped.

## Key Components

- **KVM boot/runtime**: Direct `/dev/kvm` interaction via `kvm-ioctls`.
- **VirtIO**: Device emulation and transports under `src/virtio/`.
- **Cross-platform facade**: A unified `VirtualMachine` export per OS.

## Interaction

This crate is currently used as a building block for future VMM-based workflows. It is intentionally isolated from the rest of the daemon’s core orchestration so it can evolve independently.

## Testing

Some integration-style tests are intentionally ignored by default because they may download large VM assets.

- Run ignored tests explicitly: `cargo test -p locald-vmm -- --ignored`
