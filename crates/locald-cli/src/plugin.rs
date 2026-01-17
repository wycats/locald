use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use locald_core::plugin::manifest::PackageManifest;
use locald_server::plugins::{HostCapabilities, PluginRunner, ServiceSpec, WorkspaceContext};

/// Current locald version for compatibility checking.
const LOCALD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Supported IR versions by this host.
const SUPPORTED_IR_VERSIONS: &[i32] = &[1];

/// Install a plugin from a local path or URL.
///
/// - If `project` is true, installs into the current project's `.locald/plugins` directory.
/// - If `user` is true, installs into `$XDG_DATA_HOME/locald/plugins/`.
/// - If neither, defaults to project scope.
/// - Auto-detects `.locald-package` archives vs raw `.wasm` files.
/// - If `force` is true, overwrites existing plugins.
pub fn install(
    source: &str,
    name: Option<String>,
    _project: bool,
    user: bool,
    force: bool,
) -> Result<()> {
    // Determine destination directory
    let dest_dir = if user {
        user_plugins_dir()
    } else {
        // Default to project scope (even if --project not specified)
        project_plugins_dir()?
    };
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("failed to create plugin dir {}", dest_dir.display()))?;

    let source = source.trim();

    // Detect package format
    let is_package = is_package_source(source);

    if is_package {
        install_package(source, &dest_dir, force)
    } else {
        install_raw_wasm(source, &dest_dir, name, force)
    }
}

/// Check if source looks like a .locald-package
fn is_package_source(source: &str) -> bool {
    source.ends_with(".locald-package")
}

/// Install a raw .wasm file
fn install_raw_wasm(
    source: &str,
    dest_dir: &Path,
    name: Option<String>,
    force: bool,
) -> Result<()> {
    if let Some(path) = file_url_to_path(source) {
        return install_wasm_from_path(&path, dest_dir, name, force);
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        return install_wasm_from_url(source, dest_dir, name, force);
    }

    install_wasm_from_path(Path::new(source), dest_dir, name, force)
}

