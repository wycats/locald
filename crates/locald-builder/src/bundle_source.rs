use anyhow::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct BundleInfo {
    pub env: Vec<String>,
    pub command: Option<Vec<String>>,
    pub workdir: Option<String>,
    pub bind_mounts: Vec<(String, String)>,
}

#[async_trait]
pub trait BundleSource: Send + Sync {
    /// Prepares the rootfs and returns metadata for the bundle.
    ///
    /// # Arguments
    ///
    /// * `bundle_dir` - The directory where the bundle should be prepared.
    ///                  The implementation should create a `rootfs` subdirectory here
    ///                  or prepare mounts that map to it.
    async fn prepare_rootfs(&self, bundle_dir: &Path) -> Result<BundleInfo>;
}

#[derive(Debug)]
pub struct LocalLayoutBundleSource {
    pub layout_path: PathBuf,
    pub image_ref: String,
    pub app_dir: PathBuf,
}

#[async_trait]
impl BundleSource for LocalLayoutBundleSource {
    async fn prepare_rootfs(&self, bundle_dir: &Path) -> Result<BundleInfo> {
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

        // Use crate::oci_layout which is re-exported from locald-oci
        crate::oci_layout::unpack_image_from_layout(&self.image_ref, &self.layout_path, &rootfs)
            .await?;

        let env = crate::oci_layout::get_image_env(&self.image_ref, &self.layout_path).await?;

        let bind_mounts = vec![(
            self.app_dir.to_string_lossy().to_string(),
            "/workspace".to_string(),
        )];

        Ok(BundleInfo {
            env,
            command: None,
            workdir: Some("/workspace".to_string()),
            bind_mounts,
        })
    }
}
