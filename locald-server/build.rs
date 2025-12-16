#![allow(clippy::unwrap_used)]
#![allow(missing_docs)]
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

fn main() {
    // Docs triggers
    println!("cargo:rerun-if-changed=../locald-docs/dist");
    println!("cargo:rerun-if-changed=../locald-docs/src");
    println!("cargo:rerun-if-changed=../locald-docs/astro.config.mjs");
    println!("cargo:rerun-if-changed=../locald-docs/package.json");

    // Feature gating for UI embedding.
    // (Cargo sets these env vars automatically when features are enabled.)
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_UI");

    // Dashboard triggers
    println!("cargo:rerun-if-changed=../locald-dashboard/build");
    println!("cargo:rerun-if-changed=../locald-dashboard/src");
    println!("cargo:rerun-if-changed=../locald-dashboard/svelte.config.js");
    println!("cargo:rerun-if-changed=../locald-dashboard/package.json");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = env::var("OUT_DIR").unwrap();
    let assets_root = Path::new(&out_dir).join("assets");

    let ui_enabled = env::var_os("CARGO_FEATURE_UI").is_some();

    // --- Paths ---
    let dashboard_root = Path::new(&manifest_dir).join("../locald-dashboard");
    let dashboard_dist = dashboard_root.join("build");
    let dashboard_src = dashboard_root.join("src");

    let docs_root = Path::new(&manifest_dir).join("../locald-docs");
    let docs_dist = docs_root.join("dist");
    let docs_src = docs_root.join("src");
    let docs_destination = assets_root.join("docs");

    if !ui_enabled {
        // Headless build: do not require pnpm/node, and do not mutate assets.
        // The runtime will return a helpful response when UI routes are hit.
        println!("cargo:warning=Building without embedded UI assets (feature 'ui' disabled).");
        return;
    }

    // UI build: ensure artifacts exist and are up-to-date, then refresh assets.
    ensure_frontend_artifacts(
        &dashboard_root,
        &dashboard_dist,
        &dashboard_src,
        "locald-dashboard",
    );
    ensure_frontend_artifacts(&docs_root, &docs_dist, &docs_src, "locald-docs");

    // Clean up existing generated assets to ensure we don't have stale files.
    if assets_root.exists() {
        fs::remove_dir_all(&assets_root).unwrap();
    }
    fs::create_dir_all(&assets_root).unwrap();

    // --- Copy Dashboard ---
    copy_dir_all(&dashboard_dist, &assets_root).unwrap();

    // --- Copy Docs ---
    fs::create_dir_all(&docs_destination).unwrap();
    copy_dir_all(&docs_dist, &docs_destination).unwrap();
}

fn ensure_frontend_artifacts(root: &Path, dist: &Path, src: &Path, name: &str) {
    let needs_build = !dist.exists() || is_dist_stale(root, dist, src);
    if needs_build && let Err(e) = run_pnpm_build(root, name) {
        println!("cargo:error=Failed to build {name} assets via pnpm: {e}");
        std::process::exit(1);
    }

    if !dist.exists() {
        println!(
            "cargo:error={name} build output not found at {} after build.",
            dist.display()
        );
        std::process::exit(1);
    }
}

#[allow(clippy::disallowed_methods)]
fn run_pnpm_build(project_root: &Path, name: &str) -> std::io::Result<()> {
    // Prefer pnpm since the repo uses pnpm-lock.yaml.
    let status = Command::new("pnpm")
        .current_dir(project_root)
        .args(["install", "--frozen-lockfile"])
        .status()?;
    if !status.success() {
        println!("cargo:error=Failed to run 'pnpm install' for {name}");
        std::process::exit(1);
    }

    let status = Command::new("pnpm")
        .current_dir(project_root)
        .arg("build")
        .status()?;
    if !status.success() {
        println!("cargo:error=Failed to run 'pnpm build' for {name}");
        std::process::exit(1);
    }

    Ok(())
}

fn is_dist_stale(root: &Path, dist: &Path, src: &Path) -> bool {
    let Some(dist_time) = get_newest_mtime(dist) else {
        return true;
    };

    let src_time = get_newest_mtime(src);
    let config_time = get_newest_mtime(&root.join("package.json"));
    let lock_time = get_newest_mtime(&root.join("pnpm-lock.yaml"));

    let latest_src = [src_time, config_time, lock_time]
        .into_iter()
        .flatten()
        .max();

    latest_src.is_some_and(|t| t > dist_time)
}

fn get_newest_mtime(path: &Path) -> Option<SystemTime> {
    let mut max_time = None;
    if path.is_file() {
        if let Ok(metadata) = fs::metadata(path)
            && let Ok(modified) = metadata.modified()
        {
            max_time = Some(modified);
        }
    } else if path.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            let time = get_newest_mtime(&entry.path());
            if let Some(t) = time
                && max_time.is_none_or(|max| t > max)
            {
                max_time = Some(t);
            }
        }
    }
    max_time
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}
