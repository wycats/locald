use anyhow::Result;
use locald_core::config::{
    CommonServiceConfig, ContainerServiceConfig, ServiceConfig, TypedServiceConfig,
};
use locald_oci::oci_layout::{get_image_config, pull_image_to_layout, unpack_image_from_layout};
use locald_oci::runtime::run;
use locald_oci::runtime_spec::generate_from_service;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::info;

#[derive(Debug)]
pub struct ContainerManager {
    layout_dir: PathBuf,
    bundles_dir: PathBuf,
    active_count: AtomicUsize,
}

struct CountGuard<'a>(&'a AtomicUsize);
impl Drop for CountGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl ContainerManager {
    #[must_use]
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            layout_dir: data_dir.join("oci-layout"),
            bundles_dir: data_dir.join("bundles"),
            active_count: AtomicUsize::new(0),
        }
    }

    pub fn active_count(&self) -> usize {
        self.active_count.load(Ordering::Relaxed)
    }

    pub async fn run(
        &self,
        image: &str,
        command: Option<Vec<String>>,
        _interactive: bool,
        _detached: bool,
        log_tx: Option<tokio::sync::mpsc::Sender<(String, bool)>>,
    ) -> Result<()> {
        self.active_count.fetch_add(1, Ordering::Relaxed);
        // Use a guard to ensure we decrement even if we panic or return early
        let _guard = CountGuard(&self.active_count);

        // 1. Pull Image
        info!("Pulling image {}...", image);
        let _digest = pull_image_to_layout(image, &self.layout_dir).await?;

        // 2. Prepare Bundle
        let container_id = uuid::Uuid::new_v4().to_string();
        let bundle_path = self.bundles_dir.join(&container_id);
        let rootfs_path = bundle_path.join("rootfs");

        info!("Unpacking image to {:?}...", rootfs_path);
        unpack_image_from_layout(image, &self.layout_dir, &rootfs_path).await?;

        // 3. Generate Spec
        info!("Generating runtime spec...");
        let image_config = get_image_config(image, &self.layout_dir).await?;
        let uid = nix::unistd::Uid::current().as_raw();
        let gid = nix::unistd::Gid::current().as_raw();

        let service_config =
            ServiceConfig::Typed(TypedServiceConfig::Container(ContainerServiceConfig {
                common: CommonServiceConfig::default(),
                image: image.to_string(),
                command: command.map(|c| c.join(" ")),
                container_port: None,
                workdir: None,
            }));

        #[cfg(target_os = "linux")]
        let cgroup_path =
            locald_utils::cgroup::maybe_cgroup_path_for_leaf(&format!("adhoc-{container_id}"));
        #[cfg(not(target_os = "linux"))]
        let cgroup_path: Option<String> = None;

        let spec = generate_from_service(
            &service_config,
            &image_config,
            &rootfs_path,
            uid,
            gid,
            0, // container uid (root)
            0, // container gid (root)
            cgroup_path.as_deref(),
        )?;

        let config_path = bundle_path.join("config.json");
        let config_file = std::fs::File::create(&config_path)?;
        serde_json::to_writer(config_file, &spec)?;

        // 4. Run
        info!("Running container {}...", container_id);
        // For now, we run in foreground (blocking).
        // TODO: Handle detached/interactive
        run(&bundle_path, &container_id, log_tx).await?;

        // 5. Cleanup
        info!("Cleaning up container {}...", container_id);
        if let Err(e) = std::fs::remove_dir_all(&bundle_path) {
            tracing::warn!("Failed to cleanup bundle: {e}. Attempting privileged cleanup...");
            // We need to use ShimRuntime here, but it's in locald-builder.
            // locald-server depends on locald-builder.
            // However, this function is not async, but ShimRuntime::cleanup_path is async.
            // We are in an async context (run is async), so we can await.
            // Wait, run is async.

            locald_builder::ShimRuntime::cleanup_path(&bundle_path).await?;
        }

        Ok(())
    }
}
