//! Minimal example used for debugging the boot params type.

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn main() {
    // Try to find boot_params
    let _ = linux_loader::bootparam::boot_params::default();
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn main() {
    // `linux_loader` only exposes x86/x86_64 boot params.
}
