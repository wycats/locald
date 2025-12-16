//! Minimal example used for debugging the boot params type.

fn main() {
    // Try to find boot_params
    let _ = linux_loader::bootparam::boot_params::default();
}
