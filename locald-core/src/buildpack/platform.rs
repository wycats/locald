use super::lifecycle::Lifecycle;
use super::progress::BuildProgress;
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Platform {
    pub app_dir: PathBuf,
    pub layers_dir: PathBuf,
    pub platform_dir: PathBuf,
    pub lifecycle: Lifecycle,
}

impl Platform {
    pub fn new(app_dir: PathBuf, layers_dir: PathBuf, lifecycle_root: PathBuf) -> Self {
        Self {
            app_dir,
            layers_dir,
            platform_dir: PathBuf::from("platform"), // TODO: Make configurable
            lifecycle: Lifecycle::new(lifecycle_root),
        }
    }

    pub async fn detect(
        &self,
        buildpacks_dir: &std::path::Path,
        order_toml: &std::path::Path,
        progress: &impl BuildProgress,
    ) -> Result<()> {
        let args = [
            "-app",
            self.app_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid app_dir path"))?,
            "-buildpacks",
            buildpacks_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid buildpacks_dir path"))?,
            "-order",
            order_toml
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid order_toml path"))?,
            "-layers",
            self.layers_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid layers_dir path"))?,
            "-platform",
            self.platform_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid platform_dir path"))?,
        ];
        let env = [("CNB_PLATFORM_API", "0.14")];

        self.lifecycle
            .run_phase(
                "detector",
                self.lifecycle.detector_path(),
                &args,
                &env,
                progress,
            )
            .await
    }

    pub async fn build(
        &self,
        buildpacks_dir: &std::path::Path,
        progress: &impl BuildProgress,
    ) -> Result<()> {
        let args = [
            "-app",
            self.app_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid app_dir path"))?,
            "-buildpacks",
            buildpacks_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid buildpacks_dir path"))?,
            "-layers",
            self.layers_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid layers_dir path"))?,
            "-platform",
            self.platform_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid platform_dir path"))?,
        ];
        let env = [("CNB_PLATFORM_API", "0.14")];

        self.lifecycle
            .run_phase(
                "builder",
                self.lifecycle.builder_path(),
                &args,
                &env,
                progress,
            )
            .await
    }
}
