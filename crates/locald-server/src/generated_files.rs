use anyhow::{Context, Result};
use jsonc_parser::{ParseOptions, parse_to_serde_value, parse_to_value};
use locald_core::config::{GeneratedFileConfig, LocaldConfig, ServiceConfig};
use locald_core::service::{ListenerName, ServiceKey, ServiceRuntimeBindings};
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_GENERATED_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_GENERATED_SOURCE_BYTES_U64: u64 = 1024 * 1024;
const SERVICE_REFERENCE_PATTERN: &str = r"\$\{services\.([^.]+)\.([^}]+)\}";

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceFingerprint([u8; 32]);

#[derive(Debug)]
struct LoadedSource {
    bytes: Vec<u8>,
    fingerprint: SourceFingerprint,
    format: GeneratedFileFormat,
}

#[derive(Clone, Copy, Debug)]
enum GeneratedFileFormat {
    Json,
    Jsonc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedGeneratedSource {
    value: Value,
    replacements: BTreeMap<String, Value>,
    fingerprint: SourceFingerprint,
}

/// An immutable, validated snapshot of one service's generated-file sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedGeneratedFileSet {
    sources: BTreeMap<String, PreparedGeneratedSource>,
}

/// One complete generation of runtime files owned by a service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GeneratedFileSet {
    generation_dir: PathBuf,
    paths: BTreeMap<String, PathBuf>,
    source_fingerprints: BTreeMap<String, SourceFingerprint>,
}

impl GeneratedFileSet {
    pub(crate) fn path(&self, name: &str) -> Option<&Path> {
        self.paths.get(name).map(PathBuf::as_path)
    }

    pub(crate) async fn sources_match(
        &self,
        project_root: &Path,
        service_config: &ServiceConfig,
    ) -> bool {
        if self.source_fingerprints.len() != service_config.generated().len() {
            return false;
        }

        for (name, config) in service_config.generated() {
            let Ok(source) = load_source(project_root, config).await else {
                return false;
            };
            if self.source_fingerprints.get(name) != Some(&source.fingerprint) {
                return false;
            }
        }
        true
    }

    pub(crate) fn matches_prepared(&self, prepared: &PreparedGeneratedFileSet) -> bool {
        self.source_fingerprints.len() == prepared.sources.len()
            && prepared.sources.iter().all(|(name, source)| {
                self.source_fingerprints.get(name) == Some(&source.fingerprint)
            })
    }

    pub(crate) async fn cleanup(&self) -> Result<()> {
        cleanup_generation_dir(&self.generation_dir).await
    }
}

/// Return a generated-file name from a service interpolation field.
pub(crate) fn generated_name_from_path_field(field: &str) -> Option<&str> {
    field
        .strip_prefix("generated.")
        .and_then(|field| field.strip_suffix(".path"))
        .filter(|name| !name.is_empty())
}

/// Validate generated declarations and their private runtime references.
pub(crate) fn validate_declarations(config: &LocaldConfig) -> Result<()> {
    let valid_name = Regex::new(r"^[A-Za-z][A-Za-z0-9_-]{0,62}$")?;

    for (service_name, service) in &config.services {
        if service.generated().is_empty() {
            continue;
        }
        anyhow::ensure!(
            service.supports_generated_files(),
            "service `{service_name}` declares generated files, but only host exec and worker services support them; build-enabled exec services require an explicit container mount contract"
        );

        let mut output_names = BTreeMap::new();
        for (name, generated) in service.generated() {
            anyhow::ensure!(
                valid_name.is_match(name),
                "service `{service_name}` generated file `{name}` is invalid; generated file names must start with a letter and contain only letters, digits, `_`, or `-` (maximum 63 characters)"
            );
            if let Some(existing) = output_names.insert(name.to_ascii_lowercase(), name.as_str()) {
                anyhow::bail!(
                    "service `{service_name}` generated files `{existing}` and `{name}` collide on a case-insensitive filesystem"
                );
            }
            validate_source_path(service_name, name, &generated.source)?;
            validate_replacement_pointers(service_name, name, generated)?;
            validate_replacement_references(service_name, name, service, generated)?;
        }
    }

    Ok(())
}

/// Load, parse, and validate generated-file sources before a runtime transition begins.
pub(crate) async fn prepare(
    project_root: &Path,
    key: &ServiceKey,
    service_config: &ServiceConfig,
) -> Result<Option<PreparedGeneratedFileSet>> {
    if service_config.generated().is_empty() {
        return Ok(None);
    }

    let mut sources = BTreeMap::new();
    for (name, config) in service_config.generated() {
        let source = load_source(project_root, config).await.with_context(|| {
            format!(
                "failed to load generated file `{name}` for service `{}`",
                key.name()
            )
        })?;
        let value = parse_source(&source).with_context(|| {
            format!(
                "failed to parse generated file `{name}` source `{}`",
                config.source
            )
        })?;
        validate_replacement_targets(&value, &config.replace).with_context(|| {
            format!(
                "failed to validate replacements for generated file `{name}` on service `{}`",
                key.name()
            )
        })?;
        sources.insert(
            name.clone(),
            PreparedGeneratedSource {
                value,
                replacements: config.replace.clone(),
                fingerprint: source.fingerprint,
            },
        );
    }

    Ok(Some(PreparedGeneratedFileSet { sources }))
}

