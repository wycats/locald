use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use oci_distribution::Reference;
use oci_distribution::client::{Client, ClientConfig};
use oci_distribution::manifest::OciManifest;
use oci_distribution::secrets::RegistryAuth;
use oci_spec::image::ImageConfiguration;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tar::Archive;
use tokio::fs;
use tracing::{debug, info};

#[derive(Serialize, Deserialize)]
struct OciLayout {
    #[serde(rename = "imageLayoutVersion")]
    image_layout_version: String,
}

#[derive(Serialize, Deserialize)]
struct Index {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    manifests: Vec<ManifestDescriptor>,
}

#[derive(Serialize, Deserialize)]
struct ManifestDescriptor {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<Platform>,
}

#[derive(Serialize, Deserialize)]
struct Platform {
    architecture: String,
    os: String,
}

pub async fn get_image_labels(
    image_ref: &str,
    layout_dir: &Path,
) -> Result<HashMap<String, String>> {
    let config = get_image_config(image_ref, layout_dir).await?;
    Ok(config
        .config()
        .as_ref()
        .and_then(|c| c.labels().as_ref())
        .cloned()
        .unwrap_or_default())
}

pub async fn pull_image_to_layout(image: &str, layout_dir: &Path) -> Result<String> {
    let reference: Reference = image.parse()?;
    let client = Client::new(ClientConfig::default());

    info!("Pulling image {} to OCI layout at {:?}", image, layout_dir);

    let (manifest, digest) = client
        .pull_manifest(&reference, &RegistryAuth::Anonymous)
        .await?;

    let blobs_dir = layout_dir.join("blobs/sha256");
    fs::create_dir_all(&blobs_dir).await?;

    // Write oci-layout file
    let oci_layout = OciLayout {
        image_layout_version: "1.0.0".to_string(),
    };
    let oci_layout_path = layout_dir.join("oci-layout");
    if !oci_layout_path.exists() {
        let content = serde_json::to_string(&oci_layout)?;
        fs::write(oci_layout_path, content).await?;
    }

    match manifest {
        OciManifest::Image(image_manifest) => {
            write_image_manifest_to_layout(
                &client,
                &reference,
                &image_manifest,
                &digest,
                layout_dir,
                image,
                &blobs_dir,
            )
            .await?;
            Ok(digest)
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
                .context("No linux/amd64 manifest found in manifest list")?;

            info!("Resolved to {}", entry.digest);

            let new_ref = Reference::with_digest(
                reference.registry().to_string(),
                reference.repository().to_string(),
                entry.digest.clone(),
            );

            let (resolved_manifest, resolved_digest) = client
                .pull_manifest(&new_ref, &RegistryAuth::Anonymous)
                .await?;

            if let OciManifest::Image(image_manifest) = resolved_manifest {
                write_image_manifest_to_layout(
                    &client,
                    &new_ref,
                    &image_manifest,
                    &resolved_digest,
                    layout_dir,
                    image,
                    &blobs_dir,
                )
                .await?;
                Ok(resolved_digest)
            } else {
                Err(anyhow::anyhow!(
                    "Resolved manifest was not an image manifest"
                ))
            }
        }
    }
}

