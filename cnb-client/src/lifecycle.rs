use crate::runtime::ShimRuntime;
use anyhow::Result;
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use locald_oci::{oci_layout, runtime_spec};
use serde::Deserialize;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Deserialize)]
struct RunConfig {
    images: Vec<RunImage>,
}

#[derive(Deserialize)]
struct RunImage {
    image: String,
}

pub struct ContainerConfig<'a> {
    pub rootfs: &'a Path,
    pub args: &'a [String],
    pub env: &'a [String],
    pub bind_mounts: &'a [(String, String)],
    pub verbose: bool,
    pub log_dir: Option<&'a Path>,
    pub log_callback: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
}

impl std::fmt::Debug for ContainerConfig<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerConfig")
            .field("rootfs", &self.rootfs)
            .field("args", &self.args)
            .field("env", &self.env)
            .field("bind_mounts", &self.bind_mounts)
            .field("verbose", &self.verbose)
            .field("log_dir", &self.log_dir)
            .field(
                "log_callback",
                &self.log_callback.as_ref().map(|_| "Fn(...)"),
            )
            .finish()
    }
}

#[derive(Debug)]
pub struct Lifecycle {
    cnb_dir: PathBuf,
}

impl Lifecycle {
    pub fn new(cnb_dir: impl Into<PathBuf>) -> Self {
        Self {
            cnb_dir: cnb_dir.into(),
        }
    }

    pub async fn get_run_image_env(&self, output_dir: &Path) -> Result<Vec<String>> {
        let run_toml = self.cnb_dir.join("run.toml");
        let layout_dir = output_dir.join("oci-layout");

        let run_config_content = tokio::fs::read_to_string(&run_toml).await?;
        let run_config: RunConfig = toml::from_str(&run_config_content)?;

        if let Some(run_image) = run_config.images.first() {
            let run_layout_dir = Self::get_layout_path(&layout_dir, &run_image.image);
            return oci_layout::get_image_env(&run_image.image, &run_layout_dir).await;
        }

        Ok(vec![])
    }

    pub async fn run_creator(
        &self,
        app_dir: &Path,
        target_image: &str,
        cache_dir: &Path,
        output_dir: &Path,
        verbose: bool,
        log_callback: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Result<()> {
        let run_toml = self.cnb_dir.join("run.toml");
        let platform_dir = output_dir.join("platform"); // Or temp dir?
        let layers_dir = output_dir.join("layers");
        let layout_dir = output_dir.join("oci-layout");

        // Ensure directories exist
        tokio::fs::create_dir_all(&platform_dir).await?;
        tokio::fs::create_dir_all(&layers_dir).await?;
        tokio::fs::create_dir_all(&layout_dir).await?;
        tokio::fs::create_dir_all(cache_dir).await?;

        // Parse run.toml to get run image and pull it to layout
        let run_config_content = tokio::fs::read_to_string(&run_toml).await?;
        let run_config: RunConfig = toml::from_str(&run_config_content)?;
        let mut run_image_ref_with_digest = String::new();

        if let Some(run_image) = run_config.images.first() {
            info!("Pulling run image {}...", run_image.image);

            // Construct the path where lifecycle expects the image
            // Based on error logs, it expects: layout_dir/registry/repo
            // e.g. layout_dir/index.docker.io/heroku/heroku

            let run_layout_dir = Self::get_layout_path(&layout_dir, &run_image.image);
            tokio::fs::create_dir_all(&run_layout_dir).await?;

            oci_layout::pull_image_to_layout(&run_image.image, &run_layout_dir).await?;

            // Pass original image name
            run_image_ref_with_digest.clone_from(&run_image.image);
            info!("Using run image reference: {}", run_image_ref_with_digest);
        }

        info!("Running CNB Creator...");

        // Prepare workspace (copy filtered)
        let workspace_temp = tempfile::tempdir()?;
        let workspace_path = workspace_temp.path();
        info!("Preparing workspace at {:?}...", workspace_path);

        let app_dir_buf = app_dir.to_path_buf();
        let workspace_path_buf = workspace_path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            Self::prepare_workspace(&app_dir_buf, &workspace_path_buf)
        })
        .await??;

        // Define mounts
        let bind_mounts = vec![
            (
                workspace_path.to_string_lossy().to_string(),
                "/workspace".to_string(),
            ),
            (
                layers_dir.to_string_lossy().to_string(),
                "/layers".to_string(),
            ),
            (
                platform_dir.to_string_lossy().to_string(),
                "/platform".to_string(),
            ),
            (
                cache_dir.to_string_lossy().to_string(),
                "/cache".to_string(),
            ),
            (
                layout_dir.to_string_lossy().to_string(),
                "/layout".to_string(),
            ),
        ];