/// Install a .locald-package archive
fn install_package(source: &str, dest_dir: &Path, force: bool) -> Result<()> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tar::Archive;

    println!("→ Extracting package...");

    // Get package bytes
    let package_bytes = if let Some(path) = file_url_to_path(source) {
        std::fs::read(&path)
            .with_context(|| format!("Error: Package not found: '{}'", path.display()))?
    } else if source.starts_with("http://") || source.starts_with("https://") {
        // Security warning for remote URLs
        eprintln!("⚠ Warning: Installing unsigned package from remote URL.");
        eprintln!("  Packages can request capabilities that affect your system.");
        eprintln!("  Only install packages from sources you trust.");

        // For now, continue without prompt (would need dialoguer for interactive)
        let response = reqwest::blocking::get(source)
            .with_context(|| format!("failed to download {source}"))?
            .error_for_status()
            .with_context(|| format!("download failed for {source}"))?;
        response.bytes()?.to_vec()
    } else {
        let path = Path::new(source);
        std::fs::read(path)
            .with_context(|| format!("Error: Package not found: '{}'", path.display()))?
    };

    // Create temp directory for extraction
    let temp_dir = dest_dir.join(".tmp-install");
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }
    std::fs::create_dir_all(&temp_dir)?;

    // Extract archive
    let decoder = GzDecoder::new(package_bytes.as_slice());
    let mut archive = Archive::new(decoder);

    // Track extracted files
    let mut manifest_content: Option<String> = None;
    let mut component_path: Option<PathBuf> = None;
    let mut asset_files: Vec<PathBuf> = Vec::new();

    for entry in archive
        .entries()
        .context("Error: Invalid .locald-package archive")?
    {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();

        // Security: validate no path traversal
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                std::fs::remove_dir_all(&temp_dir).ok();
                anyhow::bail!("Error: Archive contains path traversal attack");
            }
        }

        let dest = temp_dir.join(&path);

        // Create parent directories
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Read content
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;

        // Track special files
        if path.as_os_str() == "manifest.toml" {
            manifest_content = Some(
                String::from_utf8(content.clone()).context("manifest.toml is not valid UTF-8")?,
            );
        } else if path.extension().map(|e| e == "wasm").unwrap_or(false) {
            component_path = Some(dest.clone());
        } else if path.starts_with("assets/") {
            asset_files.push(path.clone());
        }

        std::fs::write(&dest, content)?;
    }

    // === Phase 2: Manifest Validation ===
    let manifest_content =
        manifest_content.ok_or_else(|| anyhow::anyhow!("Error: Package missing manifest.toml"))?;

    let manifest = PackageManifest::parse(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Error: Invalid manifest: {e}"))?;

    // === Phase 3: Compatibility Checking ===
    println!("→ Checking compatibility...");

    // Check locald version
    if let Some(ref min_version) = manifest.compatibility.locald_min {
        let current: semver::Version = LOCALD_VERSION
            .parse()
            .unwrap_or_else(|_| semver::Version::new(0, 1, 0));
        let required: semver::Version = min_version
            .parse()
            .context("Error: Invalid locald_min version in manifest")?;

        if current >= required {
            println!(
                "   ✓ locald version {} meets minimum {}",
                LOCALD_VERSION, min_version
            );
        } else {
            std::fs::remove_dir_all(&temp_dir).ok();
            anyhow::bail!(
                "Error: Package requires locald >= {}, current is {}",
                min_version,
                LOCALD_VERSION
            );
        }
    }

    // Check IR version
    let ir_version = manifest.compatibility.ir_version;
    if SUPPORTED_IR_VERSIONS.contains(&ir_version) {
        println!("   ✓ IR version {} supported", ir_version);
    } else {
        std::fs::remove_dir_all(&temp_dir).ok();
        anyhow::bail!(
            "Error: Package uses IR version {}, supported: {:?}",
            ir_version,
            SUPPORTED_IR_VERSIONS
        );
    }

    // === Phase 4: Capability Warning ===
    for cap in &manifest.capabilities.required {
        println!(
            "   ⚠ Requires capability: {} (will be requested at runtime)",
            cap
        );
    }

    // === Phase 5: Installation ===
    let final_wasm = dest_dir.join(format!("{}.wasm", manifest.package.name));

    // Check if already exists
    if final_wasm.exists() && !force {
        std::fs::remove_dir_all(&temp_dir).ok();
        anyhow::bail!(
            "Error: Plugin '{}' already installed (use --force to overwrite)",
            manifest.package.name
        );
    }

    // Copy component
    if let Some(ref comp_path) = component_path {
        println!("→ Installing to {}", final_wasm.display());
        std::fs::copy(comp_path, &final_wasm)
            .with_context(|| format!("failed to copy component to {}", final_wasm.display()))?;
    } else {
        std::fs::remove_dir_all(&temp_dir).ok();
        anyhow::bail!("Error: Package missing WASM component");
    }

    // Copy assets if present
    if !asset_files.is_empty() {
        let assets_dest = dest_dir.join("assets").join(&manifest.package.name);
        std::fs::create_dir_all(&assets_dest)?;

        for asset_rel in &asset_files {
            // asset_rel is like "assets/templates/foo.txt", we want just "templates/foo.txt"
            let inner_path = asset_rel.strip_prefix("assets/").unwrap_or(asset_rel);
            let src = temp_dir.join(asset_rel);
            let dst = assets_dest.join(inner_path);

            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)?;
        }
        println!("   ✓ Installed {} asset files", asset_files.len());
    }

    // Cleanup
    std::fs::remove_dir_all(&temp_dir).ok();

    println!("→ Done.");
    Ok(())
}

