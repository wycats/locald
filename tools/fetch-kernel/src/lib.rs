use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub const KERNEL_URL: &str = "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/4360-tmp-artifacts/x86_64/vmlinux-5.10.209";
pub const ROOTFS_URL: &str = "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/4360-tmp-artifacts/x86_64/ubuntu-22.04.ext4";
pub const FIRECRACKER_REPO: &str = "https://github.com/firecracker-microvm/firecracker.git";

fn download_file(url: &str, dest: &Path) -> Result<()> {
    if dest.exists() {
        println!("{} already exists, skipping.", dest.display());
        return Ok(());
    }

    println!("Downloading {}...", url);
    let response = reqwest::blocking::get(url).context("Failed to send request")?;
    let content = response.bytes().context("Failed to read response bytes")?;

    let mut file = File::create(dest).context("Failed to create file")?;
    file.write_all(&content)
        .context("Failed to write content")?;

    println!("Saved to {}", dest.display());
    Ok(())
}

pub fn ensure_references(references_dir: &Path) -> Result<PathBuf> {
    if !references_dir.exists() {
        fs::create_dir_all(references_dir).context("Failed to create references directory")?;
    }

    let firecracker_dir = references_dir.join("firecracker");
    if firecracker_dir.exists() {
        println!(
            "Firecracker reference already exists at {}, skipping.",
            firecracker_dir.display()
        );
    } else {
        println!("Cloning Firecracker reference...");
        let mut cmd = Command::new("git");
        cmd.args([
            "clone",
            "--depth",
            "1",
            FIRECRACKER_REPO,
            firecracker_dir
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("firecracker_dir is not valid UTF-8"))?,
        ]);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to build tokio runtime for git clone")?;

        let status = rt
            .block_on(cmd.status())
            .context("Failed to execute git clone")?;

        if !status.success() {
            anyhow::bail!("git clone failed");
        }
    }

    Ok(firecracker_dir)
}

pub fn ensure_assets(assets_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    if !assets_dir.exists() {
        fs::create_dir_all(assets_dir).context("Failed to create assets directory")?;
    }

    let kernel_path = assets_dir.join("vmlinux");
    let rootfs_path = assets_dir.join("rootfs.ext4");

    download_file(KERNEL_URL, &kernel_path)?;
    download_file(ROOTFS_URL, &rootfs_path)?;

    Ok((kernel_path, rootfs_path))
}
