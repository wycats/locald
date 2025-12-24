use anyhow::{Result, anyhow};
use flate2::read::GzDecoder;
use oci_distribution::Reference;
use oci_distribution::client::{Client, ClientConfig};
use oci_distribution::manifest::OciManifest;
use oci_distribution::secrets::RegistryAuth;
use serde::Deserialize;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tar::{Archive, EntryType};
use tracing::{debug, info};

#[derive(Deserialize)]
struct ImageConfig {
    config: ConfigConfig,
}

#[derive(Deserialize)]
struct ConfigConfig {
    #[serde(rename = "Labels")]
    labels: Option<std::collections::HashMap<String, String>>,
    #[serde(rename = "Env")]
    env: Option<Vec<String>>,
    #[serde(rename = "Cmd")]
    cmd: Option<Vec<String>>,
    #[serde(rename = "WorkingDir")]
    working_dir: Option<String>,
}

#[derive(Debug)]
pub struct ImageFetcher {
    image: String,
    cache_dir: PathBuf,
}

impl ImageFetcher {
    pub fn new(image: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            image: image.into(),
            cache_dir: cache_dir.into(),
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub async fn pull(
        &self,
    ) -> Result<(
        Option<std::collections::HashMap<String, String>>,
        Option<Vec<String>>,
        Option<Vec<String>>,
        Option<String>,
    )> {
        let reference: Reference = self.image.parse()?;
        let client = Client::new(ClientConfig::default());

        let (manifest, _) = client
            .pull_manifest(&reference, &RegistryAuth::Anonymous)
            .await?;

        match manifest {
            OciManifest::Image(image_manifest) => {
                let (labels, env, cmd, workdir) = self
                    .process_config(&client, &reference, &image_manifest)
                    .await?;
                self.extract_layers(&client, &reference, &image_manifest, &self.cache_dir)
                    .await?;

                Ok((labels, env, cmd, workdir))
            }
            OciManifest::ImageIndex(list) => {
                info!("Manifest list found. Resolving for linux/amd64...");
                let entry = list
                    .manifests
                    .iter()
                    .find(|m| {
                        m.platform
                            .as_ref()
                            .is_some_and(|p| p.architecture == "amd64" && p.os == "linux")
                    })
                    .ok_or_else(|| anyhow!("No linux/amd64 manifest found in manifest list"))?;

                info!("Resolved to {}", entry.digest);

                let new_ref = Reference::with_digest(
                    reference.registry().to_string(),
                    reference.repository().to_string(),
                    entry.digest.clone(),
                );

                let (resolved_manifest, _) = client
                    .pull_manifest(&new_ref, &RegistryAuth::Anonymous)
                    .await?;

                if let OciManifest::Image(image_manifest) = resolved_manifest {
                    let (labels, env, cmd, workdir) = self
                        .process_config(&client, &new_ref, &image_manifest)
                        .await?;
                    self.extract_layers(&client, &new_ref, &image_manifest, &self.cache_dir)
                        .await?;

                    Ok((labels, env, cmd, workdir))
                } else {
                    Err(anyhow!("Resolved manifest was not an image manifest"))
                }
            }
        }
    }

    pub async fn ensure_system_files(&self) -> Result<()> {
        let etc_dir = self.cache_dir.join("etc");
        tokio::fs::create_dir_all(&etc_dir).await?;

        let passwd_path = etc_dir.join("passwd");
        if !passwd_path.exists() {
            info!("Synthesizing /etc/passwd for container...");
            // Map root to 0
            let content = "root:x:0:0:root:/root:/bin/sh\n";
            tokio::fs::write(passwd_path, content).await?;
        }

        let group_path = etc_dir.join("group");
        if !group_path.exists() {
            info!("Synthesizing /etc/group for container...");
            // Map root group to 0
            let content = "root:x:0:root\n";
            tokio::fs::write(group_path, content).await?;
        }

        // Copy /etc/resolv.conf from host for DNS resolution
        let resolv_path = etc_dir.join("resolv.conf");
        // Always use Google DNS for now to avoid systemd-resolved issues in container
        tokio::fs::write(resolv_path, "nameserver 8.8.8.8\n").await?;

        Ok(())
    }

    async fn process_config(
        &self,
        client: &Client,
        reference: &Reference,
        image_manifest: &oci_distribution::manifest::OciImageManifest,
    ) -> Result<(
        Option<std::collections::HashMap<String, String>>,
        Option<Vec<String>>,
        Option<Vec<String>>,
        Option<String>,
    )> {
        let mut config_data = Vec::new();
        client
            .pull_blob(reference, &image_manifest.config, &mut config_data)
            .await?;

        let config: ImageConfig = serde_json::from_slice(&config_data)?;
        Ok((
            config.config.labels,
            config.config.env,
            config.config.cmd,
            config.config.working_dir,
        ))
    }

    async fn extract_layers(
        &self,
        client: &Client,
        reference: &Reference,
        image_manifest: &oci_distribution::manifest::OciImageManifest,
        target_dir: &PathBuf,
    ) -> Result<()> {
        info!("Manifest pulled. Layers: {}", image_manifest.layers.len());

        fs::create_dir_all(target_dir)?;

        for (i, layer) in image_manifest.layers.iter().enumerate() {
            debug!(
                "Checking layer {} (digest: {}, size: {})",
                i, layer.digest, layer.size
            );

            let mut blob_data = Vec::new();
            client.pull_blob(reference, layer, &mut blob_data).await?;

            let cursor = Cursor::new(blob_data);
            let decoder = GzDecoder::new(cursor);
            let mut archive = Archive::new(decoder);

            if let Ok(entries) = archive.entries() {
                for entry in entries {
                    match entry {
                        Ok(mut entry) => {
                            let path = match entry.path() {
                                Ok(p) => p.into_owned(),
                                Err(e) => {
                                    info!("Skipping entry with invalid path: {}", e);
                                    continue;
                                }
                            };
                            let path_str = path.to_string_lossy().to_string();

                            // Extract to target_dir
                            // Paths are like /cnb/..., we want to extract to target_dir/cnb/...
                            // We need to strip the leading / if present
                            let relative_path = if path_str.starts_with('/') {
                                path_str.trim_start_matches('/').to_string()
                            } else {
                                path_str.clone()
                            };

                            let target_path = target_dir.join(relative_path);

                            // Skip if it's a whiteout file (Docker/OCI specific)
                            if path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|fname| fname.starts_with(".wh."))
                            {
                                debug!("Skipping whiteout file: {}", path_str);
                                continue;
                            }

                            if let Some(e) = target_path
                                .parent()
                                .and_then(|p| fs::create_dir_all(p).err())
                            {
                                info!("Failed to create parent dir for {}: {}", path_str, e);
                                continue;
                            }

                            // Handle hard links manually to resolve targets correctly
                            if entry.header().entry_type() == EntryType::Link {
                                if let Err(e) =
                                    locald_utils::fs::unpack_hard_link(&entry, target_dir)
                                {
                                    info!("Failed to unpack hard link: {}", e);
                                }
                                continue;
                            }

                            // Handle symlinks and files
                            if let Err(e) = entry.unpack(&target_path) {
                                info!("Failed to unpack {}: {}", path_str, e);
                            }
                        }
                        Err(e) => {
                            info!("Error reading tar entry: {}", e);
                        }
                    }
                }
            } else {
                info!("Failed to read entries from archive");
            }
        }
        Ok(())
    }
}