/// Materialize one service's complete generated-file set.
pub(crate) async fn materialize(
    data_dir: &Path,
    project_root: &Path,
    key: &ServiceKey,
    service_config: &ServiceConfig,
    bindings: &ServiceRuntimeBindings,
) -> Result<Option<GeneratedFileSet>> {
    if service_config.generated().is_empty() {
        return Ok(None);
    }

    let service_root = prepare_service_root(data_dir, key).await?;
    let prepared = prepare(project_root, key, service_config)
        .await?
        .context("non-empty generated-file declarations produced no prepared snapshot")?;
    publish_prepared(&service_root, key, bindings, &prepared)
        .await
        .map(Some)
}

/// Materialize a previously prepared snapshot after runtime bindings are allocated.
pub(crate) async fn materialize_prepared(
    data_dir: &Path,
    key: &ServiceKey,
    bindings: &ServiceRuntimeBindings,
    prepared: &PreparedGeneratedFileSet,
) -> Result<GeneratedFileSet> {
    let service_root = prepare_service_root(data_dir, key).await?;
    publish_prepared(&service_root, key, bindings, prepared).await
}

async fn prepare_service_root(data_dir: &Path, key: &ServiceKey) -> Result<PathBuf> {
    let generated_root = data_dir
        .join("instances")
        .join(key.instance().to_string())
        .join("generated");
    create_private_directory(&generated_root, true).await?;

    let service_root = generated_root.join(key.resource_id());
    create_private_directory(&service_root, true).await?;
    Ok(service_root)
}

async fn publish_prepared(
    service_root: &Path,
    key: &ServiceKey,
    bindings: &ServiceRuntimeBindings,
    prepared: &PreparedGeneratedFileSet,
) -> Result<GeneratedFileSet> {
    let generation = uuid::Uuid::new_v4().to_string();
    let staging_dir = service_root.join(format!(".staging-{generation}"));
    let generation_dir = service_root.join(generation);
    create_private_directory(&staging_dir, false).await?;

    let render_result = render_generation(key, bindings, prepared, &staging_dir).await;
    let (relative_paths, source_fingerprints) = match render_result {
        Ok(rendered) => rendered,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            return Err(error);
        }
    };

    if let Err(error) = sync_directory(&staging_dir).await {
        let _ = tokio::fs::remove_dir_all(&staging_dir).await;
        return Err(error);
    }
    if let Err(error) = tokio::fs::rename(&staging_dir, &generation_dir).await {
        let _ = tokio::fs::remove_dir_all(&staging_dir).await;
        return Err(error).with_context(|| {
            format!(
                "failed to publish generated-file generation `{}`",
                generation_dir.display()
            )
        });
    }
    if let Err(error) = sync_directory(service_root).await {
        let _ = tokio::fs::remove_dir_all(&generation_dir).await;
        let _ = sync_directory(service_root).await;
        return Err(error);
    }

    let paths = relative_paths
        .into_iter()
        .map(|(name, relative)| (name, generation_dir.join(relative)))
        .collect();
    Ok(GeneratedFileSet {
        generation_dir,
        paths,
        source_fingerprints,
    })
}

/// Remove every generated runtime file for one project instance.
#[cfg(test)]
pub(crate) async fn cleanup_instance(
    data_dir: &Path,
    instance_id: locald_core::ProjectInstanceId,
) -> Result<()> {
    let root = data_dir
        .join("instances")
        .join(instance_id.to_string())
        .join("generated");
    remove_generated_root(&root).await.with_context(|| {
        format!(
            "failed to remove generated files for project instance {instance_id} at `{}`",
            root.display()
        )
    })
}

/// Remove ephemeral generated runtime files for every recorded instance root.
pub(crate) async fn cleanup_all_instances(data_dir: &Path) -> Result<()> {
    let instances_root = data_dir.join("instances");
    let mut entries = match tokio::fs::read_dir(&instances_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect generated-file instance roots at `{}`",
                    instances_root.display()
                )
            });
        }
    };

    while let Some(entry) = entries.next_entry().await.with_context(|| {
        format!(
            "failed to enumerate generated-file instance roots at `{}`",
            instances_root.display()
        )
    })? {
        let file_type = entry.file_type().await.with_context(|| {
            format!(
                "failed to inspect generated-file instance root `{}`",
                entry.path().display()
            )
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let generated_root = entry.path().join("generated");
        remove_generated_root(&generated_root)
            .await
            .with_context(|| {
                format!(
                    "failed to remove stale generated files at `{}`",
                    generated_root.display()
                )
            })?;
    }

    Ok(())
}

async fn remove_generated_root(root: &Path) -> Result<()> {
    let metadata = match tokio::fs::symlink_metadata(root).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        tokio::fs::remove_file(root).await?;
    } else {
        tokio::fs::remove_dir_all(root).await?;
    }
    Ok(())
}

