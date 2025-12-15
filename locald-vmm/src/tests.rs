#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    #[ignore = "integration test downloads large VM assets; run manually with `cargo test -p locald-vmm -- --ignored`"]
    fn test_fetch_kernel_integration() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let assets_dir = manifest_dir.join("assets");

        let result = fetch_kernel::ensure_assets(&assets_dir);
        assert!(
            result.is_ok(),
            "Failed to fetch kernel assets: {:?}",
            result.err()
        );

        let (kernel, rootfs) = result.unwrap();
        assert!(kernel.exists());
        assert!(rootfs.exists());
    }
}
