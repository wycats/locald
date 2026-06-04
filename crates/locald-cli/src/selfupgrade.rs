//! Self-upgrade functionality for locald.
//!
//! Downloads and installs updates from GitHub Releases.

use crate::error::{CliError, CliResult};
use anyhow::Context;
use std::path::Path;

const REPO: &str = "wycats/locald";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub Release metadata
#[derive(Debug, serde::Deserialize)]
struct Release {
    tag_name: String,
}

/// Check for available updates
pub fn check() -> CliResult<Option<String>> {
    let release = fetch_latest_release()?;
    let latest = normalize_version(&release.tag_name);

    if is_newer(latest, CURRENT_VERSION)? {
        Ok(Some(latest.to_string()))
    } else {
        Ok(None)
    }
}

/// Perform self-upgrade
pub fn upgrade(version: Option<&str>) -> CliResult<()> {
    // 1. Determine target version
    let target_version = match version {
        Some(v) => {
            let normalized = normalize_version(v);
            let tag = format!("v{normalized}");
            let release = fetch_release_by_tag(&tag)?;
            normalize_version(&release.tag_name).to_string()
        }
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
        stop_daemon()?;
    }

    // 7. Atomic replace
    let current_exe = std::env::current_exe().context("Failed to get current executable path")?;
    let new_exe = extract_dir.join("locald");

    if !new_exe.is_file() {
        return Err(CliError::message(format!(
            "Downloaded archive does not contain expected 'locald' binary at {}",
            new_exe.display()
        )));
    }

    println!("Installing to {}...", current_exe.display());
    atomic_replace(&new_exe, &current_exe)?;

    // 8. Check shim version
    check_shim_version();

    println!("\n✓ Successfully upgraded to v{target_version}");
    println!("\nRun `locald up` to restart the daemon.");

    Ok(())
}

fn fetch_latest_release() -> CliResult<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "locald-selfupgrade")
        .send()
        .context("Failed to fetch release info")?;

    if !response.status().is_success() {
        return Err(CliError::message(format!(
            "Failed to fetch release: HTTP {}",
            response.status()
        )));
    }

    response
        .json()
        .context("Failed to parse release info")
        .map_err(Into::into)
}

fn fetch_release_by_tag(tag: &str) -> CliResult<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/tags/{tag}");
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "locald-selfupgrade")
        .send()
        .context("Failed to fetch release info")?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(CliError::message(format!(
            "Version {tag} not found in releases"
        )));
    }

    if !response.status().is_success() {
        return Err(CliError::message(format!(
            "Failed to fetch release {tag}: HTTP {}",
            response.status()
        )));
    }

    response
        .json()
        .context("Failed to parse release info")
        .map_err(Into::into)
}

fn download_file(url: &str, path: &Path) -> CliResult<()> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(url)
        .header("User-Agent", "locald-selfupgrade")
        .send()
        .context("Failed to download file")?;

    if !response.status().is_success() {
        return Err(CliError::message(format!(
            "Download failed: HTTP {}",
            response.status()
        )));
    }

    let bytes = response.bytes()?;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn verify_checksum(tarball: &Path, checksum_file: &Path) -> CliResult<()> {
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
        return Err(CliError::message(format!(
            "Checksum mismatch!\nExpected: {expected}\nActual:   {actual}"
        )));
    }

    Ok(())
}

fn extract_tarball(tarball: &Path, dest: &Path) -> CliResult<()> {
    use flate2::read::GzDecoder;
    use std::path::Component;
    use tar::Archive;

    let file = std::fs::File::open(tarball)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(CliError::message(format!(
                "Tarball contains an entry with a parent directory component: {}",
                path.display()
            )));
        }

        entry.unpack_in(dest)?;
    }
    Ok(())
}

fn is_daemon_running() -> bool {
    crate::client::send_request(&locald_core::IpcRequest::Ping).is_ok()
}

fn stop_daemon() -> CliResult<()> {
    let _ = crate::client::send_request(&locald_core::IpcRequest::Shutdown);

    for _ in 0..20 {
        if crate::client::send_request(&locald_core::IpcRequest::Ping).is_err() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    Err(CliError::message(
        "locald daemon appears to still be running after shutdown timeout; \
proceeding with self-upgrade may fail if the binary is still in use.",
    ))
}

fn atomic_replace(src: &Path, dst: &Path) -> CliResult<()> {
    let backup = dst.with_extension("old");
    let tmp = dst.with_extension("new");

    // Remove old backup or temp if exists
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_file(&tmp);

    // Create a backup copy of the current binary, if it exists
    if dst.exists() {
        std::fs::copy(dst, &backup).context("Failed to backup current binary")?;
    }

    // Copy new binary to a temporary location next to the destination
    std::fs::copy(src, &tmp).context("Failed to write new binary to temporary location")?;

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms)?;
    }

    // Atomically replace the destination with the temporary file
    std::fs::rename(&tmp, dst).context("Failed to atomically replace current binary")?;

    let _ = std::fs::remove_file(&backup);

    Ok(())
}

fn check_shim_version() {
    println!("\nNote: If the shim version has changed, run:");
    println!("  sudo locald admin setup");
}

fn detect_arch() -> CliResult<&'static str> {
    if std::env::consts::OS != "linux" {
        return Err(CliError::message(
            "Self-upgrade is only supported on Linux.",
        ));
    }

    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Ok("aarch64"),
        arch => Err(CliError::message(format!(
            "Unsupported architecture: {arch}"
        ))),
    }
}

fn normalize_version(version: &str) -> &str {
    version.trim_start_matches('v')
}

fn is_newer(latest: &str, current: &str) -> CliResult<bool> {
    let latest = semver::Version::parse(latest)
        .with_context(|| format!("Invalid latest version '{latest}'"))?;
    let current = semver::Version::parse(current)
        .with_context(|| format!("Invalid current version '{current}'"))?;
    Ok(latest > current)
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
        assert!(is_newer("1.0.0", "0.9.0").unwrap());
        assert!(is_newer("1.1.0", "1.0.0").unwrap());
        assert!(is_newer("1.0.1", "1.0.0").unwrap());
        assert!(!is_newer("1.0.0", "1.0.0").unwrap());
        assert!(!is_newer("0.9.0", "1.0.0").unwrap());
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