async fn create_private_directory(path: &Path, recursive: bool) -> Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).await.with_context(|| {
        format!(
            "failed to create generated-file directory `{}`",
            path.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .with_context(|| {
                format!(
                    "failed to make generated-file directory private `{}`",
                    path.display()
                )
            })?;
    }

    Ok(())
}

async fn render_generation(
    key: &ServiceKey,
    bindings: &ServiceRuntimeBindings,
    prepared: &PreparedGeneratedFileSet,
    staging_dir: &Path,
) -> Result<(
    BTreeMap<String, PathBuf>,
    BTreeMap<String, SourceFingerprint>,
)> {
    let mut relative_paths = BTreeMap::new();
    let mut source_fingerprints = BTreeMap::new();

    for (name, source) in &prepared.sources {
        let mut value = source.value.clone();
        apply_replacements(
            &mut value,
            &source.replacements,
            key.name().as_str(),
            bindings,
        )
        .with_context(|| {
            format!(
                "failed to apply replacements for generated file `{name}` on service `{}`",
                key.name()
            )
        })?;

        let mut rendered =
            serde_json::to_vec_pretty(&value).context("failed to serialize generated JSON")?;
        rendered.push(b'\n');
        let relative = PathBuf::from(format!("{name}.json"));
        let output = staging_dir.join(&relative);
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&output)
            .await
            .with_context(|| format!("failed to create generated file `{}`", output.display()))?;
        file.write_all(&rendered)
            .await
            .with_context(|| format!("failed to write generated file `{}`", output.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("failed to sync generated file `{}`", output.display()))?;
        relative_paths.insert(name.clone(), relative);
        source_fingerprints.insert(name.clone(), source.fingerprint.clone());
    }

    Ok((relative_paths, source_fingerprints))
}

async fn load_source(project_root: &Path, config: &GeneratedFileConfig) -> Result<LoadedSource> {
    let format = source_format(&config.source)?;
    let configured = Path::new(&config.source);
    let canonical_root = tokio::fs::canonicalize(project_root)
        .await
        .with_context(|| {
            format!(
                "failed to resolve generated-file project root `{}`",
                project_root.display()
            )
        })?;
    let canonical_source = tokio::fs::canonicalize(canonical_root.join(configured))
        .await
        .with_context(|| format!("generated-file source `{}` does not exist", config.source))?;
    anyhow::ensure!(
        canonical_source.starts_with(&canonical_root),
        "generated-file source `{}` resolves outside project root `{}`",
        config.source,
        canonical_root.display()
    );
    let file = tokio::fs::File::open(&canonical_source)
        .await
        .with_context(|| {
            format!(
                "failed to open generated-file source `{}`",
                canonical_source.display()
            )
        })?;
    let metadata = file.metadata().await.with_context(|| {
        format!(
            "failed to inspect generated-file source `{}`",
            canonical_source.display()
        )
    })?;
    anyhow::ensure!(
        metadata.is_file(),
        "generated-file source `{}` is not a regular file",
        config.source
    );
    anyhow::ensure!(
        metadata.len() <= MAX_GENERATED_SOURCE_BYTES_U64,
        "generated-file source `{}` is {} bytes; maximum supported size is {MAX_GENERATED_SOURCE_BYTES} bytes",
        config.source,
        metadata.len()
    );
    let source_length = usize::try_from(metadata.len()).with_context(|| {
        format!(
            "generated-file source `{}` does not fit in this platform's address space",
            config.source
        )
    })?;
    let mut bytes = Vec::with_capacity(source_length);
    file.take(MAX_GENERATED_SOURCE_BYTES_U64 + 1)
        .read_to_end(&mut bytes)
        .await
        .with_context(|| {
            format!(
                "failed to read generated-file source `{}`",
                canonical_source.display()
            )
        })?;
    anyhow::ensure!(
        bytes.len() <= MAX_GENERATED_SOURCE_BYTES,
        "generated-file source `{}` grew beyond the {MAX_GENERATED_SOURCE_BYTES}-byte limit while being read",
        config.source
    );
    let fingerprint = SourceFingerprint(Sha256::digest(&bytes).into());
    Ok(LoadedSource {
        bytes,
        fingerprint,
        format,
    })
}

fn parse_source(source: &LoadedSource) -> Result<Value> {
    match source.format {
        GeneratedFileFormat::Json => {
            serde_json::from_slice(&source.bytes).context("source is not strict JSON")
        }
        GeneratedFileFormat::Jsonc => {
            let text = std::str::from_utf8(&source.bytes).context("JSONC source is not UTF-8")?;
            let options = ParseOptions {
                allow_comments: true,
                allow_loose_object_property_names: false,
                allow_trailing_commas: true,
                allow_missing_commas: false,
                allow_single_quoted_strings: false,
                allow_hexadecimal_numbers: false,
                allow_unary_plus_numbers: false,
            };
            parse_to_value(text, &options)
                .context("source is not supported JSONC")?
                .context("JSONC source is empty")?;
            parse_to_serde_value::<Value>(text, &options).context("source is not supported JSONC")
        }
    }
}

fn validate_replacement_targets(
    root: &Value,
    replacements: &BTreeMap<String, Value>,
) -> Result<()> {
    let mut candidate = root.clone();
    for (pointer, replacement) in replacements {
        let target = candidate.pointer_mut(pointer).with_context(|| {
            format!("JSON Pointer `{pointer}` does not identify an existing value")
        })?;
        // Preserve the configured replacement's container shape while
        // validating targets in the same deterministic order used during
        // rendering. Runtime binding resolution changes scalar values, not
        // whether a replacement is an object, array, or scalar.
        *target = replacement.clone();
    }
    Ok(())
}

fn apply_replacements(
    root: &mut Value,
    replacements: &BTreeMap<String, Value>,
    service_name: &str,
    bindings: &ServiceRuntimeBindings,
) -> Result<()> {
    for (pointer, replacement) in replacements {
        let resolved = resolve_value(replacement, service_name, bindings)?;
        let target = root.pointer_mut(pointer).with_context(|| {
            format!("JSON Pointer `{pointer}` does not identify an existing value")
        })?;
        *target = resolved;
    }
    Ok(())
}

fn resolve_value(
    value: &Value,
    service_name: &str,
    bindings: &ServiceRuntimeBindings,
) -> Result<Value> {
    match value {
        Value::String(value) => resolve_string(value, service_name, bindings),
        Value::Array(values) => values
            .iter()
            .map(|value| resolve_value(value, service_name, bindings))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(name, value)| {
                resolve_value(value, service_name, bindings)
                    .map(|resolved| (name.clone(), resolved))
            })
            .collect::<Result<serde_json::Map<_, _>>>()
            .map(Value::Object),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(value.clone()),
    }
}

