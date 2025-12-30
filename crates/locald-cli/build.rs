#![allow(clippy::expect_used)]
#![allow(missing_docs)]
#![allow(clippy::disallowed_methods)]
use std::env;
use std::path::PathBuf;
use std::process::Command;
// use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Re-run if Cargo.toml changes (version change)
    println!("cargo:rerun-if-changed=Cargo.toml");
    // Re-run if source code changes (to update timestamp on rebuild)
    println!("cargo:rerun-if-changed=src");
    // Re-run if dependencies change (so we get a new timestamp/version)
    println!("cargo:rerun-if-changed=../locald-server/src");
    println!("cargo:rerun-if-changed=../locald-builder/src");
    println!("cargo:rerun-if-changed=../locald-core/src");

    // Build locald-shim on Linux
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rerun-if-changed=../locald-shim/src");
        println!("cargo:rerun-if-changed=../locald-shim/Cargo.toml");

        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
        let shim_dir = PathBuf::from("../locald-shim");

        // Extract version from locald-shim/Cargo.toml
        let shim_toml_path = shim_dir.join("Cargo.toml");
        let shim_toml_content = std::fs::read_to_string(&shim_toml_path)
            .expect("Failed to read locald-shim/Cargo.toml");

        let shim_version = shim_toml_content
            .lines()
            .find(|line| line.starts_with("version = "))
            .and_then(|line| line.split('"').nth(1))
            .expect("Failed to parse version from locald-shim/Cargo.toml");

        println!("cargo:rustc-env=LOCALD_EXPECTED_SHIM_VERSION={shim_version}");

        // We build the shim in release mode to keep it small
        let status = Command::new("cargo")
            .arg("build")
            .arg("--release")
            .arg("--manifest-path")
            .arg(shim_dir.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(out_dir.join("shim-target"))
            .status()
            .expect("Failed to build locald-shim");

        assert!(status.success(), "Failed to build locald-shim");

        // The binary will be in out_dir/shim-target/release/locald-shim
        let shim_bin = out_dir.join("shim-target/release/locald-shim");
        println!(
            "cargo:rustc-env=LOCALD_EMBEDDED_SHIM_PATH={}",
            shim_bin.display()
        );
    }

    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION not set");

    // Determine channel from features
    let channel = if env::var("CARGO_FEATURE_CHANNEL_NIGHTLY").is_ok() {
        "nightly"
    } else if env::var("CARGO_FEATURE_CHANNEL_BETA").is_ok() {
        "beta"
    } else {
        "stable"
    };
    println!("cargo:rustc-env=LOCALD_CHANNEL={channel}");

    // Generate timestamp
    let now = std::time::SystemTime::now();
    let since_the_epoch = now
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards");
    let timestamp = since_the_epoch.as_secs();

    // Build version string with channel suffix for non-stable
    // Format: 0.1.0 (stable), 0.1.0-beta, 0.1.0-nightly.1735567200
    let full_version = match channel {
        "stable" => version.clone(),
        "beta" => format!("{version}-beta"),
        "nightly" => format!("{version}-nightly.{timestamp}"),
        _ => version.clone(),
    };

    println!("cargo:rustc-env=LOCALD_BUILD_VERSION={full_version}");
}
