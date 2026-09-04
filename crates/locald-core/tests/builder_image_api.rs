//! Compile contract for the public builder-image API.

use anyhow::Result;
use locald_core::buildpack::image::BuilderImage;
use std::future::Future;
use std::path::PathBuf;

fn assert_pull_future(future: impl Future<Output = Result<()>>) {
    drop(future);
}

#[test]
fn builder_image_pull_remains_public_and_async() -> Result<()> {
    let image = BuilderImage::new("docker.io/example/builder:latest")?;
    let cache_dir = PathBuf::from("builder-image-cache");

    assert_pull_future(image.pull(&cache_dir));

    Ok(())
}