fn resolve_string(
    value: &str,
    service_name: &str,
    bindings: &ServiceRuntimeBindings,
) -> Result<Value> {
    let pattern = Regex::new(SERVICE_REFERENCE_PATTERN)?;
    let captures = pattern.captures_iter(value).collect::<Vec<_>>();
    if captures.is_empty() {
        return Ok(Value::String(value.to_owned()));
    }

    if captures.len() == 1
        && captures[0]
            .get(0)
            .is_some_and(|full| full.as_str() == value)
    {
        let port = resolve_binding_reference(&captures[0], service_name, bindings)?;
        return Ok(Value::Number(port.into()));
    }

    let mut resolved = value.to_owned();
    let mut replacements = Vec::new();
    for captures in captures {
        let full = captures
            .get(0)
            .context("generated replacement reference has no complete match")?;
        let port = resolve_binding_reference(&captures, service_name, bindings)?;
        replacements.push((full.range(), port.to_string()));
    }
    replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, port) in replacements {
        resolved.replace_range(range, &port);
    }
    Ok(Value::String(resolved))
}

fn resolve_binding_reference(
    captures: &regex::Captures<'_>,
    service_name: &str,
    bindings: &ServiceRuntimeBindings,
) -> Result<u16> {
    let referenced_service = captures
        .get(1)
        .context("generated replacement reference is missing its service")?
        .as_str();
    let field = captures
        .get(2)
        .context("generated replacement reference is missing its field")?
        .as_str();
    anyhow::ensure!(
        referenced_service == service_name,
        "generated replacement references service `{referenced_service}`; generated files may use only their owning service `{service_name}`"
    );
    if field == "port" {
        return bindings
            .primary_port()
            .context("owning service has no primary port");
    }
    let listener = ListenerName::from_port_field(field)
        .with_context(|| format!("generated replacement references unsupported field `{field}`"))?;
    bindings
        .listener_port(listener)
        .with_context(|| format!("owning service has no listener `{listener}`"))
}

fn validate_source_path(service_name: &str, name: &str, source: &str) -> Result<()> {
    let path = Path::new(source);
    anyhow::ensure!(
        !path.is_absolute(),
        "service `{service_name}` generated file `{name}` source must be project-relative"
    );
    anyhow::ensure!(
        !source.trim().is_empty(),
        "service `{service_name}` generated file `{name}` source is empty"
    );
    anyhow::ensure!(
        !path.components().any(|component| matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )),
        "service `{service_name}` generated file `{name}` source may not traverse outside the project"
    );
    source_format(source).with_context(|| {
        format!("service `{service_name}` generated file `{name}` has an unsupported source")
    })?;
    Ok(())
}

fn source_format(source: &str) -> Result<GeneratedFileFormat> {
    match Path::new(source)
        .extension()
        .and_then(|value| value.to_str())
    {
        Some("json") => Ok(GeneratedFileFormat::Json),
        Some("jsonc") => Ok(GeneratedFileFormat::Jsonc),
        _ => anyhow::bail!(
            "generated-file source `{source}` must use a `.json` or `.jsonc` extension"
        ),
    }
}

