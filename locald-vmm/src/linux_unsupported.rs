use std::path::Path;

/// A placeholder Linux `VirtualMachine` implementation for non-x86 Linux.
///
/// The current Linux/KVM backend in `locald-vmm` is implemented for `x86/x86_64`
/// (boot params, CPUID, PIT/IRQ chip, segment regs, etc.).
///
/// On other Linux architectures (e.g. aarch64), we compile a stub so the
/// workspace can build, but VM execution is not yet supported.
#[derive(Debug, Default, Copy, Clone)]
pub struct VirtualMachine;

impl VirtualMachine {
    /// Creates a new Virtual Machine instance.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Attempts to run a Linux kernel in the VM.
    ///
    /// # Errors
    ///
    /// Always returns `ErrorKind::Unsupported` on non-x86 Linux.
    pub fn run_kernel(&mut self, _kernel_path: &Path, _memory_mb: u64) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "locald-vmm: linux backend is only implemented for x86/x86_64 today",
        ))
    }
}
