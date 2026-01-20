//! Distribution handling for locald.
//!
//! This module implements the distribution system specified in RFC 0129, section 3.14.
//! Distributions are project bootstrap kits that bundle plugins and project configuration.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use locald_core::plugin::distribution::{
    DistributionManifest, render_template, validate_template_syntax,
};

/// Current locald version for compatibility checking.
const LOCALD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Create a distributable distribution archive (.locald-distribution).
///
/// Bundles a distribution.toml, locald.toml, optional packages, and scaffold files
/// into a gzip-compressed tar archive.
pub fn create(
    source: &Path,
    output: Option<&Path>,
    manifest_name: Option<&Path>,
    include_remote: bool,
    dry_run: bool,
    force: bool,
    verbose: bool,
) -> Result<()> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::fs::File;
    use tar::Builder;

    // === Phase 1: Manifest Validation ===
    let source = source
        .canonicalize()
        .with_context(|| format!("Error: Source directory '{}' not found", source.display()))?;

    let manifest_path = source.join(manifest_name.unwrap_or(Path::new("distribution.toml")));
    let manifest_content = std::fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "Error: distribution.toml not found in '{}'",
            source.display()
        )
    })?;

    let manifest = DistributionManifest::parse(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Error: Invalid manifest: {e}"))?;

    println!(
        "✓ Validated manifest ({} v{})",
        manifest.distribution.name, manifest.distribution.version
    );

    // === Phase 2: locald.toml Validation ===
    let locald_toml_path = source.join("locald.toml");
    let locald_toml_content = std::fs::read_to_string(&locald_toml_path)
        .with_context(|| format!("Error: locald.toml not found in '{}'", source.display()))?;

    // Validate template syntax in locald.toml
    validate_template_syntax(&locald_toml_content, "locald.toml")
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("✓ Validated locald.toml template");

    // === Phase 3: Plugin Resolution ===
    let packages_dir = source.join("packages");
    let mut bundled_sizes: usize = 0;

    for bundled in &manifest.plugins.bundled {
        let pkg_path = packages_dir.join(bundled);
        if !pkg_path.exists() {
            anyhow::bail!(
                "Error: Bundled plugin not found: {} (expected at {})",
                bundled,
                pkg_path.display()
            );
        }
        bundled_sizes += std::fs::metadata(&pkg_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);
    }

    if !manifest.plugins.bundled.is_empty() {
        println!(
            "✓ Found {} bundled plugins ({} KB)",
            manifest.plugins.bundled.len(),
            bundled_sizes / 1024
        );
    }

    // Validate remote plugin URLs
    for remote in &manifest.plugins.remote {
        let url = remote.url();
        // Basic URL validation was done in manifest parsing
        if verbose {
            println!("   Remote: {}", url);
        }
    }

    if !manifest.plugins.remote.is_empty() {
        println!(
            "✓ Validated {} remote plugin URLs",
            manifest.plugins.remote.len()
        );
    }

    // Optionally fetch and bundle remote plugins
    let mut fetched_remotes: Vec<(String, Vec<u8>)> = Vec::new();
    if include_remote && !manifest.plugins.remote.is_empty() {
        println!("→ Fetching remote plugins...");
        for remote in &manifest.plugins.remote {
            let url = remote.url();
            let filename = url_filename(url)
                .ok_or_else(|| anyhow::anyhow!("Cannot determine filename from URL: {}", url))?;

            println!("   ⚡ Fetching {}...", filename);
            let response = reqwest::blocking::get(url)
                .with_context(|| format!("Failed to fetch {}", url))?
                .error_for_status()
                .with_context(|| format!("Download failed for {}", url))?;
            let bytes = response.bytes()?.to_vec();

            // Verify checksum if provided
            if let Some(expected) = remote.checksum() {
                let actual = sha256_hex(&bytes);
                if actual != expected {
                    anyhow::bail!(
                        "Checksum mismatch for {}: expected {}, got {}",
                        url,
                        expected,
                        actual
                    );
                }
                println!("   ✓ Verified checksum for {}", filename);
            }

            fetched_remotes.push((filename, bytes));
        }
    }

    // === Phase 4: Scaffold Validation ===
    let scaffold_dir = source.join("scaffold");

    for template in &manifest.scaffold.templates {
        let template_path = scaffold_dir.join(template);
        if !template_path.exists() {
            anyhow::bail!(
                "Error: Template file not found: {} (expected at {})",
                template,
                template_path.display()
            );
        }
        // Validate template syntax
        let content = std::fs::read_to_string(&template_path)
            .with_context(|| format!("Failed to read {}", template_path.display()))?;
        validate_template_syntax(&content, template).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    for file in &manifest.scaffold.files {
        let file_path = scaffold_dir.join(file);
        if !file_path.exists() {
            anyhow::bail!(
                "Error: Scaffold file not found: {} (expected at {})",
                file,
                file_path.display()
            );
        }
    }

    let total_scaffold = manifest.scaffold.templates.len() + manifest.scaffold.files.len();
    if total_scaffold > 0 {
        println!("✓ Collected {} scaffold files", total_scaffold);
    }

    // Determine output path
    let output_path = output
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(manifest.default_filename()));

    // === Dry Run ===
    if dry_run {
        println!("\nWould create distribution from {}", source.display());
        println!(
            "\n  Distribution: {} v{}",
            manifest.distribution.name, manifest.distribution.version
        );
        if !manifest.plugins.bundled.is_empty() {
            println!(
                "  Bundled plugins: {} ({} KB)",
                manifest.plugins.bundled.len(),
                bundled_sizes / 1024
            );
        }
        if !manifest.plugins.remote.is_empty() {
            println!("  Remote plugins: {}", manifest.plugins.remote.len());
        }
        if total_scaffold > 0 {
            println!("  Scaffold files: {}", total_scaffold);
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

    // === Phase 5: Archive Creation ===
    let temp_path = output_path.with_extension("locald-distribution.tmp");

    {
        let file = File::create(&temp_path).with_context(|| {
            format!(
                "Error: Cannot write to output path: {}",
                temp_path.display()
            )
        })?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = Builder::new(encoder);

        // Add distribution.toml
        add_file_to_archive(
            &mut archive,
            "distribution.toml",
            manifest_content.as_bytes(),
            verbose,
        )?;

        // Add locald.toml
        add_file_to_archive(
            &mut archive,
            "locald.toml",
            locald_toml_content.as_bytes(),
            verbose,
        )?;

        // Add bundled packages
        for bundled in &manifest.plugins.bundled {
            let pkg_path = packages_dir.join(bundled);
            let content = std::fs::read(&pkg_path)?;
            let archive_path = format!("packages/{}", bundled);
            add_file_to_archive(&mut archive, &archive_path, &content, verbose)?;
        }

        // Add fetched remote packages (if --include-remote)
        for (filename, content) in &fetched_remotes {
            let archive_path = format!("packages/{}", filename);
            add_file_to_archive(&mut archive, &archive_path, content, verbose)?;
        }

        // Add scaffold templates
        for template in &manifest.scaffold.templates {
            let template_path = scaffold_dir.join(template);
            let content = std::fs::read(&template_path)?;
            let archive_path = format!("scaffold/{}", template);
            add_file_to_archive(&mut archive, &archive_path, &content, verbose)?;
        }

        // Add scaffold files
        for file in &manifest.scaffold.files {
            let file_path = scaffold_dir.join(file);
            let content = std::fs::read(&file_path)?;
            let archive_path = format!("scaffold/{}", file);
            add_file_to_archive(&mut archive, &archive_path, &content, verbose)?;
        }

        // Finish archive
        let encoder = archive.into_inner().context("Failed to finalize archive")?;
        encoder.finish().context("Failed to compress archive")?;
    }

    // Atomic rename
    std::fs::rename(&temp_path, &output_path).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            temp_path.display(),
            output_path.display()
        )
    })?;

    let final_size = std::fs::metadata(&output_path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    println!("✓ Created archive (compressed: {} KB)", final_size / 1024);
    println!("\n→ Distribution created: {}", output_path.display());
    println!("\n  Initialize with:");
    println!(
        "    locald init --from-distribution {}",
        output_path.display()
    );

    Ok(())
}

