use crate::bundle_source::{BundleInfo, BundleSource};
use anyhow::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

// Re-export Lifecycle from cnb-client
pub use cnb_client::lifecycle::Lifecycle;

#[derive(Debug)]
pub struct CnbBundleSource {
    pub run_image: String,
    pub layers_dir: PathBuf,
    pub app_dir: PathBuf,
    pub layout_dir: PathBuf,
}

#[async_trait]
impl BundleSource for CnbBundleSource {
    async fn prepare_rootfs(&self, bundle_dir: &Path) -> Result<BundleInfo> {
        // 1. Unpack run image to rootfs
        let rootfs = bundle_dir.join("rootfs");
        match tokio::fs::remove_dir_all(&rootfs).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!("Failed to clean rootfs: {e}. Attempting privileged cleanup...");
                crate::runtime::ShimRuntime::cleanup_path(&rootfs)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to clean rootfs (privileged): {e}"))?;
            }
            _ => {}
        }
        tokio::fs::create_dir_all(&rootfs).await?;

        let run_layout_path = Lifecycle::get_layout_path(&self.layout_dir, &self.run_image);

        crate::oci_layout::unpack_image_from_layout("latest", &run_layout_path, &rootfs).await?;

        // 2. Prepare bind mounts
        let bind_mounts = vec![
            (
                self.layers_dir.to_string_lossy().to_string(),
                "/layers".to_string(),
            ),
            (
                self.app_dir.to_string_lossy().to_string(),
                "/workspace".to_string(),
            ),
        ];

        // 3. Get env from run image
        let env = crate::oci_layout::get_image_env("latest", &run_layout_path).await?;

        Ok(BundleInfo {
            env,
            command: Some(vec!["/cnb/lifecycle/launcher".to_string()]),
            workdir: Some("/workspace".to_string()),
            bind_mounts,
        })
    }
}
