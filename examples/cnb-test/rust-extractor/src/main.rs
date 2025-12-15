#![allow(clippy::collapsible_if)]
#![allow(clippy::manual_flatten)]
#![allow(missing_docs)]
#![allow(clippy::print_stdout)]
use anyhow::{Result, anyhow};
use flate2::read::GzDecoder;
use oci_distribution::Reference;
use oci_distribution::client::{Client, ClientConfig};
use oci_distribution::manifest::OciManifest;
use oci_distribution::secrets::RegistryAuth;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use tar::Archive;

#[tokio::main]
async fn main() -> Result<()> {
    let image = "heroku/builder:22";
    let reference: Reference = image.parse()?;

    let client = Client::new(ClientConfig::default());

    println!("Pulling manifest for {image}");
    let (manifest, _) = client
        .pull_manifest(&reference, &RegistryAuth::Anonymous)
        .await?;

    let output_dir = Path::new("../../builder-data");
    if output_dir.exists() {
        fs::remove_dir_all(output_dir)?;
    }
    fs::create_dir_all(output_dir)?;

    if let OciManifest::Image(image_manifest) = manifest {
        println!("Manifest pulled successfully!");
        println!("Layers: {}", image_manifest.layers.len());

        for (i, layer) in image_manifest.layers.iter().enumerate() {
            println!(
                "Checking layer {} (digest: {}, size: {})",
                i, layer.digest, layer.size
            );

            // Let's try to pull the layer.
            let mut blob_data = Vec::new();
            client.pull_blob(&reference, layer, &mut blob_data).await?;

            println!("  Downloaded {} bytes", blob_data.len());

            // Try to list files
            let cursor = Cursor::new(blob_data);
            let decoder = GzDecoder::new(cursor);
            let mut archive = Archive::new(decoder);

            let mut found_cnb = false;
            if let Ok(entries) = archive.entries() {
                for entry in entries {
                    if let Ok(mut entry) = entry {
                        if let Ok(path) = entry.path() {
                            let path_str = path.to_string_lossy();
                            if path_str.contains("cnb") {
                                // Extract
                                // Paths are like /cnb/..., we want to extract to output_dir/cnb/...
                                // We need to strip the leading / if present
                                let relative_path = if path_str.starts_with('/') {
                                    path_str.trim_start_matches('/').to_string()
                                } else {
                                    path_str.to_string()
                                };

                                let target_path = output_dir.join(relative_path);
                                if let Some(parent) = target_path.parent() {
                                    fs::create_dir_all(parent)?;
                                }

                                entry.unpack(&target_path)?;
                                found_cnb = true;
                            }
                        }
                    }
                }
            }

            if found_cnb {
                println!("  Layer {i} contained /cnb data, extracted.");
            }
        }
    } else {
        return Err(anyhow!("Unexpected manifest type"));
    }

    Ok(())
}
