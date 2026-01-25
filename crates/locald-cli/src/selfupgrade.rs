//! Self-upgrade functionality for locald.
//!
//! Downloads and installs updates from GitHub Releases.

use anyhow::{Context, Result};
use std::path::PathBuf;

const REPO: &str = "wycats/locald";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub Release metadata
#[derive(Debug, serde::Deserialize)]
struct Release {
    tag_name: String,
}

/// Check for available updates
pub fn check() -> Result<Option<String>> {
    let release = fetch_latest_release()?;
    let latest = normalize_version(&release.tag_name);

    if is_newer(latest, CURRENT_VERSION) {
        Ok(Some(latest.to_string()))
    } else {
        Ok(None)
    }
}

/// Perform self-upgrade
pub fn upgrade(version: Option<&str>) -> Result<()> {
    // 1. Determine target version
    let target_version = match version {
        Some(v) => normalize_version(v).to_string(),
        None => {
            let release = fetch_latest_release()?;
            normalize_version(&release.tag_name).to_string()
        }
    };

    println!("Upgrading to v{target_version}...");

    // 2. Detect architecture
    let arch = detect_arch()?;
    let artifact_name = format!("locald-linux-{arch}");

    // 3. Download tarball and checksum
    let base_url = format!("https://github.com/{REPO}/releases/download/v{target_version}");
    let tarball_url = format!("{base_url}/{artifact_name}.tar.gz");
    let checksum_url = format!("{base_url}/{artifact_name}.tar.gz.sha256");

    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let tarball_path = temp_dir.path().join("locald.tar.gz");
    let checksum_path = temp_dir.path().join("locald.tar.gz.sha256");

    println!("Downloading...");
    download_file(&tarball_url, &tarball_path)?;
    download_file(&checksum_url, &checksum_path)?;

    // 4. Verify checksum
    println!("Verifying checksum...");
    verify_checksum(&tarball_path, &checksum_path)?;

    // 5. Extract
    println!("Extracting...");
    let extract_dir = temp_dir.path().join("extract");
    std::fs::create_dir_all(&extract_dir)?;
    extract_tarball(&tarball_path, &extract_dir)?;

    // 6. Check if daemon is running and stop it
    if is_daemon_running() {
        println!("Stopping daemon...");
        stop_daemon();
    }

    // 7. Atomic replace
    let current_exe = std::env::current_exe().context("Failed to get current executable path")?;
    let new_exe = extract_dir.join("locald");

    println!("Installing to {}...", current_exe.display());
    atomic_replace(&new_exe, &current_exe)?;

    // 8. Check shim version
    check_shim_version();

    println!("\n✓ Successfully upgraded to v{target_version}");
    println!("\nRun `locald up` to restart the daemon.");

    Ok(())
}

fn fetch_latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "locald-selfupgrade")
        .send()
        .context("Failed to fetch release info")?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch release: HTTP {}", response.status());
    }

    response.json().context("Failed to parse release info")
}

fn download_file(url: &str, path: &PathBuf) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(url)
        .header("User-Agent", "locald-selfupgrade")
        .send()
        .context("Failed to download file")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed: HTTP {}", response.status());
    }

    let bytes = response.bytes()?;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn verify_checksum(tarball: &PathBuf, checksum_file: &PathBuf) -> Result<()> {
    use sha2::{Digest, Sha256};

    let expected = std::fs::read_to_string(checksum_file)?;
    let expected = expected
        .split_whitespace()
        .next()
        .context("Invalid checksum file format")?;

    let data = std::fs::read(tarball)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let actual = format!("{:x}", hasher.finalize());

    if actual != expected {
        anyhow::bail!("Checksum mismatch!\nExpected: {expected}\nActual:   {actual}");
    }

    Ok(())
}

fn extract_tarball(tarball: &PathBuf, dest: &PathBuf) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file = std::fs::File::open(tarball)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

fn is_daemon_running() -> bool {
    crate::client::send_request(&locald_core::IpcRequest::Ping).is_ok()
}

fn stop_daemon() {
    let _ = crate::client::send_request(&locald_core::IpcRequest::Shutdown);

    for _ in 0..20 {
        if crate::client::send_request(&locald_core::IpcRequest::Ping).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn atomic_replace(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    let backup = dst.with_extension("old");

    // Remove old backup if exists
    let _ = std::fs::remove_file(&backup);

    // Rename current to backup
    std::fs::rename(dst, &backup).context("Failed to backup current binary")?;

    // Copy new binary (can't rename across filesystems)
    std::fs::copy(src, dst).context("Failed to install new binary")?;

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dst)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dst, perms)?;
    }

    let _ = std::fs::remove_file(&backup);

    Ok(())
}

fn check_shim_version() {
    println!("\nNote: If the shim version has changed, run:");
    println!("  sudo locald admin setup");
}

fn detect_arch() -> Result<&'static str> {
    if std::env::consts::OS != "linux" {
        anyhow::bail!("Self-upgrade is only supported on Linux.");
    }

    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Ok("aarch64"),
        arch => anyhow::bail!("Unsupported architecture: {arch}"),
    }
}

fn normalize_version(version: &str) -> &str {
    version.trim_start_matches('v')
}

fn is_newer(latest: &str, current: &str) -> bool {
    let latest = semver::Version::parse(latest).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    let current = semver::Version::parse(current).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    latest > current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_version() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
        assert_eq!(normalize_version("v0.1.0"), "0.1.0");
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer("1.0.0", "0.9.0"));
        assert!(is_newer("1.1.0", "1.0.0"));
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn test_detect_arch() {
        // This should succeed on Linux x86_64 or aarch64 CI environments
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(detect_arch().unwrap(), "x86_64");

        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        assert_eq!(detect_arch().unwrap(), "aarch64");
    }
}
