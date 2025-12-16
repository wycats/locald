# RFC 0002: VirtIO MMIO Constants and Sources

## Summary

This RFC documents the hardcoded constants used in the `locald-vmm` VirtIO MMIO implementation and their canonical sources.

## Motivation

We are implementing a custom Virtual Machine Monitor (VMM) using `rust-vmm` components. While `virtio-queue` handles the ring buffer logic, the MMIO transport layer requires specific register offsets and magic values defined by the VirtIO specification. To ensure correctness and maintainability, we must document where these values come from, rather than relying on "magic numbers".

## Reference Source

We use the [Firecracker](https://github.com/firecracker-microvm/firecracker) repository as our primary reference implementation for these constants. Firecracker is a battle-tested VMM that uses the same architecture.

**Reference File:** `src/vmm/src/devices/virtio/transport/mmio.rs` (in Firecracker repo)

## Constants

The following constants are defined in `locald-vmm/src/virtio/mmio.rs` and match the Firecracker implementation.

### Magic and Version

| Constant      | Value        | Description           | Firecracker Ref    |
| :------------ | :----------- | :-------------------- | :----------------- |
| `MAGIC_VALUE` | `0x74726976` | ASCII "virt"          | `MMIO_MAGIC_VALUE` |
| `VERSION`     | `2`          | VirtIO MMIO Version 2 | `MMIO_VERSION`     |
| `VENDOR_ID`   | `0x554d4551` | "QEMU" (See Note 1)   | `VENDOR_ID` (0)    |

**Note 1:** Firecracker uses `0` for Vendor ID. We use `0x554d4551` ("QEMU") which is commonly used by QEMU and other VMMs to ensure broader compatibility with guest kernels that might check this.

### Register Offsets

These offsets are relative to the MMIO base address (`0xd0000000`).

| Offset  | Name                  | Description                         |
| :------ | :-------------------- | :---------------------------------- |
| `0x000` | `MAGIC_VALUE`         | Magic value "virt"                  |
| `0x004` | `VERSION`             | Device version                      |
| `0x008` | `DEVICE_ID`           | VirtIO Subsystem Device ID          |
| `0x00c` | `VENDOR_ID`           | VirtIO Subsystem Vendor ID          |
| `0x010` | `DEVICE_FEATURES`     | Device features                     |
| `0x014` | `DEVICE_FEATURES_SEL` | Device features selection           |
| `0x020` | `DRIVER_FEATURES`     | Driver features                     |
| `0x024` | `DRIVER_FEATURES_SEL` | Driver features selection           |
| `0x030` | `QUEUE_SEL`           | Queue selection                     |
| `0x034` | `QUEUE_NUM_MAX`       | Maximum queue size                  |
| `0x038` | `QUEUE_NUM`           | Queue size                          |
| `0x044` | `QUEUE_READY`         | Queue ready bit                     |
| `0x050` | `QUEUE_NOTIFY`        | Queue notifier                      |
| `0x060` | `INTERRUPT_STATUS`    | Interrupt status                    |
| `0x064` | `INTERRUPT_ACK`       | Interrupt acknowledge               |
| `0x070` | `STATUS`              | Device status                       |
| `0x080` | `QUEUE_DESC_LOW`      | Queue Descriptor Table Low 32-bits  |
| `0x084` | `QUEUE_DESC_HIGH`     | Queue Descriptor Table High 32-bits |
| `0x090` | `QUEUE_AVAIL_LOW`     | Queue Available Ring Low 32-bits    |
| `0x094` | `QUEUE_AVAIL_HIGH`    | Queue Available Ring High 32-bits   |
| `0x0a0` | `QUEUE_USED_LOW`      | Queue Used Ring Low 32-bits         |
| `0x0a4` | `QUEUE_USED_HIGH`     | Queue Used Ring High 32-bits        |
| `0x0fc` | `CONFIG_GENERATION`   | Configuration generation            |
| `0x100` | `CONFIG`              | Device-specific configuration space |

## Implementation Strategy

1.  **Clone Reference**: We clone the Firecracker repository into `references/firecracker` (git-ignored) to verify values.
2.  **Manual Definition**: Since these constants are part of the transport protocol and not exposed by `virtio-queue` (which handles the ring), we define them manually in `locald-vmm`.
3.  **Verification**: We verify these values against the Firecracker source code.

## Future Work

If a `virtio-mmio` crate becomes available in the `rust-vmm` ecosystem that exposes these constants, we should switch to using it to reduce maintenance burden.
