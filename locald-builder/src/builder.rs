#![allow(clippy::collapsible_if)]
use crate::image::ContainerImage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

#[derive(Deserialize, Serialize)]
struct OrderToml {
    order: Vec<OrderGroup>,
}

#[derive(Deserialize, Serialize)]
struct OrderGroup {
    group: Vec<BuildpackRef>,
}

#[derive(Deserialize, Serialize)]
struct BuildpackRef {
    id: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    optional: Option<bool>,
}

#[derive(Debug)]
pub struct BuilderImage {
    image: String,
    cache_dir: PathBuf,
    additional_buildpacks: Vec<String>,
}

impl BuilderImage {
    pub fn new(
        image: impl Into<String>,
        cache_dir: impl Into<PathBuf>,
        additional_buildpacks: Vec<String>,
    ) -> Self {
        Self {
            image: image.into(),
            cache_dir: cache_dir.into(),
            additional_buildpacks,
        }
    }

    pub async fn ensure_available(&self) -> Result<PathBuf> {
        let builder_dir = self.cache_dir.join("cnb");
        if builder_dir.exists() {
            info!(
                "Builder image {} already cached at {:?}",
                self.image, builder_dir
            );
        } else {
            info!("Pulling builder image {}...", self.image);
            let container_image = ContainerImage::new(&self.image, &self.cache_dir);
            let (_labels, env, _, _) = container_image.pull().await?;

            if let Some(env_vars) = env {
                self.save_env(&env_vars)?;
            }

            // Ensure binaries are executable (only for builder)
            self.make_executable("cnb/lifecycle/creator")?;
            self.make_executable("cnb/lifecycle/detector")?;
            self.make_executable("cnb/lifecycle/analyzer")?;
            self.make_executable("cnb/lifecycle/restorer")?;
            self.make_executable("cnb/lifecycle/builder")?;
            self.make_executable("cnb/lifecycle/exporter")?;
            self.make_executable("cnb/lifecycle/launcher")?;
        }

        // Ensure system files exist (passwd, group, resolv.conf) for the container
        self.ensure_system_files()?;

        if !self.additional_buildpacks.is_empty() {
            self.inject_buildpacks().await?;
        }

        Ok(builder_dir)
    }

    async fn inject_buildpacks(&self) -> Result<()> {
        for bp_image in &self.additional_buildpacks {
            info!("Injecting buildpack {}...", bp_image);
            let container_image = ContainerImage::new(bp_image, &self.cache_dir);
            let (labels, _, _, _) = container_image.pull().await?;

            if let Some(labels) = labels {
                // Check for standard buildpack ID
                if let (Some(id), Some(version)) = (
                    labels.get("io.buildpacks.buildpack.id"),
                    labels.get("io.buildpacks.buildpack.version"),
                ) {
                    info!("Found buildpack: {}@{}", id, version);
                    self.update_order_toml(id, version)?;
                } else if let Some(metadata_str) = labels.get("io.buildpacks.buildpackage.metadata")
                {
                    // Check for buildpackage metadata (composite buildpacks)
                    if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_str) {
                        if let (Some(id), Some(version)) = (
                            metadata.get("id").and_then(|v| v.as_str()),
                            metadata.get("version").and_then(|v| v.as_str()),
                        ) {
                            info!("Found buildpack: {}@{}", id, version);
                            self.update_order_toml(id, version)?;
                        }
                    }
                } else {
                    info!(
                        "Warning: Could not determine buildpack ID/Version from image labels for {}",
                        bp_image
                    );
                }
            }
        }
        Ok(())
    }

    fn update_order_toml(&self, id: &str, version: &str) -> Result<()> {
        let order_path = self.cache_dir.join("cnb/order.toml");
        let mut order_toml: OrderToml = if order_path.exists() {
            let content = fs::read_to_string(&order_path)?;
            toml::from_str(&content)?
        } else {
            OrderToml { order: Vec::new() }
        };

        // Prepend a new group with just this buildpack
        let new_group = OrderGroup {
            group: vec![BuildpackRef {
                id: id.to_string(),
                version: version.to_string(),
                optional: None,
            }],
        };

        order_toml.order.insert(0, new_group);

        let new_content = toml::to_string_pretty(&order_toml)?;
        fs::write(order_path, new_content)?;

        Ok(())
    }

    fn save_env(&self, env: &[String]) -> Result<()> {
        let env_path = self.cache_dir.join("env");
        let content = serde_json::to_string_pretty(env)?;
        fs::write(env_path, content)?;
        Ok(())
    }

    fn ensure_system_files(&self) -> Result<()> {
        let etc_dir = self.cache_dir.join("etc");
        fs::create_dir_all(&etc_dir)?;

        let passwd_path = etc_dir.join("passwd");
        if !passwd_path.exists() {
            info!("Synthesizing /etc/passwd for builder...");
            // Map cnb to 0 (root) to support rootless execution where only 0 is mapped
            let content = "root:x:0:0:root:/root:/bin/sh\ncnb:x:0:0:cnb:/home/cnb:/bin/sh\n";
            fs::write(passwd_path, content)?;
        }

        let group_path = etc_dir.join("group");
        if !group_path.exists() {
            info!("Synthesizing /etc/group for builder...");
            // Map cnb group to 0
            let content = "root:x:0:root\ncnb:x:0:cnb\n";
            fs::write(group_path, content)?;
        }

        // Copy /etc/resolv.conf from host for DNS resolution
        let resolv_path = etc_dir.join("resolv.conf");
        // Always use Google DNS for now to avoid systemd-resolved issues in container
        fs::write(resolv_path, "nameserver 8.8.8.8\n")?;
        /*
        if !resolv_path.exists() {
            info!("Copying /etc/resolv.conf for builder...");
            if let Err(e) = fs::copy("/etc/resolv.conf", &resolv_path) {
                info!("Warning: Failed to copy /etc/resolv.conf: {}", e);
                // Fallback to Google DNS if copy fails
                fs::write(resolv_path, "nameserver 8.8.8.8\n")?;
            }
        }
        */

        Ok(())
    }

    fn make_executable(&self, relative_path: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = self.cache_dir.join(relative_path);
            if path.exists() {
                let mut perms = fs::metadata(&path)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(path, perms)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_ensure_system_files() -> Result<()> {
        let temp = tempdir()?;
        let cache_dir = temp.path().to_path_buf();
        let builder = BuilderImage::new("test-image", &cache_dir, vec![]);

        // Call the private method (allowed in child module)
        builder.ensure_system_files()?;

        let etc = cache_dir.join("etc");
        assert!(etc.exists());
        assert!(etc.join("passwd").exists());
        assert!(etc.join("group").exists());
        assert!(etc.join("resolv.conf").exists());

        let passwd = fs::read_to_string(etc.join("passwd"))?;
        assert!(passwd.contains("cnb:x:0:0"));

        let resolv = fs::read_to_string(etc.join("resolv.conf"))?;
        // It should contain "nameserver 8.8.8.8" as per current implementation
        assert!(resolv.contains("nameserver 8.8.8.8"));

        Ok(())
    }
}