fn install_wasm_from_path(
    source: &Path,
    dest_dir: &Path,
    name: Option<String>,
    force: bool,
) -> Result<()> {
    if !source.exists() {
        anyhow::bail!("source path does not exist: {}", source.display());
    }

    let filename = if let Some(name) = name {
        sanitize_filename(&name)
    } else {
        source
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .context("could not determine filename from source path; pass --name")?
    };

    let dest = dest_dir.join(&filename);

    if dest.exists() && !force {
        anyhow::bail!(
            "Plugin '{}' already installed (use --force to overwrite)",
            filename
        );
    }

    std::fs::copy(source, &dest)
        .with_context(|| format!("failed to copy {} -> {}", source.display(), dest.display()))?;

    println!("installed {}", dest.display());
    Ok(())
}

fn install_wasm_from_url(
    url: &str,
    dest_dir: &Path,
    name: Option<String>,
    force: bool,
) -> Result<()> {
    let filename = if let Some(name) = name {
        sanitize_filename(&name)
    } else {
        url_filename(url).context("could not determine filename from URL; pass --name")?
    };

    let dest = dest_dir.join(&filename);

    if dest.exists() && !force {
        anyhow::bail!(
            "Plugin '{}' already installed (use --force to overwrite)",
            filename
        );
    }

    let response = reqwest::blocking::get(url)
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed for {url}"))?;

    let bytes = response
        .bytes()
        .with_context(|| format!("failed reading response body from {url}"))?;

    std::fs::write(&dest, &bytes).with_context(|| format!("failed to write {}", dest.display()))?;

    println!("installed {}", dest.display());
    Ok(())
}

pub fn inspect(
    plugin: &str,
    kind: &str,
    name: Option<&str>,
    depends_on: Option<&str>,
    config: &[String],
    grant: &[String],
) -> Result<()> {
    let component_path = resolve_plugin(plugin)?;

    let (workspace_id, root) = detect_workspace_identity()?;

    let runner = PluginRunner::new()?;
    let ctx = WorkspaceContext { workspace_id, root };

    let caps = HostCapabilities {
        supported_ir_versions: vec![1],
        granted: grant.to_vec(),
    };

    let spec = ServiceSpec {
        name: name.unwrap_or(kind).to_string(),
        kind: kind.to_string(),
        depends_on: parse_depends_on(depends_on),
        config: parse_config(config)?,
    };

    let detect = runner
        .detect(&component_path, ctx.clone(), spec.clone())
        .context("plugin.detect failed")?;

    println!("detect: {}", detect.as_deref().unwrap_or("(none)"));

    let plan_result = runner
        .apply(&component_path, ctx, caps.clone(), spec)
        .context("plugin.apply failed")?;

    match plan_result {
        Ok(plan) => {
            locald_server::plugins::validate_plan(&plan, &caps).map_err(|d| {
                anyhow::anyhow!(
                    "plan validation failed: warnings={:?} errors={:?}",
                    d.warnings,
                    d.errors
                )
            })?;
            let json = locald_server::plugins::normalized_plan_debug_json(&plan);
            println!("{}", serde_json::to_string_pretty(&json)?);
            Ok(())
        }
        Err(diag) => {
            eprintln!("plugin diagnostics:");
            for w in diag.warnings {
                eprintln!("warning: {w}");
            }
            for e in diag.errors {
                eprintln!("error: {e}");
            }
            anyhow::bail!("plugin returned diagnostics")
        }
    }
}