/// Initialize a project from a distribution.
pub fn init_from_distribution(
    source: &str,
    name: Option<&str>,
    target: Option<&Path>,
    no_scaffold: bool,
    offline: bool,
    yes: bool,
    verbose: bool,
) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    // === Phase 1: Distribution Extraction ===
    println!("→ Loading distribution...");

    let dist_bytes = if source.starts_with("http://") || source.starts_with("https://") {
        // Download from URL
        if verbose {
            println!("   Downloading from {}...", source);
        }
        let response = reqwest::blocking::get(source)
            .with_context(|| format!("Error: Distribution not found: '{}'", source))?
            .error_for_status()
            .with_context(|| format!("Download failed: {}", source))?;
        response.bytes()?.to_vec()
    } else {
        // Local file
        let path = Path::new(source);
        std::fs::read(path)
            .with_context(|| format!("Error: Distribution not found: '{}'", source))?
    };

    // Create temp directory for extraction
    let temp_dir = std::env::temp_dir().join(format!("locald-dist-{}", std::process::id()));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }
    std::fs::create_dir_all(&temp_dir)?;

    // Clean up on error
    let _cleanup = scopeguard::guard(temp_dir.clone(), |dir| {
        std::fs::remove_dir_all(dir).ok();
    });

    // Extract archive
    let decoder = GzDecoder::new(dist_bytes.as_slice());
    let mut archive = Archive::new(decoder);

    for entry in archive
        .entries()
        .context("Error: Invalid .locald-distribution archive")?
    {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();

        // Security: validate no path traversal
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                anyhow::bail!("Error: Archive contains path traversal attack");
            }
        }

        let dest = temp_dir.join(&path);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        std::fs::write(&dest, content)?;
    }

    // === Phase 2: Parse Manifest ===
    let manifest_path = temp_dir.join("distribution.toml");
    let manifest_content = std::fs::read_to_string(&manifest_path)
        .context("Error: Distribution missing distribution.toml")?;

    let manifest = DistributionManifest::parse(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Error: Invalid distribution: {e}"))?;

    println!(
        "→ Initializing from {} v{}",
        manifest.distribution.name, manifest.distribution.version
    );

    // === Phase 3: Compatibility Check ===
    if let Some(ref min_version) = manifest.compatibility.locald_min {
        let current: semver::Version = LOCALD_VERSION
            .parse()
            .unwrap_or_else(|_| semver::Version::new(0, 1, 0));
        let required: semver::Version = min_version
            .parse()
            .context("Invalid locald_min version in distribution")?;

        if current < required {
            anyhow::bail!(
                "Error: Distribution requires locald >= {}, current is {}",
                min_version,
                LOCALD_VERSION
            );
        }
    }

    // === Phase 4: Variable Collection ===
    let mut variables: HashMap<String, String> = HashMap::new();

    // Handle project_name specially
    let project_name = if let Some(n) = name {
        n.to_string()
    } else if let Some(var) = manifest.scaffold.variables.get("project_name") {
        if yes {
            var.default
                .clone()
                .unwrap_or_else(|| "my-project".to_string())
        } else {
            prompt_variable("project_name", var)?
        }
    } else {
        "my-project".to_string()
    };

    variables.insert("project_name".to_string(), project_name.clone());

    // Collect remaining variables
    for (var_name, var_config) in &manifest.scaffold.variables {
        if var_name == "project_name" {
            continue; // Already handled
        }

        let value = if yes {
            var_config.default.clone().unwrap_or_default()
        } else {
            prompt_variable(var_name, var_config)?
        };

        variables.insert(var_name.clone(), value);
    }

    // === Phase 5: Target Directory Setup ===
    let target_dir = target
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&project_name));

    if target_dir.exists() {
        let entries = std::fs::read_dir(&target_dir)?;
        if entries.count() > 0 {
            anyhow::bail!(
                "Error: Target directory '{}' already exists and is not empty",
                target_dir.display()
            );
        }
    }

    std::fs::create_dir_all(&target_dir)
        .with_context(|| format!("Failed to create directory: {}", target_dir.display()))?;

    println!("→ Creating project directory: {}", target_dir.display());

    // Create .locald/plugins directory
    let plugins_dir = target_dir.join(".locald").join("plugins");
    std::fs::create_dir_all(&plugins_dir)?;

    // === Phase 6: Plugin Installation ===
    println!("→ Installing plugins...");

    let packages_dir = temp_dir.join("packages");

    // Install bundled plugins
    for bundled in &manifest.plugins.bundled {
        let pkg_path = packages_dir.join(bundled);
        if !pkg_path.exists() {
            anyhow::bail!("Error: Bundled plugin not found: {}", bundled);
        }

        install_package(&pkg_path, &plugins_dir, verbose)?;
    }

    // Install remote plugins (unless --offline)
    if !offline {
        for remote in &manifest.plugins.remote {
            let url = remote.url();
            let filename = url_filename(url).unwrap_or_else(|| "plugin.locald-package".to_string());

            println!("   ⚡ Fetching {}...", filename);
            let response = reqwest::blocking::get(url)
                .with_context(|| format!("Error: Failed to fetch remote plugin: {}", url))?
                .error_for_status()
                .with_context(|| format!("Download failed for {}", url))?;
            let bytes = response.bytes()?.to_vec();

            // Verify checksum if provided
            if let Some(expected) = remote.checksum() {
                let actual = sha256_hex(&bytes);
                if actual != expected {
                    anyhow::bail!(
                        "Error: Checksum mismatch for {}: expected {}, got {}",
                        url,
                        expected,
                        actual
                    );
                }
            }

            // Write to temp file and install
            let temp_pkg = temp_dir.join(&filename);
            std::fs::write(&temp_pkg, bytes)?;
            install_package(&temp_pkg, &plugins_dir, verbose)?;
        }
    } else if !manifest.plugins.remote.is_empty() {
        println!(
            "   ⚠ Skipping {} remote plugins (offline mode)",
            manifest.plugins.remote.len()
        );
    }

    // === Phase 7: Config Generation ===
    println!("→ Generating configuration...");

    let locald_toml_path = temp_dir.join("locald.toml");
    let locald_toml_template = std::fs::read_to_string(&locald_toml_path)
        .context("Error: Distribution missing locald.toml")?;

    let locald_toml_rendered =
        render_template(&locald_toml_template, &variables).map_err(|e| anyhow::anyhow!("{e}"))?;

    std::fs::write(target_dir.join("locald.toml"), &locald_toml_rendered)?;
    println!("   ✓ locald.toml");

    // === Phase 8: Scaffold Generation ===
    if !no_scaffold {
        let scaffold_dir = temp_dir.join("scaffold");

        // Process templates
        for template in &manifest.scaffold.templates {
            let template_path = scaffold_dir.join(template);
            if template_path.exists() {
                let content = std::fs::read_to_string(&template_path)?;
                let rendered =
                    render_template(&content, &variables).map_err(|e| anyhow::anyhow!("{e}"))?;

                // Remove .template suffix if present
                let output_name = if template.ends_with(".template") {
                    template.strip_suffix(".template").unwrap()
                } else {
                    template.as_str()
                };

                let output_path = target_dir.join(output_name);
                if let Some(parent) = output_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output_path, rendered)?;
                println!("   ✓ {}", output_name);
            }
        }

        // Copy static files
        for file in &manifest.scaffold.files {
            let file_path = scaffold_dir.join(file);
            if file_path.exists() {
                let output_path = target_dir.join(file);
                if let Some(parent) = output_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&file_path, &output_path)?;
                println!("   ✓ {}", file);
            }
        }
    }

    println!("→ Done!");
    println!();
    println!("  Next steps:");
    println!("    cd {}", target_dir.display());
    println!("    locald up");

    Ok(())
}