        // Construct args
        let mut args = vec![
            "/cnb/lifecycle/creator".to_string(),
            "-app".to_string(),
            "/workspace".to_string(),
            "-launcher".to_string(),
            "/cnb/lifecycle/launcher".to_string(),
            "-buildpacks".to_string(),
            "/cnb/buildpacks".to_string(),
            "-order".to_string(),
            "/cnb/order.toml".to_string(),
            "-run".to_string(),
            "/cnb/run.toml".to_string(),
            "-stack".to_string(),
            "/cnb/stack.toml".to_string(),
            "-layers".to_string(),
            "/layers".to_string(),
            "-platform".to_string(),
            "/platform".to_string(),
            "-cache-dir".to_string(),
            "/cache".to_string(),
            "-layout".to_string(),
            "-layout-dir".to_string(),
            "/layout".to_string(),
            "-log-level".to_string(),
            "debug".to_string(),
            "-daemon=false".to_string(),
            "-uid=0".to_string(),
            "-gid=0".to_string(),
        ];

        if !run_image_ref_with_digest.is_empty() {
            args.push("-run-image".to_string());
            args.push(run_image_ref_with_digest);
        }

        // Env vars
        let mut env = vec![
            "CNB_PLATFORM_API=0.12".to_string(),
            "CNB_EXPERIMENTAL_MODE=warn".to_string(),
        ];

        // Load builder environment from cache if available
        let env_path = self
            .cnb_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid cnb_dir structure"))?
            .join("env");
        if env_path.exists() {
            let content = tokio::fs::read_to_string(&env_path).await?;
            let builder_env: Vec<String> = serde_json::from_str(&content)?;
            env.extend(builder_env);
        } else {
            // Fallback if no env file found (e.g. old cache)
            env.push("PATH=/cnb/lifecycle:/usr/bin:/bin".to_string());
        }

        // We are using User Namespaces to map the host user to root (0) inside the container.
        // We pass -uid=0 and -gid=0 to the lifecycle because we are running as root (0) inside the container.
        // This ensures the lifecycle operates as the container root, which maps to the host user outside.

        // Positional argument must be last
        args.push(target_image.to_string());

        // Rootfs is the parent of cnb_dir (which is cache_dir/cnb)
        let rootfs = self
            .cnb_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid cnb_dir structure"))?;

        if !rootfs.join("etc/passwd").exists() {
            return Err(anyhow::anyhow!(
                "Rootfs at {} is missing /etc/passwd. Please run scripts/setup-cnb.sh to set up the builder image correctly.",
                rootfs.display()
            ));
        }

        let log_dir = app_dir.join(".locald/crashes");
        self.run_in_container(ContainerConfig {
            rootfs,
            args: &args,
            env: &env,
            bind_mounts: &bind_mounts,
            verbose,
            log_dir: Some(&log_dir),
            log_callback,
        })
        .await?;

