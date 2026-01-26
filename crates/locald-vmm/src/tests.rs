#[cfg(test)]
mod tests {
    use crate::VmConfig;
    use std::path::PathBuf;

    #[test]
    fn vm_config_preserves_inputs() {
        let kernel_path = PathBuf::from("kernel.img");
        let config = VmConfig {
            kernel_path: kernel_path.clone(),
            memory_mb: 512,
        };

        assert_eq!(config.kernel_path, kernel_path);
        assert_eq!(config.memory_mb, 512);
    }

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