/// Install a .locald-package to the plugins directory.
fn install_package(pkg_path: &Path, plugins_dir: &Path, verbose: bool) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let pkg_bytes = std::fs::read(pkg_path)?;
    let decoder = GzDecoder::new(pkg_bytes.as_slice());
    let mut archive = Archive::new(decoder);

    let temp_dir = pkg_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(".tmp-pkg-install");
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }
    std::fs::create_dir_all(&temp_dir)?;

    let mut manifest_content: Option<String> = None;
    let mut component_path: Option<PathBuf> = None;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();

        let dest = temp_dir.join(&path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;

        if path.as_os_str() == "manifest.toml" {
            manifest_content = Some(
                String::from_utf8(content.clone()).context("manifest.toml is not valid UTF-8")?,
            );
        } else if path.extension().is_some_and(|e| e == "wasm") {
            component_path = Some(dest.clone());
        }

        std::fs::write(&dest, content)?;
    }

    let manifest_content =
        manifest_content.ok_or_else(|| anyhow::anyhow!("Package missing manifest.toml"))?;

    let manifest = locald_core::plugin::PackageManifest::parse(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Invalid package manifest: {e}"))?;

    let component =
        component_path.ok_or_else(|| anyhow::anyhow!("Package missing WASM component"))?;

    let final_wasm = plugins_dir.join(format!("{}.wasm", manifest.package.name));
    std::fs::copy(&component, &final_wasm)?;

    if verbose {
        println!(
            "   ✓ {} v{}",
            manifest.package.name, manifest.package.version
        );
    } else {
        println!(
            "   ✓ {} v{}",
            manifest.package.name, manifest.package.version
        );
    }

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

/// Prompt user for a variable value.
fn prompt_variable(_name: &str, config: &locald_core::plugin::ScaffoldVariable) -> Result<String> {
    use dialoguer::Input;

    let mut input = Input::<String>::new().with_prompt(&config.prompt);

    if let Some(ref default) = config.default {
        input = input.default(default.clone());
    }

    input.interact_text().context("Failed to read input")
}

/// Add a file to a tar archive.
fn add_file_to_archive<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    path: &str,
    content: &[u8],
    verbose: bool,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();

    archive
        .append_data(&mut header, path, content)
        .with_context(|| format!("Failed to add {} to archive", path))?;

    if verbose {
        println!("  + {}", path);
    }

    Ok(())
}

/// Extract filename from URL.
fn url_filename(url: &str) -> Option<String> {
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

/// Compute SHA-256 hash of data and return as hex string.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}