/// Validate the plan produced by a plugin.
///
/// Exits with an error if the plugin returns diagnostics or if the produced plan is invalid.
pub fn validate(
    plugin: &str,
    kind: &str,
    name: Option<&str>,
    depends_on: Option<&str>,
    config: &[String],
    grant: &[String],
) -> Result<()> {
    let component_path = resolve_plugin(plugin)?;

    let (workspace_id, root) = detect_workspace_identity()?;

    let runner = PluginRunner::new()?;
    let ctx = WorkspaceContext { workspace_id, root };

    let caps = HostCapabilities {
        supported_ir_versions: vec![1],
        granted: grant.to_vec(),
    };

    let spec = ServiceSpec {
        name: name.unwrap_or(kind).to_string(),
        kind: kind.to_string(),
        depends_on: parse_depends_on(depends_on),
        config: parse_config(config)?,
    };

    let plan_result = runner
        .apply(&component_path, ctx, caps.clone(), spec)
        .context("plugin.apply failed")?;

    match plan_result {
        Ok(plan) => {
            locald_server::plugins::validate_plan(&plan, &caps).map_err(|d| {
                anyhow::anyhow!(
                    "plan validation failed: warnings={:?} errors={:?}",
                    d.warnings,
                    d.errors
                )
            })?;
            println!("ok");
            Ok(())
        }
        Err(diag) => {
            for w in diag.warnings {
                eprintln!("warning: {w}");
            }
            for e in diag.errors {
                eprintln!("error: {e}");
            }
            anyhow::bail!("plugin returned diagnostics")
        }
    }
}

fn user_plugins_dir() -> PathBuf {
    locald_utils::env::get_xdg_data_home().join("plugins")
}

fn project_plugins_dir() -> Result<PathBuf> {
    Ok(std::env::current_dir()?.join(".locald").join("plugins"))
}

fn resolve_plugin(plugin: &str) -> Result<PathBuf> {
    let plugin = plugin.trim();

    let direct = Path::new(plugin);
    if direct.exists() {
        return Ok(direct.to_path_buf());
    }

    let candidates = plugin_name_candidates(plugin);

    let dirs = vec![project_plugins_dir()?, user_plugins_dir()];

    for dir in dirs {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.exists() {
                return Ok(path);
            }
        }
    }

    anyhow::bail!("plugin '{plugin}' not found (checked explicit path and plugin dirs)")
}

fn plugin_name_candidates(name: &str) -> Vec<String> {
    let name = name.trim();
    if name.is_empty() {
        return vec![];
    }

    // If it already looks like a filename, try it as-is too.
    let mut out = vec![name.to_string()];

    if !name.contains('.') {
        out.push(format!("{name}.wasm"));
        out.push(format!("{name}.component.wasm"));
    }

    out
}

fn file_url_to_path(source: &str) -> Option<PathBuf> {
    if let Some(rest) = source.strip_prefix("file://") {
        return Some(PathBuf::from(rest));
    }
    None
}