fn validate_replacement_pointers(
    service_name: &str,
    name: &str,
    config: &GeneratedFileConfig,
) -> Result<()> {
    let mut parsed = Vec::new();
    for pointer in config.replace.keys() {
        let tokens = parse_pointer(pointer).with_context(|| {
            format!(
                "service `{service_name}` generated file `{name}` has invalid JSON Pointer `{pointer}`"
            )
        })?;
        parsed.push((pointer, tokens));
    }

    for (index, (left_pointer, left)) in parsed.iter().enumerate() {
        for (right_pointer, right) in parsed.iter().skip(index + 1) {
            anyhow::ensure!(
                !is_prefix(left, right) && !is_prefix(right, left),
                "service `{service_name}` generated file `{name}` replacements `{left_pointer}` and `{right_pointer}` overlap"
            );
        }
    }
    Ok(())
}

fn parse_pointer(pointer: &str) -> Result<Vec<String>> {
    anyhow::ensure!(!pointer.is_empty(), "root replacement is not supported");
    anyhow::ensure!(pointer.starts_with('/'), "pointer must start with `/`");
    pointer[1..]
        .split('/')
        .map(|token| {
            anyhow::ensure!(token != "-", "array append token `-` is not supported");
            let mut decoded = String::new();
            let mut characters = token.chars();
            while let Some(character) = characters.next() {
                if character != '~' {
                    decoded.push(character);
                    continue;
                }
                match characters.next() {
                    Some('0') => decoded.push('~'),
                    Some('1') => decoded.push('/'),
                    Some(other) => anyhow::bail!("invalid `~{other}` escape"),
                    None => anyhow::bail!("trailing `~` escape"),
                }
            }
            Ok(decoded)
        })
        .collect()
}

fn is_prefix(left: &[String], right: &[String]) -> bool {
    left.len() < right.len() && right.starts_with(left)
}

