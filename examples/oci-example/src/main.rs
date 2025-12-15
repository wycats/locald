use anyhow::Result;
use clap::Parser;
use locald_oci::oci_layout::{pull_image_to_layout, unpack_image_from_layout};
use locald_oci::runtime_spec;
use std::path::PathBuf;
use tokio::fs;

#[cfg(unix)]
fn is_setuid_root(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let is_root = metadata.uid() == 0;
    let is_setuid = (metadata.mode() & 0o4000) != 0;
    is_root && is_setuid
}

fn find_privileged_shim() -> Option<PathBuf> {
    // 1) Prefer a sibling shim (supports `sudo target/debug/locald admin setup` in dev).
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        let candidate = exe_dir.join("locald-shim");
        #[cfg(unix)]
        {
            if is_setuid_root(&candidate) {
                return Some(candidate);
            }
        }
    }

    // 2) Fall back to PATH (supports installed `locald` / `locald-shim`).
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("locald-shim");
            #[cfg(unix)]
            {
                if is_setuid_root(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

#[derive(Parser)]
struct Args {
    /// Image to run (e.g., alpine:latest)
    #[arg(default_value = "alpine:latest")]
    image: String,
    /// Command to run inside the container
    #[arg(trailing_var_arg = true)]
    command: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // 1. Setup Workspace
    let workspace = PathBuf::from("tmp-oci-example");
    let layout_dir = std::env::var_os("LOCALD_OCI_LAYOUT_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("layout"));
    let bundle_dir = workspace.join("bundle");
    let rootfs = bundle_dir.join("rootfs");

    if workspace.exists() {
        // Clean up previous run (best effort)
        let _ = fs::remove_dir_all(&workspace).await;
    }
    fs::create_dir_all(&layout_dir).await?;
    fs::create_dir_all(&rootfs).await?;

    // 2. Pull Image
    println!("Pulling {}...", args.image);
    pull_image_to_layout(&args.image, &layout_dir).await?;

    // 3. Unpack Rootfs
    println!("Unpacking rootfs...");
    unpack_image_from_layout(&args.image, &layout_dir, &rootfs).await?;

    // 4. Generate Spec
    println!("Generating config.json...");

    // Get current UID/GID for rootless mapping
    let output = tokio::process::Command::new("id")
        .arg("-u")
        .output()
        .await?;
    let uid = String::from_utf8(output.stdout)?.trim().parse::<u32>()?;
    let output = tokio::process::Command::new("id")
        .arg("-g")
        .output()
        .await?;
    let gid = String::from_utf8(output.stdout)?.trim().parse::<u32>()?;

    // Use the high-level generate_config from locald-oci
    // This handles namespaces, mounts, and user mapping for us
    let default_cmd = vec!["/bin/sh".to_string()];
    let cmd = if args.command.is_empty() {
        &default_cmd
    } else {
        &args.command
    };

    let spec = runtime_spec::generate_config(
        std::path::Path::new("rootfs"),
        cmd,
        &["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()],
        &[], // No extra mounts
        uid,
        gid,
        0,    // container_uid (root inside)
        0,    // container_gid (root inside)
        None, // No working dir override
        None, // No cgroup path
    )?;

    // Write spec
    let config_json = serde_json::to_string_pretty(&spec)?;
    fs::write(bundle_dir.join("config.json"), config_json).await?;

    // 5. Run with Shim (Libcontainer)
    println!("Running container...");

    let shim_path = find_privileged_shim().ok_or_else(|| {
        anyhow::anyhow!(
            "Privileged locald-shim not found (setuid root). Run `sudo locald admin setup` (installed) or `sudo target/debug/locald admin setup` (repo dev) to install/repair it."
        )
    })?;

    let id = format!("oci-example-{}", std::process::id());
    let status = tokio::process::Command::new(&shim_path)
        .arg("bundle")
        .arg("run")
        .arg("--bundle")
        .arg(&bundle_dir)
        .arg("--id")
        .arg(&id)
        .status()
        .await?;

    if !status.success() {
        anyhow::bail!("Container exited with error");
    }

    Ok(())
}