/// Create a distributable plugin package (.locald-package).
///
/// Bundles a manifest.toml, WASM component, and optional assets into a gzip-compressed tar archive.
pub fn create(
    source: &Path,
    output: Option<&Path>,
    manifest_name: Option<&Path>,
    dry_run: bool,
    force: bool,
    verbose: bool,
) -> Result<()> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::fs::File;
    use std::io::Read;
    use tar::Builder;

    // === Phase 1: Manifest Validation ===
    let source = source
        .canonicalize()
        .with_context(|| format!("Error: Source directory '{}' not found", source.display()))?;

    let manifest_path = source.join(manifest_name.unwrap_or(Path::new("manifest.toml")));
    let manifest_content = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("Error: manifest.toml not found in '{}'", source.display()))?;

    let manifest = PackageManifest::parse(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Error: Invalid manifest: {e}"))?;

    println!(
        "✓ Validated manifest ({} v{})",
        manifest.package.name, manifest.package.version
    );

    // === Phase 2: WASM Verification ===
    let component_path = source.join(&manifest.plugin.component);
    if !component_path.exists() {
        anyhow::bail!(
            "Error: Plugin component '{}' specified in manifest not found",
            manifest.plugin.component
        );
    }

    let component_bytes = std::fs::read(&component_path)
        .with_context(|| format!("failed to read {}", component_path.display()))?;

    // Check WASM magic bytes: \0asm = [0x00, 0x61, 0x73, 0x6D]
    const WASM_MAGIC: &[u8] = &[0x00, 0x61, 0x73, 0x6D];
    if component_bytes.len() < 4 || &component_bytes[0..4] != WASM_MAGIC {
        anyhow::bail!(
            "Error: '{}' is not a valid WASM file (invalid magic bytes)",
            manifest.plugin.component
        );
    }

    let component_size = component_bytes.len();
    println!(
        "✓ Verified WASM component ({}, {} KB)",
        manifest.plugin.component,
        component_size / 1024
    );

    // === Phase 3: Asset Collection ===
    let assets_dir = source.join("assets");
    let mut asset_files: Vec<PathBuf> = Vec::new();
    let mut assets_size: usize = 0;

    if assets_dir.exists() && assets_dir.is_dir() {
        collect_assets(&assets_dir, &assets_dir, &mut asset_files)?;
        for asset_path in &asset_files {
            let full_path = assets_dir.join(asset_path);
            assets_size += std::fs::metadata(&full_path)
                .map(|m| m.len() as usize)
                .unwrap_or(0);
        }
        println!(
            "✓ Collected {} asset files ({} KB)",
            asset_files.len(),
            assets_size / 1024
        );
    }

    // Determine output path
    let output_path = output.map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}-{}.locald-package",
            manifest.package.name, manifest.package.version
        ))
    });

    // === Dry Run: Just print what would happen ===
    if dry_run {
        println!("\nWould create package from {}", source.display());
        println!(
            "\n  Manifest: {} v{}",
            manifest.package.name, manifest.package.version
        );
        println!(
            "  Component: {} ({} KB)",
            manifest.plugin.component,
            component_size / 1024
        );
        if !asset_files.is_empty() {
            println!(
                "  Assets: {} files ({} KB)",
                asset_files.len(),
                assets_size / 1024
            );
        }
        println!("\n  Would write: {}", output_path.display());
        return Ok(());
    }

    // Check if output exists
    if output_path.exists() && !force {
        anyhow::bail!(
            "Error: Output file already exists: {} (use --force to overwrite)",
            output_path.display()
        );
    }

    // === Phase 4: Archive Creation ===
    // Create temp file in same directory for atomic write
    let temp_path = output_path.with_extension("locald-package.tmp");

    {
        let file = File::create(&temp_path).with_context(|| {
            format!(
                "Error: Cannot write to output path: {}",
                temp_path.display()
            )
        })?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = Builder::new(encoder);

        // Add manifest.toml
        let mut header = tar::Header::new_gnu();
        let manifest_bytes = manifest_content.as_bytes();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "manifest.toml", manifest_bytes)
            .context("failed to add manifest.toml to archive")?;
        if verbose {
            println!("  + manifest.toml");
        }

        // Add component
        let mut header = tar::Header::new_gnu();
        header.set_size(component_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                &manifest.plugin.component,
                component_bytes.as_slice(),
            )
            .context("failed to add component to archive")?;
        if verbose {
            println!("  + {}", manifest.plugin.component);
        }

        // Add assets
        for asset_rel in &asset_files {
            let full_path = assets_dir.join(asset_rel);
            let archive_path = Path::new("assets").join(asset_rel);

            let mut file = File::open(&full_path)
                .with_context(|| format!("failed to open asset {}", full_path.display()))?;
            let mut contents = Vec::new();
            file.read_to_end(&mut contents)?;

            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, &archive_path, contents.as_slice())
                .with_context(|| format!("failed to add {} to archive", archive_path.display()))?;
            if verbose {
                println!("  + {}", archive_path.display());
            }
        }

        // Finish archive
        let encoder = archive.into_inner().context("failed to finalize archive")?;
        encoder.finish().context("failed to compress archive")?;
    }

    // Atomic rename
    std::fs::rename(&temp_path, &output_path).with_context(|| {
        format!(
            "failed to rename {} to {}",
            temp_path.display(),
            output_path.display()
        )
    })?;

    // Get final size
    let final_size = std::fs::metadata(&output_path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    println!("✓ Created archive (compressed: {} KB)", final_size / 1024);
    println!("\n→ Package created: {}", output_path.display());
    println!("\n  Install with:");
    println!("    locald plugin install {}", output_path.display());

    Ok(())
}