fn validate_replacement_references(
    service_name: &str,
    name: &str,
    service: &ServiceConfig,
    config: &GeneratedFileConfig,
) -> Result<()> {
    let pattern = Regex::new(SERVICE_REFERENCE_PATTERN)?;
    for replacement in config.replace.values() {
        visit_strings(replacement, &mut |value| {
            for captures in pattern.captures_iter(value) {
                let referenced_service = captures
                    .get(1)
                    .context("generated replacement reference is missing its service")?
                    .as_str();
                let field = captures
                    .get(2)
                    .context("generated replacement reference is missing its field")?
                    .as_str();
                anyhow::ensure!(
                    referenced_service == service_name,
                    "service `{service_name}` generated file `{name}` references service `{referenced_service}`; generated files may use only their owning service"
                );
                if field == "port" {
                    anyhow::ensure!(
                        !matches!(
                            service,
                            ServiceConfig::Typed(locald_core::config::TypedServiceConfig::Worker(
                                _
                            ))
                        ) || service.port().is_some(),
                        "service `{service_name}` generated file `{name}` references its primary port, but this worker has no configured primary port"
                    );
                    continue;
                }
                let listener = ListenerName::from_port_field(field).with_context(|| {
                    format!(
                        "service `{service_name}` generated file `{name}` references unsupported field `{field}`"
                    )
                })?;
                anyhow::ensure!(
                    service
                        .listeners()
                        .iter()
                        .any(|configured| configured == listener),
                    "service `{service_name}` generated file `{name}` references unknown listener `{listener}`"
                );
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn visit_strings<F>(value: &Value, visitor: &mut F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    match value {
        Value::String(value) => visitor(value),
        Value::Array(values) => {
            for value in values {
                visit_strings(value, visitor)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for value in values.values() {
                visit_strings(value, visitor)?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

async fn cleanup_generation_dir(generation_dir: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(generation_dir).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to remove generated-file generation `{}`",
                    generation_dir.display()
                )
            });
        }
    }
    if let Some(service_root) = generation_dir.parent() {
        let _ = tokio::fs::remove_dir(service_root).await;
    }
    Ok(())
}

async fn sync_directory(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("failed to sync directory `{}`", path.display()))
    })
    .await
    .context("generated-file directory sync task failed")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use locald_core::config::{CommonServiceConfig, ExecServiceConfig};
    use locald_core::service::{ServiceName, ServiceRuntimeBindings};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn service_config(source: &str, replacements: BTreeMap<String, Value>) -> ServiceConfig {
        ServiceConfig::Legacy(ExecServiceConfig {
            common: CommonServiceConfig {
                listeners: vec!["chat".to_owned()],
                generated: BTreeMap::from([(
                    "microfrontends".to_owned(),
                    GeneratedFileConfig {
                        source: source.to_owned(),
                        replace: replacements,
                    },
                )]),
                ..CommonServiceConfig::default()
            },
            command: Some("true".to_owned()),
            ..ExecServiceConfig::default()
        })
    }

    fn key() -> ServiceKey {
        key_for("00000000-0000-4000-8000-000000000001")
    }

    fn key_for(instance: &str) -> ServiceKey {
        ServiceKey::new(
            instance.parse().expect("valid instance"),
            ServiceName::new("web"),
        )
    }

    fn bindings() -> ServiceRuntimeBindings {
        ServiceRuntimeBindings::new(
            Some(4100),
            BTreeMap::from([(ListenerName::new("chat"), 4200)]),
        )
    }

    #[tokio::test]
    async fn materializes_jsonc_with_typed_existing_path_replacements() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(
            root.path().join("microfrontends.jsonc"),
            r#"{
                // local proxy
                "options": { "localProxyPort": 3000, },
                "applications": { "chat": { "development": { "local": 3002 } } },
                "label": "",
                "combined": "",
                "metadata": null,
            }"#,
        )
        .await
        .expect("write source");
        let config = service_config(
            "microfrontends.jsonc",
            BTreeMap::from([
                (
                    "/options/localProxyPort".to_owned(),
                    Value::String("${services.web.port}".to_owned()),
                ),
                (
                    "/applications/chat/development/local".to_owned(),
                    Value::String("${services.web.listeners.chat.port}".to_owned()),
                ),
                (
                    "/label".to_owned(),
                    Value::String("chat-${services.web.listeners.chat.port}".to_owned()),
                ),
                (
                    "/combined".to_owned(),
                    Value::String(
                        "${services.web.port}:${services.web.listeners.chat.port}".to_owned(),
                    ),
                ),
                (
                    "/metadata".to_owned(),
                    serde_json::json!({
                        "primary": "${services.web.port}",
                        "listener": "${services.web.listeners.chat.port}"
                    }),
                ),
            ]),
        );

        let generated = materialize(
            &root.path().join("data"),
            root.path(),
            &key(),
            &config,
            &bindings(),
        )
        .await
        .expect("materialize")
        .expect("generated set");
        let output =
            tokio::fs::read_to_string(generated.path("microfrontends").expect("generated path"))
                .await
                .expect("read output");
        let value: Value = serde_json::from_str(&output).expect("valid output JSON");
        assert_eq!(
            value.pointer("/options/localProxyPort"),
            Some(&Value::from(4100))
        );
        assert_eq!(
            value.pointer("/applications/chat/development/local"),
            Some(&Value::from(4200))
        );
        assert_eq!(value.pointer("/label"), Some(&Value::from("chat-4200")));
        assert_eq!(value.pointer("/combined"), Some(&Value::from("4100:4200")));
        assert_eq!(value.pointer("/metadata/primary"), Some(&Value::from(4100)));
        assert_eq!(
            value.pointer("/metadata/listener"),
            Some(&Value::from(4200))
        );
        assert!(output.ends_with('\n'));
        generated.cleanup().await.expect("clean generation");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generated_hierarchy_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().expect("create project root");
        tokio::fs::write(root.path().join("runtime.json"), r#"{"port":3000}"#)
            .await
            .expect("write source");
        let data_dir = root.path().join("data");
        let generated_root = data_dir
            .join("instances")
            .join(key().instance().to_string())
            .join("generated");
        let existing_service_root = generated_root.join(key().resource_id());
        tokio::fs::create_dir_all(&existing_service_root)
            .await
            .expect("seed permissive generated hierarchy");
        for directory in [&generated_root, &existing_service_root] {
            tokio::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))
                .await
                .expect("seed permissive generated directory");
        }
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );

        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize")
            .expect("generated set");
        let service_root = generated
            .generation_dir
            .parent()
            .expect("generation has service root");
        let output = generated.path("microfrontends").expect("generated path");

        for directory in [
            generated_root.as_path(),
            service_root,
            generated.generation_dir.as_path(),
        ] {
            let mode = tokio::fs::metadata(directory)
                .await
                .expect("inspect generated directory")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700, "{}", directory.display());
        }
        let file_mode = tokio::fs::metadata(output)
            .await
            .expect("inspect generated file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn rejects_invalid_source_paths_and_replacement_pointers() {
        for source in ["", "/tmp/source.json", "../source.json", "source.toml"] {
            let config = service_config(source, BTreeMap::new());
            let locald = LocaldConfig {
                services: std::collections::HashMap::from([("web".to_owned(), config)]),
                ..LocaldConfig::default()
            };
            assert!(validate_declarations(&locald).is_err(), "{source}");
        }

        let replacement = Value::from(1);
        for pointer in ["", "options/port", "/options/~2port", "/array/-"] {
            let config = service_config(
                "source.json",
                BTreeMap::from([(pointer.to_owned(), replacement.clone())]),
            );
            let locald = LocaldConfig {
                services: std::collections::HashMap::from([("web".to_owned(), config)]),
                ..LocaldConfig::default()
            };
            assert!(validate_declarations(&locald).is_err(), "{pointer}");
        }

        let config = service_config(
            "source.json",
            BTreeMap::from([
                ("/options".to_owned(), replacement.clone()),
                ("/options/port".to_owned(), replacement),
            ]),
        );
        let locald = LocaldConfig {
            services: std::collections::HashMap::from([("web".to_owned(), config)]),
            ..LocaldConfig::default()
        };
        assert!(validate_declarations(&locald).is_err());

        let case_collision: LocaldConfig = toml::from_str(
            r#"
[project]
name = "case-collision"

[services.web]
command = "serve"

[services.web.generated.Runtime]
source = "runtime.json"

[services.web.generated.runtime]
source = "runtime.json"
"#,
        )
        .expect("parse case-colliding generated files");
        let error = validate_declarations(&case_collision)
            .expect_err("case-insensitive output names must not collide");
        assert!(error.to_string().contains("case-insensitive filesystem"));
    }

    #[test]
    fn jsonc_rejects_extensions_beyond_comments_and_trailing_commas() {
        for source in [
            r"{ loose: 1 }",
            r#"{"first": 1 "second": 2}"#,
            r"{'single': 1}",
            r#"{"hex": 0x10}"#,
            r#"{"positive": +1}"#,
        ] {
            let loaded = LoadedSource {
                bytes: source.as_bytes().to_vec(),
                fingerprint: SourceFingerprint([0; 32]),
                format: GeneratedFileFormat::Jsonc,
            };
            assert!(parse_source(&loaded).is_err(), "{source}");
        }
    }

    #[test]
    fn jsonc_preserves_null_and_rejects_empty_documents() {
        for source in ["null", "/* comment */ null // trailing comment"] {
            let loaded = LoadedSource {
                bytes: source.as_bytes().to_vec(),
                fingerprint: SourceFingerprint([0; 32]),
                format: GeneratedFileFormat::Jsonc,
            };
            assert_eq!(
                parse_source(&loaded).expect("parse JSONC null"),
                Value::Null
            );
        }

        for source in ["", " \n\t", "// comment only"] {
            let loaded = LoadedSource {
                bytes: source.as_bytes().to_vec(),
                fingerprint: SourceFingerprint([0; 32]),
                format: GeneratedFileFormat::Jsonc,
            };
            let error = parse_source(&loaded).expect_err("empty JSONC must be rejected");
            assert!(error.to_string().contains("JSONC source is empty"));
        }
    }

    #[tokio::test]
    async fn strict_json_rejects_jsonc_and_cleans_the_staging_generation() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(
            root.path().join("runtime.json"),
            r#"{
                // comments require a .jsonc source
                "port": 3000
            }"#,
        )
        .await
        .expect("write strict JSON source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let data_dir = root.path().join("data");

        let error = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect_err("strict JSON source rejects comments");
        assert!(format!("{error:#}").contains("strict JSON"));

        let service_root = data_dir
            .join("instances")
            .join(key().instance().to_string())
            .join("generated")
            .join(key().resource_id());
        let mut entries = tokio::fs::read_dir(service_root)
            .await
            .expect("service root remains inspectable");
        assert!(
            entries
                .next_entry()
                .await
                .expect("read service root")
                .is_none(),
            "a failed render leaves no staging or published generation"
        );
    }

    #[tokio::test]
    async fn missing_pointer_is_rejected_during_preparation_without_publishing_a_generation() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(root.path().join("runtime.json"), r#"{"port":3000}"#)
            .await
            .expect("write source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/missing".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let data_dir = root.path().join("data");

        let error = prepare(root.path(), &key(), &config)
            .await
            .expect_err("preparation must validate replacement targets");
        assert!(format!("{error:#}").contains("does not identify an existing value"));
        assert!(
            !data_dir.exists(),
            "preparation must not create runtime output directories"
        );

        let error = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect_err("replacement must target an existing value");
        assert!(format!("{error:#}").contains("does not identify an existing value"));

        let service_root = data_dir
            .join("instances")
            .join(key().instance().to_string())
            .join("generated")
            .join(key().resource_id());
        let mut entries = tokio::fs::read_dir(service_root)
            .await
            .expect("service root remains inspectable");
        assert!(
            entries
                .next_entry()
                .await
                .expect("read service root")
                .is_none()
        );
    }

    #[tokio::test]
    async fn source_fingerprints_detect_changes_without_exposing_generated_paths() {
        let root = tempdir().expect("create project root");
        let source = root.path().join("runtime.json");
        tokio::fs::write(&source, r#"{"port":3000}"#)
            .await
            .expect("write source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let generated = materialize(
            &root.path().join("data"),
            root.path(),
            &key(),
            &config,
            &bindings(),
        )
        .await
        .expect("materialize")
        .expect("generated set");

        assert!(generated.sources_match(root.path(), &config).await);
        tokio::fs::write(&source, r#"{"port":3001}"#)
            .await
            .expect("change source");
        assert!(!generated.sources_match(root.path(), &config).await);
    }

    #[tokio::test]
    async fn prepared_snapshot_survives_live_invalid_mutation_and_detects_the_mismatch() {
        let root = tempdir().expect("create project root");
        let source = root.path().join("runtime.json");
        tokio::fs::write(&source, r#"{"port":3000}"#)
            .await
            .expect("write valid source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let prepared = prepare(root.path(), &key(), &config)
            .await
            .expect("prepare valid source")
            .expect("prepared generated files");

        tokio::fs::write(&source, r#"{"port":"#)
            .await
            .expect("replace live source with invalid JSON");

        let generated =
            materialize_prepared(&root.path().join("data"), &key(), &bindings(), &prepared)
                .await
                .expect("materialize the validated snapshot");
        let output =
            tokio::fs::read_to_string(generated.path("microfrontends").expect("generated path"))
                .await
                .expect("read generated output");
        let value: Value = serde_json::from_str(&output).expect("valid generated JSON");

        assert_eq!(value.pointer("/port"), Some(&Value::from(4100)));
        assert!(generated.matches_prepared(&prepared));
        assert!(
            !generated.sources_match(root.path(), &config).await,
            "the invalid live source must queue a later retry without changing this generation"
        );
    }

    #[tokio::test]
    async fn generations_are_instance_scoped_and_instance_cleanup_is_selective() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(root.path().join("runtime.json"), r#"{"port":3000}"#)
            .await
            .expect("write source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let first_key = key_for("00000000-0000-4000-8000-000000000001");
        let second_key = key_for("00000000-0000-4000-8000-000000000002");
        let data_dir = root.path().join("data");
        let first = materialize(&data_dir, root.path(), &first_key, &config, &bindings())
            .await
            .expect("materialize first instance")
            .expect("first generated set");
        let second = materialize(&data_dir, root.path(), &second_key, &config, &bindings())
            .await
            .expect("materialize second instance")
            .expect("second generated set");
        let first_path = first.path("microfrontends").expect("first path");
        let second_path = second.path("microfrontends").expect("second path");

        assert_ne!(first_path, second_path);
        assert!(
            first_path.starts_with(
                data_dir
                    .join("instances")
                    .join(first_key.instance().to_string())
            )
        );
        assert!(
            second_path.starts_with(
                data_dir
                    .join("instances")
                    .join(second_key.instance().to_string())
            )
        );

        cleanup_instance(&data_dir, first_key.instance())
            .await
            .expect("clean first instance");
        assert!(!first_path.exists());
        assert!(second_path.exists());
    }

    #[tokio::test]
    async fn restart_cleanup_removes_catalogued_and_orphan_generated_roots_only() {
        let root = tempdir().expect("create generated restart-cleanup root");
        let data_dir = root.path().join("data");
        let valid_instance = data_dir
            .join("instances")
            .join(key().instance().to_string());
        let orphan_instance = data_dir.join("instances").join("orphaned-instance");
        for instance in [&valid_instance, &orphan_instance] {
            tokio::fs::create_dir_all(instance.join("generated/service/generation"))
                .await
                .expect("create generated restart fixture");
            tokio::fs::write(
                instance.join("generated/service/generation/runtime.json"),
                "{}",
            )
            .await
            .expect("write generated restart fixture");
            tokio::fs::write(instance.join("availability.json"), "{}")
                .await
                .expect("write persistent instance fixture");
        }

        cleanup_all_instances(&data_dir)
            .await
            .expect("clean all generated instance roots");

        for instance in [&valid_instance, &orphan_instance] {
            assert!(!instance.join("generated").exists());
            assert!(instance.join("availability.json").exists());
        }
    }

    #[tokio::test]
    async fn failed_replacement_preserves_the_active_generation() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(root.path().join("runtime.json"), r#"{"port":3000}"#)
            .await
            .expect("write source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let data_dir = root.path().join("data");
        let active = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize active generation")
            .expect("active generated set");
        let active_path = active
            .path("microfrontends")
            .expect("active generated path")
            .to_path_buf();

        let invalid = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/missing".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        materialize(&data_dir, root.path(), &key(), &invalid, &bindings())
            .await
            .expect_err("replacement with a missing pointer must fail");

        assert!(
            active_path.exists(),
            "failed candidate rendering must preserve the active generation"
        );
        active.cleanup().await.expect("clean active generation");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn source_symlink_cannot_escape_the_project_root() {
        let root = tempdir().expect("create project root");
        let outside = tempdir().expect("create outside directory");
        let outside_source = outside.path().join("outside.json");
        tokio::fs::write(&outside_source, r#"{"port":3000}"#)
            .await
            .expect("write outside source");
        std::os::unix::fs::symlink(&outside_source, root.path().join("runtime.json"))
            .expect("create escaping symlink");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );

        let error = materialize(
            &root.path().join("data"),
            root.path(),
            &key(),
            &config,
            &bindings(),
        )
        .await
        .expect_err("escaping source must fail");
        assert!(format!("{error:#}").contains("resolves outside project root"));
    }

    #[tokio::test]
    async fn oversized_sources_fail_before_parsing_or_publication() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(
            root.path().join("runtime.json"),
            vec![b' '; MAX_GENERATED_SOURCE_BYTES + 1],
        )
        .await
        .expect("write oversized source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let data_dir = root.path().join("data");

        let error = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect_err("oversized source must fail");
        assert!(format!("{error:#}").contains("maximum supported size"));
        let service_root = data_dir
            .join("instances")
            .join(key().instance().to_string())
            .join("generated")
            .join(key().resource_id());
        let mut entries = tokio::fs::read_dir(service_root)
            .await
            .expect("service root remains inspectable");
        assert!(
            entries
                .next_entry()
                .await
                .expect("read service root")
                .is_none()
        );
    }
}
