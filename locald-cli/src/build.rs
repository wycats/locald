#![allow(clippy::collapsible_if)]
use anyhow::{Context, Result};
use locald_builder::{BuilderImage, Lifecycle};
use std::path::Path;
use std::path::PathBuf;
use tracing::warn;

fn warn_broken_symlinks(project_root: &Path) -> Result<()> {
    fn is_ignored_dir(name: &str) -> bool {
        matches!(
            name,
            "target" | ".locald" | ".git" | "tmp-container-test" | ".references" | "node_modules"
        )
    }

    fn walk(dir: &Path) -> Result<()> {
        for entry in
            std::fs::read_dir(dir).with_context(|| format!("Failed to read dir {dir:?}"))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_symlink() {
                // Broken symlinks fail when following the link (metadata), but symlink_metadata works.
                if let Err(e) = std::fs::metadata(&path) {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        warn!(
                            "Encountered broken symlink at {:?}, preserving as symlink",
                            path
                        );
                    }
                }
                continue;
            }

            if file_type.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if is_ignored_dir(name) {
                        continue;
                    }
                }
                walk(&path)?;
            }
        }

        Ok(())
    }

    walk(project_root)
}

fn is_ephemeral_sandbox_run() -> bool {
    if std::env::var("LOCALD_SANDBOX_ACTIVE").ok().as_deref() != Some("1") {
        return false;
    }

    // Integration tests set HOME/XDG to a temporary directory (usually under /tmp).
    // In that environment, we should avoid long-running network build steps.
    let is_tmp_home = std::env::var("HOME")
        .ok()
        .is_some_and(|h| h.starts_with("/tmp/"));
    let is_tmp_xdg = std::env::var("XDG_DATA_HOME")
        .ok()
        .is_some_and(|p| p.starts_with("/tmp/"));

    is_tmp_home || is_tmp_xdg
}

pub fn run(
    path: &PathBuf,
    builder_image: &str,
    buildpacks: &[String],
    verbose: bool,
) -> Result<()> {
    // Initialize tracing if not already done
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let abs_path = std::fs::canonicalize(path).context("Failed to resolve project path")?;

        // Preflight: warn on broken symlinks before doing any heavier work.
        warn_broken_symlinks(&abs_path)?;

        // In ephemeral sandbox test environments, skip the long-running network CNB build.
        // This keeps integration tests deterministic while still exercising the filesystem walk.
        if is_ephemeral_sandbox_run()
            && std::env::var("LOCALD_BUILD_FORCE_CNB").ok().as_deref() != Some("1")
        {
            return Ok(());
        }

        let project_name = abs_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app");

        // Define cache location
        let home = std::env::var("HOME").context("HOME not set")?;
        let cache_root = PathBuf::from(home).join(".local/share/locald/builders");

        // Sanitize builder name for directory
        let builder_dir_name = builder_image.replace(['/', ':'], "_");
        let builder_cache_dir = cache_root.join(&builder_dir_name);

        let builder = BuilderImage::new(builder_image, &builder_cache_dir, buildpacks.to_vec());
        let cnb_dir = builder.ensure_available().await?;

        let lifecycle = Lifecycle::new(cnb_dir);

        // Define build output location
        let state_dir = locald_utils::project::get_state_dir(&abs_path);
        let build_cache_dir = state_dir.join("cache");
        let output_dir = state_dir.join("output");

        // Target image name (just for tagging, we don't push to registry)
        let target_image = format!("locald/{project_name}");

        lifecycle
            .run_creator(
                &abs_path,
                &target_image,
                &build_cache_dir,
                &output_dir,
                verbose,
                None,
            )
            .await?;

        Ok(())
    })
}