async fn write_image_manifest_to_layout(
    client: &Client,
    reference: &Reference,
    image_manifest: &oci_distribution::manifest::OciImageManifest,
    digest: &str,
    layout_dir: &Path,
    original_image_name: &str,
    blobs_dir: &Path,
) -> Result<()> {
    // 1. Pull and write config blob
    let config_digest = &image_manifest.config.digest;
    let config_path = blobs_dir.join(config_digest.trim_start_matches("sha256:"));
    if !config_path.exists() {
        debug!("Pulling config blob {}", config_digest);
        let mut config_data = Vec::new();
        client
            .pull_blob(reference, &image_manifest.config, &mut config_data)
            .await?;
        fs::write(config_path, config_data).await?;
    }

    // 2. Pull and write layers
    for layer in &image_manifest.layers {
        let layer_digest = &layer.digest;
        let layer_path = blobs_dir.join(layer_digest.trim_start_matches("sha256:"));
        if !layer_path.exists() {
            debug!("Pulling layer blob {}", layer_digest);
            let mut layer_data = Vec::new();
            client.pull_blob(reference, layer, &mut layer_data).await?;
            fs::write(layer_path, layer_data).await?;
        }
    }

    // 3. Write manifest blob
    let manifest_json = serde_json::to_string(&image_manifest)?;
    let manifest_path = blobs_dir.join(digest.trim_start_matches("sha256:"));
    fs::write(&manifest_path, &manifest_json).await?;

    // 4. Update index.json
    let index_path = layout_dir.join("index.json");
    let mut index = if index_path.exists() {
        let content = fs::read_to_string(&index_path).await?;
        serde_json::from_str(&content)?
    } else {
        Index {
            schema_version: 2,
            manifests: Vec::new(),
        }
    };

    // Remove existing entries for this ref name to ensure we point to the latest digest
    index.manifests.retain(|m| {
        !m.annotations.as_ref().is_some_and(|a| {
            a.get("org.opencontainers.image.ref.name")
                .map(String::as_str)
                == Some(original_image_name)
        })
    });

    let mut annotations = HashMap::new();
    annotations.insert(
        "org.opencontainers.image.ref.name".to_string(),
        original_image_name.to_string(),
    );

    index.manifests.push(ManifestDescriptor {
        media_type: image_manifest
            .media_type
            .clone()
            .unwrap_or_else(|| "application/vnd.oci.image.manifest.v1+json".to_string()),
        digest: digest.to_string(),
        size: manifest_json.len() as u64,
        annotations: Some(annotations),
        platform: Some(Platform {
            architecture: "amd64".to_string(),
            os: "linux".to_string(),
        }),
    });

    let index_json = serde_json::to_string_pretty(&index)?;
    fs::write(index_path, index_json).await?;

    Ok(())
}
pub async fn unpack_image_from_layout(
    image_ref: &str,
    layout_dir: &Path,
    rootfs: &Path,
) -> Result<()> {
    info!(
        "Unpacking image {} from {:?} to {:?}",
        image_ref, layout_dir, rootfs
    );

    // 1. Read index.json
    let index_path = layout_dir.join("index.json");
    let index_content = fs::read_to_string(&index_path).await?;
    let index: Index = serde_json::from_str(&index_content)?;

    // 2. Find manifest for image_ref
    // We look for annotation "org.opencontainers.image.ref.name" == image_ref
    let manifest_desc = index
        .manifests
        .iter()
        .find(|m| {
            m.annotations.as_ref().is_some_and(|a| {
                a.get("org.opencontainers.image.ref.name")
                    .is_some_and(|v| v == image_ref)
            })
        })
        .context(format!("Image {image_ref} not found in layout index"))?;

    // 3. Read Manifest
    let blobs_dir = layout_dir.join("blobs/sha256");
    let manifest_path = blobs_dir.join(manifest_desc.digest.trim_start_matches("sha256:"));
    let manifest_content = fs::read_to_string(&manifest_path).await?;
    let manifest: oci_distribution::manifest::OciImageManifest =
        serde_json::from_str(&manifest_content)?;

    // 4. Unpack Layers
    fs::create_dir_all(rootfs).await?;

    for layer in manifest.layers {
        let layer_digest = layer.digest;
        let layer_path = blobs_dir.join(layer_digest.trim_start_matches("sha256:"));

        info!("Unpacking layer {}", layer_digest);

        let rootfs = rootfs.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let file = std::fs::File::open(layer_path)?;
            let decoder = GzDecoder::new(file);
            let mut archive = Archive::new(decoder);
            // Preserve permissions?
            archive.set_preserve_permissions(true);
            archive.set_unpack_xattrs(true);
            archive.unpack(&rootfs)?;
            Ok(())
        })
        .await??;
    }

    Ok(())
}

pub async fn get_image_env(image_ref: &str, layout_dir: &Path) -> Result<Vec<String>> {
    let config = get_image_config(image_ref, layout_dir).await?;
    Ok(config
        .config()
        .as_ref()
        .and_then(|c| c.env().as_ref())
        .cloned()
        .unwrap_or_default())
}

pub async fn get_image_config(image_ref: &str, layout_dir: &Path) -> Result<ImageConfiguration> {
    // 1. Read index.json
    let index_path = layout_dir.join("index.json");
    let index_content = fs::read_to_string(&index_path).await?;
    let index: Index = serde_json::from_str(&index_content)?;

    // 2. Find manifest for image_ref
    let manifest_desc = index
        .manifests
        .iter()
        .find(|m| {
            m.annotations.as_ref().is_some_and(|a| {
                a.get("org.opencontainers.image.ref.name")
                    .is_some_and(|v| v == image_ref)
            })
        })
        .context(format!("Image {image_ref} not found in layout index"))?;

    // 3. Read Manifest
    let blobs_dir = layout_dir.join("blobs/sha256");
    let manifest_path = blobs_dir.join(manifest_desc.digest.trim_start_matches("sha256:"));
    let manifest_content = fs::read_to_string(&manifest_path).await?;
    let manifest: oci_distribution::manifest::OciImageManifest =
        serde_json::from_str(&manifest_content)?;

    // 4. Read Config Blob
    let config_digest = manifest.config.digest;
    let config_path = blobs_dir.join(config_digest.trim_start_matches("sha256:"));
    let config_content = fs::read_to_string(&config_path).await?;
    let config: ImageConfiguration = serde_json::from_str(&config_content)?;

    Ok(config)
}