        info!("Build complete!");
        Ok(())
    }

    // New method to run a command inside a container using the shim
    // Implements RFC 0052: Containerized Execution Strategy
    pub async fn run_in_container(&self, config: ContainerConfig<'_>) -> Result<()> {
        // 1. Prepare Bundle (RFC 0052 Stage 2)
        // Create a temporary directory for the bundle
        let bundle_dir = tempfile::tempdir()?;
        let bundle_path = bundle_dir.path();
        let config_path = bundle_path.join("config.json");

        let uid = nix::unistd::getuid().as_raw();
        let gid = nix::unistd::getgid().as_raw();

        // Ensure rootfs path is absolute
        let rootfs_abs = config
            .rootfs
            .canonicalize()
            .unwrap_or_else(|_| config.rootfs.to_path_buf());

        // 2. Generate OCI Config (RFC 0052: "locald generates a config.json using oci-spec")
        // Use UID/GID 0 (root) inside the container because we only map host->0
        // We aliased 'cnb' to 0 in /etc/passwd so lifecycle is happy
        let spec = runtime_spec::generate_config(
            &rootfs_abs,
            config.args,
            config.env,
            config.bind_mounts,
            uid,
            gid,
            0,
            0,
            None,
            None,
        )?;

        let json_str = serde_json::to_string_pretty(&spec)?;
        tokio::fs::write(&config_path, json_str).await?;

        // 3. Execute via Shim (RFC 0098: Caller-Generates / Shim-Executes)
        let container_id = format!("cnb-task-{}", uuid::Uuid::new_v4());

        ShimRuntime::run_container(
            bundle_path,
            &container_id,
            config.verbose,
            config.log_dir,
            config.log_callback,
        )
        .await

        // 4. Cleanup
        // The bundle directory is a tempdir (auto-deleted). The shim is expected to
        // clean up its per-container state directory on exit.
    }

    #[allow(clippy::collapsible_if)]
    #[allow(clippy::useless_let_if_seq)]
    #[allow(clippy::disallowed_methods)]
    fn prepare_workspace(source: &Path, dest: &Path) -> Result<()> {
        let mut overrides = OverrideBuilder::new(source);
        overrides.add("!target/")?;
        overrides.add("!.locald/")?;
        overrides.add("!.git/")?;
        overrides.add("!tmp-container-test/")?;
        overrides.add("!.references/")?;
        overrides.add("!node_modules/")?;
        let override_matched = overrides.build()?;

        let walker = WalkBuilder::new(source)
            .overrides(override_matched)
            .hidden(false) // Include hidden files (like .env)
            .git_ignore(false) // Do not respect .gitignore (RFC 0060)
            .build();

        for result in walker {
            let entry = result?;
            let path = entry.path();

            // Skip the root itself
            if path == source {
                continue;
            }

            // Calculate relative path
            let rel_path = path.strip_prefix(source)?;
            let dest_path = dest.join(rel_path);

            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                std::fs::create_dir_all(&dest_path)?;
            } else {
                // Ensure parent dir exists
                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                // Try reflink, fall back to copy
                if reflink::reflink(path, &dest_path).is_err() {
                    if let Err(e) = std::fs::copy(path, &dest_path) {
                        // If copy fails because file not found, check if it is a broken symlink
                        // We want to preserve broken symlinks rather than failing
                        let is_symlink = entry.file_type().is_some_and(|ft| ft.is_symlink());
                        if e.kind() == std::io::ErrorKind::NotFound && is_symlink {
                            warn!(
                                "Encountered broken symlink at {:?}, preserving as symlink",
                                path
                            );
                            let target = std::fs::read_link(path)?;
                            symlink(target, &dest_path)?;
                        } else {
                            return Err(e.into());
                        }
                    }
                }
            }
        }

        // Inject scripts into package.json
        let package_json_path = dest.join("package.json");
        let mut has_dev_but_no_start = false;

        if package_json_path.exists() {
            let content = std::fs::read_to_string(&package_json_path)?;
            if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(scripts) = json.get_mut("scripts").and_then(|s| s.as_object_mut()) {
                    let mut modified = false;

                    // 1. Inject heroku-build to skip build
                    if scripts.contains_key("build")
                        || scripts.contains_key("heroku-build")
                        || scripts.contains_key("heroku-postbuild")
                    {
                        info!(
                            "Overriding build scripts in package.json to skip application build..."
                        );
                        scripts.insert(
                            "heroku-build".to_string(),
                            serde_json::Value::String(
                                "echo 'locald: Skipping build script for environment creation'"
                                    .to_string(),
                            ),
                        );
                        modified = true;
                    }

                    // Check for start/dev scripts
                    if !scripts.contains_key("start") && scripts.contains_key("dev") {
                        has_dev_but_no_start = true;
                    }

                    if modified {
                        let new_content = serde_json::to_string_pretty(&json)?;
                        std::fs::write(&package_json_path, new_content)?;
                    }
                }
            }
        }

        // Inject Procfile if missing and needed
        let procfile_path = dest.join("Procfile");
        if !procfile_path.exists() && has_dev_but_no_start {
            info!(
                "No start script or Procfile found, creating Procfile with 'web: npm run dev'..."
            );
            std::fs::write(&procfile_path, "web: npm run dev")?;
        }

        Ok(())
    }

    pub fn get_layout_path(layout_dir: &Path, image_name: &str) -> PathBuf {
        let (registry, repo) =
            image_name
                .find('/')
                .map_or(("index.docker.io", image_name), |slash_idx| {
                    let left = &image_name[..slash_idx];
                    if left.contains('.') || left.contains(':') || left == "localhost" {
                        (left, &image_name[slash_idx + 1..])
                    } else {
                        ("index.docker.io", image_name)
                    }
                });

        let registry = if registry == "docker.io" {
            "index.docker.io"
        } else {
            registry
        };

        let (repo_path, tag) = repo
            .rfind(':')
            .map_or((repo, "latest"), |idx| (&repo[..idx], &repo[idx + 1..]));

        layout_dir.join(registry).join(repo_path).join(tag)
    }
}
