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

    // Build locald-shim only when targeting Linux
    // We use CARGO_CFG_TARGET_OS to check the target (not host) OS.
    // This allows cross-compilation from non-Linux hosts.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "linux" {
        println!("cargo:rerun-if-changed=../locald-shim/src");
        println!("cargo:rerun-if-changed=../locald-shim/Cargo.toml");

        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
        let shim_dir = PathBuf::from("../locald-shim");
        let target = env::var("TARGET").expect("TARGET not set");
        let host = env::var("HOST").expect("HOST not set");
        let is_cross_compiling = target != host;

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

        // Build the shim in release mode to keep it small.
        // For cross-compilation, pass --target.
        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .arg("--release")
            .arg("--manifest-path")
            .arg(shim_dir.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(out_dir.join("shim-target"));

        if is_cross_compiling {
            cmd.arg("--target").arg(&target);
        }

        let status = cmd.status().expect("Failed to build locald-shim");
        assert!(status.success(), "Failed to build locald-shim");

        // The binary path differs based on whether we're cross-compiling
        let shim_bin = if is_cross_compiling {
            out_dir
                .join("shim-target")
                .join(&target)
                .join("release")
                .join("locald-shim")
        } else {
            out_dir.join("shim-target/release/locald-shim")
        };
        println!(
            "cargo:rustc-env=LOCALD_EMBEDDED_SHIM_PATH={}",
            shim_bin.display()
        );
    }

    // Build locald-agent when targeting macOS.
    if target_os == "macos" {
        println!("cargo:rerun-if-changed=../locald-agent/src");
        println!("cargo:rerun-if-changed=../locald-agent/Cargo.toml");

        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
        let agent_dir = PathBuf::from("../locald-agent");

        // Extract version from locald-agent/Cargo.toml
        let agent_toml_path = agent_dir.join("Cargo.toml");
        let agent_toml_content = std::fs::read_to_string(&agent_toml_path)
            .expect("Failed to read locald-agent/Cargo.toml");

        let agent_version = agent_toml_content
            .lines()
            .find(|line| line.starts_with("version = "))
            .and_then(|line| line.split('"').nth(1))
            .expect("Failed to parse version from locald-agent/Cargo.toml");

        println!("cargo:rustc-env=LOCALD_EXPECTED_AGENT_VERSION={agent_version}");

        // Build the agent in release mode.
        // Support cross-compilation (e.g. x86_64 on arm64 or vice versa).
        let target = env::var("TARGET").expect("TARGET not set");
        let host = env::var("HOST").expect("HOST not set");
        let is_cross_compiling = target != host;

        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .arg("--release")
            .arg("--manifest-path")
            .arg(agent_dir.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(out_dir.join("agent-target"));

        if is_cross_compiling {
            cmd.arg("--target").arg(&target);
        }

        let status = cmd.status().expect("Failed to build locald-agent");
        assert!(status.success(), "Failed to build locald-agent");

        let agent_bin = if is_cross_compiling {
            out_dir
                .join("agent-target")
                .join(&target)
                .join("release")
                .join("locald-agent")
        } else {
            out_dir.join("agent-target/release/locald-agent")
        };
        println!(
            "cargo:rustc-env=LOCALD_EMBEDDED_AGENT_PATH={}",
            agent_bin.display()
        );

        // Build locald-helper (privileged helper daemon) for macOS.
        println!("cargo:rerun-if-changed=../locald-helper/src");
        println!("cargo:rerun-if-changed=../locald-helper/Cargo.toml");

        let helper_dir = PathBuf::from("../locald-helper");

        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .arg("--release")
            .arg("--manifest-path")
            .arg(helper_dir.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(out_dir.join("helper-target"));

        if is_cross_compiling {
            cmd.arg("--target").arg(&target);
        }

        let status = cmd.status().expect("Failed to build locald-helper");
        assert!(status.success(), "Failed to build locald-helper");

        let helper_bin = if is_cross_compiling {
            out_dir
                .join("helper-target")
                .join(&target)
                .join("release")
                .join("locald-helper")
        } else {
            out_dir.join("helper-target/release/locald-helper")
        };

        // Ad-hoc codesign the helper binary.
        let sign_status = Command::new("codesign")
            .args(["-s", "-", "-f"])
            .arg(&helper_bin)
            .status()
            .expect("Failed to run codesign");
        assert!(sign_status.success(), "Failed to codesign locald-helper");

        println!(
            "cargo:rustc-env=LOCALD_EMBEDDED_HELPER_PATH={}",
            helper_bin.display()
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
    //
    // Note: Due to Cargo's feature unification, building with `--features channel-nightly`
    // will enable all three channel features (nightly depends on beta depends on stable).
    // The if-else chain in channel detection above handles this correctly by checking
    // nightly first, then beta, then stable.
    let full_version = match channel {
        "beta" => format!("{version}-beta"),
        "nightly" => format!("{version}-nightly.{timestamp}"),
        // "stable" and any unexpected value use the base version
        _ => version,
    };

    println!("cargo:rustc-env=LOCALD_BUILD_VERSION={full_version}");
}
