use anyhow::{Context, Result};
use fetch_kernel::{ensure_assets, ensure_references};

fn main() -> Result<()> {
    let workspace_root = std::env::current_dir().context("Failed to get current dir")?;

    // Assuming we run this from the workspace root
    let assets_dir = workspace_root.join("locald-vmm/assets");
    let references_dir = workspace_root.join("references");

    let (kernel, rootfs) = ensure_assets(&assets_dir)?;
    let firecracker = ensure_references(&references_dir)?;

    println!("Assets ready:");
    println!("  Kernel: {}", kernel.display());
    println!("  Rootfs: {}", rootfs.display());
    println!("  Firecracker Ref: {}", firecracker.display());
    Ok(())
}
