use anyhow::{Context, Result};
use oci_distribution::Reference;
use std::path::PathBuf;

#[derive(Debug)]
pub struct BuilderImage {
    #[allow(dead_code)]
    reference: Reference,
}

impl BuilderImage {
    pub fn new(image: &str) -> Result<Self> {
        let reference: Reference = image.parse().context("Failed to parse image reference")?;
        Ok(Self { reference })
    }

    pub async fn pull(&self, _cache_dir: &PathBuf) -> Result<()> {
        // Placeholder for OCI pulling logic
        // We will implement this in the next step
        Ok(())
    }
}
