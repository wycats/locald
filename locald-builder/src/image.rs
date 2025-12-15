#![allow(clippy::collapsible_if)]
use crate::bundle_source::{BundleInfo, BundleSource};
use anyhow::Result;
use async_trait::async_trait;
use locald_oci::fetcher::ImageFetcher;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ContainerImage {
    fetcher: ImageFetcher,
}

impl ContainerImage {
    pub fn new(image: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            fetcher: ImageFetcher::new(image, cache_dir),
        }
    }

    pub async fn pull(
        &self,
    ) -> Result<(
        Option<std::collections::HashMap<String, String>>,
        Option<Vec<String>>,
        Option<Vec<String>>,
        Option<String>,
    )> {
        self.fetcher.pull().await
    }

    pub async fn ensure_system_files(&self) -> Result<()> {
        self.fetcher.ensure_system_files().await
    }
}

#[async_trait]
impl BundleSource for ContainerImage {
    async fn prepare_rootfs(&self, bundle_dir: &Path) -> Result<BundleInfo> {
        let (_labels, image_env, image_cmd, image_workdir) = self.pull().await?;
        self.ensure_system_files().await?;

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

        let cache_dir = self.fetcher.cache_dir();
        copy_dir_recursive(cache_dir, &rootfs)?;

        Ok(BundleInfo {
            env: image_env.unwrap_or_default(),
            command: image_cmd,
            workdir: image_workdir,
            bind_mounts: vec![],
        })
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