/// Recursively collect asset files, validating no path traversal.
fn collect_assets(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read assets directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        // Security: reject any path containing ".."
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                anyhow::bail!(
                    "Error: Asset path contains '..' which is not allowed: {}",
                    path.display()
                );
            }
        }

        if path.is_dir() {
            collect_assets(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .with_context(|| format!("failed to get relative path for {}", path.display()))?;
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

fn url_filename(url: &str) -> Option<String> {
    // Minimal extraction: last path segment, ignoring query/fragment.
    let without_frag = url.split('#').next()?;
    let without_query = without_frag.split('?').next()?;
    let last = without_query.rsplit('/').next()?;
    let last = last.trim();
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "plugin.wasm".to_string();
    }

    // Avoid path separators and NUL.
    let replaced = trimmed.replace(['/', '\\', '\u{0000}'], "-");

    // Avoid "." / ".." and hidden-file style names.
    let candidate = replaced
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();

    if candidate.is_empty() || candidate == "." || candidate == ".." {
        return "plugin.wasm".to_string();
    }

    // Keep a conservative set of filename characters; normalize the rest.
    candidate
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn parse_depends_on(depends_on: Option<&str>) -> Vec<String> {
    let Some(s) = depends_on else {
        return Vec::new();
    };

    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_config(config: &[String]) -> Result<Vec<(String, locald_server::plugins::runner::Value)>> {
    let mut out = Vec::new();

    for item in config {
        let (k, v) = item
            .split_once('=')
            .with_context(|| format!("invalid --config '{item}' (expected key=value)"))?;

        let key = k.trim();
        if key.is_empty() {
            anyhow::bail!("invalid --config '{item}': empty key");
        }

        let value_str = v.trim();
        let value = if value_str.eq_ignore_ascii_case("true") {
            locald_server::plugins::runner::Value::Boolean(true)
        } else if value_str.eq_ignore_ascii_case("false") {
            locald_server::plugins::runner::Value::Boolean(false)
        } else if let Ok(n) = value_str.parse::<i64>() {
            locald_server::plugins::runner::Value::Signed(n)
        } else if let Ok(n) = value_str.parse::<u64>() {
            locald_server::plugins::runner::Value::Unsigned(n)
        } else if let Ok(n) = value_str.parse::<f64>() {
            locald_server::plugins::runner::Value::Float(n)
        } else {
            locald_server::plugins::runner::Value::Text(value_str.to_string())
        };

        out.push((key.to_string(), value));
    }

    Ok(out)
}

fn detect_workspace_identity() -> Result<(String, String)> {
    let root = std::env::current_dir()?;
    let root_str = root.to_string_lossy().to_string();

    // Best-effort: use locald.toml project name if present.
    let workspace_id = root
        .join("locald.toml")
        .exists()
        .then(|| std::fs::read_to_string(root.join("locald.toml")))
        .transpose()?
        .and_then(|content| toml::from_str::<locald_core::config::LocaldConfig>(&content).ok())
        .map(|c| c.project.name)
        .or_else(|| {
            root.file_name()
                .and_then(|s| s.to_str())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "workspace".to_string());

    Ok((workspace_id, root_str))
}

#[cfg(test)]
mod tests {
    use super::{sanitize_filename, url_filename};

    #[test]
    fn url_filename_strips_query_and_fragment() {
        assert_eq!(
            url_filename("https://example.test/path/plugin.component.wasm?x=y#frag").as_deref(),
            Some("plugin.component.wasm")
        );
    }

    #[test]
    fn sanitize_filename_rejects_dot_and_dotdot() {
        assert_eq!(sanitize_filename("."), "plugin.wasm");
        assert_eq!(sanitize_filename(".."), "plugin.wasm");
    }

    #[test]
    fn sanitize_filename_normalizes_path_separators_and_weird_chars() {
        assert_eq!(sanitize_filename("foo/bar.wasm"), "foo-bar.wasm");
        assert_eq!(sanitize_filename("  .secret\\name?  "), "secret-name-");
    }
}
