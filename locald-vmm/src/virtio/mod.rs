//! VirtIO device emulation.
//!
//! This module currently provides a minimal virtio-mmio transport and a
//! virtio-blk device sufficient to boot a Linux guest.

/// `VirtIO` MMIO transport implementation.
pub mod mmio;

/// `VirtIO` block device implementation.
pub mod block;
