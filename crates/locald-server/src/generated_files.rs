use crate::config_loader::{
    SERVICE_REFERENCE_PATTERN, ServiceReference, resolve_owned_service_reference,
};
use crate::health::ReadinessRequirement;
use anyhow::{Context, Result};
#[cfg(unix)]
use cap_std::fs::{MetadataExt as CapMetadataExt, OpenOptionsExt as CapOpenOptionsExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use jsonc_parser::{ParseOptions, parse_to_serde_value, parse_to_value};
use locald_core::ProjectInstanceId;
use locald_core::config::{GeneratedFileConfig, LocaldConfig, ServiceConfig};
use locald_core::service::{ListenerName, ServiceKey, ServiceRuntimeBindings};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::ffi::CString;
use std::ffi::OsString;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

const MAX_GENERATED_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_GENERATED_SOURCE_BYTES_U64: u64 = 1024 * 1024;
const PROJECTION_MANIFEST_NAME: &str = ".projection-ownership.json";
const PROJECTION_MANIFEST_VERSION: u32 = 3;
const WRITE_PROBE_JOURNAL: &str = "ownership.json";
#[cfg(target_os = "macos")]
const PROJECTION_PROVENANCE_XATTR: &[u8] = b"com.locald.projection-id\0";
#[cfg(target_os = "linux")]
const PROJECTION_PROVENANCE_XATTR: &[u8] = b"user.locald.projection-id\0";

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceFingerprint([u8; 32]);

#[derive(Debug)]
struct LoadedSource {
    bytes: Vec<u8>,
    identity: ProjectionFileIdentity,
    fingerprint: SourceFingerprint,
    format: GeneratedFileFormat,
}

#[cfg(unix)]
type RootIdentity = (u64, u64);
#[cfg(not(unix))]
type RootIdentity = ();

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
    projection: Option<PreparedProjection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedProjection {
    canonical_project_root: PathBuf,
    project_root_identity: ProjectionFileIdentity,
    relative_path: PathBuf,
    parent_relative_path: PathBuf,
    parent_identity: ProjectionFileIdentity,
    parent_filesystem_identity: ProjectionFilesystemIdentity,
    file_name: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectionFilesystemIdentity {
    device: u64,
    #[cfg(target_os = "linux")]
    mount_id: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ProjectionFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    unsupported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ProjectionOwnership {
    name: String,
    projection_id: String,
    canonical_project_root: PathBuf,
    project_root_identity: ProjectionFileIdentity,
    parent_relative_path: PathBuf,
    parent_identity: ProjectionFileIdentity,
    file_name: PathBuf,
    quarantine_root: PathBuf,
    quarantine_root_identity: ProjectionFileIdentity,
    relative_path: PathBuf,
    target_quarantine_path: PathBuf,
    digest: [u8; 32],
    size: u64,
    identity: ProjectionFileIdentity,
}

struct PlannedProjection {
    name: String,
    projection_id: String,
    canonical_project_root: PathBuf,
    project_root_identity: ProjectionFileIdentity,
    parent_relative_path: PathBuf,
    parent_identity: ProjectionFileIdentity,
    file_name: PathBuf,
    relative_path: PathBuf,
    target_quarantine_path: PathBuf,
    digest: [u8; 32],
    size: u64,
    identity: ProjectionFileIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ProjectionManifest {
    version: u32,
    generation: String,
    projections: Vec<ProjectionOwnership>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WriteProbeJournal {
    version: u32,
    attempt_identity: ProjectionFileIdentity,
    ownership: ProjectionOwnership,
    complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteProbeCheckpoint {
    Journaled,
    Exposed,
    Quarantined,
    Deleted,
    Completed,
}

struct RenderedGeneration {
    relative_paths: BTreeMap<String, PathBuf>,
    source_fingerprints: BTreeMap<String, SourceFingerprint>,
    projected_contents: BTreeMap<String, Vec<u8>>,
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
    projections: Vec<ProjectionOwnership>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupGeneratedFileCleanup {
    pub(crate) instance_id: Option<ProjectInstanceId>,
    pub(crate) service_resource_ids: BTreeSet<OsString>,
    pub(crate) generated_files: GeneratedFileSet,
}

struct LoadedProjectionManifest {
    service_resource_id: OsString,
    generation_dir: PathBuf,
    manifest: ProjectionManifest,
}

#[derive(Debug)]
struct RetainedProjectionError(String);

impl std::fmt::Display for RetainedProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RetainedProjectionError {}

#[derive(Debug)]
struct StaleProjectionParentError(String);

impl std::fmt::Display for StaleProjectionParentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StaleProjectionParentError {}

#[derive(Debug)]
struct RetainedGeneratedFileSetError {
    source: anyhow::Error,
    generated_files: GeneratedFileSet,
}

impl std::fmt::Display for RetainedGeneratedFileSetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, formatter)
    }
}

impl std::error::Error for RetainedGeneratedFileSetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn retained_projection_error(message: impl Into<String>) -> anyhow::Error {
    RetainedProjectionError(message.into()).into()
}

fn ensure_retained_projection(condition: bool, message: impl FnOnce() -> String) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(retained_projection_error(message()))
    }
}

fn retain_inaccessible_projection(
    error: anyhow::Error,
    message: impl FnOnce() -> String,
) -> anyhow::Error {
    let permission_denied = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    });
    if permission_denied {
        retained_projection_error(format!("{}: {error}", message()))
    } else {
        error
    }
}

fn error_retains_projection(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<RetainedProjectionError>().is_some())
}

fn error_proves_stale_projection_parent(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<StaleProjectionParentError>().is_some())
}

fn io_error_proves_stale_projection_parent(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
        || error.kind() == std::io::ErrorKind::NotADirectory
        || error.raw_os_error() == Some(libc::ELOOP)
}

fn stale_projection_parent_error(message: impl Into<String>) -> anyhow::Error {
    StaleProjectionParentError(message.into()).into()
}

pub(crate) fn retained_generated_file_set(error: &anyhow::Error) -> Option<GeneratedFileSet> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<RetainedGeneratedFileSetError>()
            .map(|retained| retained.generated_files.clone())
    })
}

impl GeneratedFileSet {
    /// Exact recorded ownership paths, independent of the current configuration.
    pub(crate) fn projection_paths(&self) -> Vec<(PathBuf, PathBuf)> {
        self.projections
            .iter()
            .map(|projection| {
                (
                    projection.canonical_project_root.clone(),
                    projection.relative_path.clone(),
                )
            })
            .collect()
    }
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
        self.projections_match().await
    }

    pub(crate) async fn matches_prepared(&self, prepared: &PreparedGeneratedFileSet) -> bool {
        if self.source_fingerprints.len() != prepared.sources.len()
            || !prepared.sources.iter().all(|(name, source)| {
                self.source_fingerprints.get(name) == Some(&source.fingerprint)
            })
        {
            return false;
        }
        let prepared_projections = prepared
            .sources
            .values()
            .filter_map(|source| source.projection.as_ref())
            .collect::<Vec<_>>();
        if prepared_projections.len() != self.projections.len() {
            return false;
        }
        for prepared in prepared_projections {
            let Some(owned) = self.projections.iter().find(|owned| {
                owned.canonical_project_root == prepared.canonical_project_root
                    && owned.relative_path == prepared.relative_path
                    && owned.parent_identity == prepared.parent_identity
                    && owned.file_name == prepared.file_name
            }) else {
                return false;
            };
            let owned = owned.clone();
            if !tokio::task::spawn_blocking(move || projection_path_matches_ownership(&owned))
                .await
                .is_ok_and(|result| result.unwrap_or(false))
            {
                return false;
            }
        }
        true
    }

    pub(crate) async fn projections_match(&self) -> bool {
        for projection in &self.projections {
            let projection = projection.clone();
            if !tokio::task::spawn_blocking(move || projection_path_matches_ownership(&projection))
                .await
                .is_ok_and(|result| result.unwrap_or(false))
            {
                return false;
            }
        }
        true
    }

    pub(crate) async fn cleanup(&self) -> Result<()> {
        cleanup_projections(&self.projections).await?;
        cleanup_generation_dir(&self.generation_dir).await
    }

    async fn owns_projection(
        &self,
        canonical_project_root: &Path,
        parent_identity: &ProjectionFileIdentity,
        selected_entry_name: &Path,
        target_metadata: &cap_std::fs::Metadata,
    ) -> bool {
        if !target_metadata.is_file() || target_metadata.file_type().is_symlink() {
            return false;
        }
        let target_identity = projection_file_identity(target_metadata);
        let Some(ownership) = self.projections.iter().find(|ownership| {
            ownership.canonical_project_root == canonical_project_root
                && ownership.parent_identity == *parent_identity
                && ownership.file_name == selected_entry_name
                && ownership.identity == target_identity
        }) else {
            return false;
        };
        let ownership = ownership.clone();
        tokio::task::spawn_blocking(move || projection_path_matches_ownership(&ownership))
            .await
            .is_ok_and(|result| result.unwrap_or(false))
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
    let mut project_targets = BTreeMap::new();

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
            if let Some(project_path) = &generated.project_path {
                anyhow::ensure!(
                    project_path_supported(),
                    "service `{service_name}` generated file `{name}` uses project_path, which is supported only on macOS and Linux"
                );
                validate_project_path(service_name, name, &generated.source, project_path)?;
                // Conservatively reserve canonical caseless identities, including
                // Unicode-equivalent directory components on macOS filesystems.
                let normalized =
                    canonical_caseless(&normalized_relative_path(project_path).to_string_lossy());
                if let Some((existing_service, existing_name, existing_path)) = project_targets
                    .insert(
                        normalized,
                        (service_name.as_str(), name.as_str(), project_path.as_str()),
                    )
                {
                    anyhow::bail!(
                        "services `{existing_service}` and `{service_name}` generated files `{existing_name}` and `{name}` project paths `{existing_path}` and `{project_path}` collide on a case-insensitive filesystem"
                    );
                }
            }
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
    allowed_existing: Option<&GeneratedFileSet>,
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
        anyhow::ensure!(
            !allowed_existing.is_some_and(|existing| existing
                .projections
                .iter()
                .any(|projection| projection.identity == source.identity)),
            "generated-file `{name}` source resolves to a currently owned projection; use an independent source file so stopping the service preserves its input"
        );
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
        let projection = match &config.project_path {
            Some(project_path) => Some(
                prepare_projection_target(project_root, project_path, allowed_existing, &source.identity)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to prepare project_path `{project_path}` for generated file `{name}` on service `{}`",
                            key.name()
                        )
                    })?,
            ),
            None => None,
        };
        sources.insert(
            name.clone(),
            PreparedGeneratedSource {
                value,
                replacements: config.replace.clone(),
                fingerprint: source.fingerprint,
                projection,
            },
        );
    }

    Ok(Some(PreparedGeneratedFileSet { sources }))
}

/// Prepare generated files and reject projection targets that cannot receive
/// hard links from this service's private generation before a runtime stops.
pub(crate) async fn prepare_for_materialization(
    data_dir: &Path,
    project_root: &Path,
    key: &ServiceKey,
    service_config: &ServiceConfig,
    allowed_existing: Option<&GeneratedFileSet>,
) -> Result<Option<PreparedGeneratedFileSet>> {
    let prepared = prepare(project_root, key, service_config, allowed_existing).await?;
    if let Some(prepared) = &prepared {
        ensure_prepared_projection_filesystems(data_dir, key, prepared).await?;
    }
    Ok(prepared)
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
    let prepared = prepare_for_materialization(data_dir, project_root, key, service_config, None)
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
    let service_root = service_root_path(data_dir, key);
    let generated_root = service_root
        .parent()
        .context("generated-file service root has no generated parent")?;
    create_private_directory(generated_root, true).await?;

    create_private_directory(&service_root, true).await?;
    Ok(service_root)
}

fn service_root_path(data_dir: &Path, key: &ServiceKey) -> PathBuf {
    data_dir
        .join("instances")
        .join(key.instance().to_string())
        .join("generated")
        .join(key.resource_id())
}

async fn ensure_prepared_projection_filesystems(
    data_dir: &Path,
    key: &ServiceKey,
    prepared: &PreparedGeneratedFileSet,
) -> Result<()> {
    if !prepared
        .sources
        .values()
        .any(|source| source.projection.is_some())
    {
        return Ok(());
    }

    let mut private_parent = service_root_path(data_dir, key);
    let private_parent = loop {
        match tokio::fs::symlink_metadata(&private_parent).await {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "generated-file private data ancestor `{}` is not a regular directory",
                    private_parent.display()
                );
                break private_parent;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                anyhow::ensure!(
                    private_parent.pop(),
                    "generated-file private data path has no existing ancestor"
                );
            }
            Err(error) => return Err(error.into()),
        }
    };
    let private_filesystem_identity = tokio::task::spawn_blocking(move || {
        let private_parent = Dir::open_ambient_dir(&private_parent, ambient_authority())?;
        projection_filesystem_identity(&private_parent)
    })
    .await
    .context("generated-file private filesystem identity task failed")??;
    validate_prepared_projection_filesystems(&private_filesystem_identity, prepared)?;
    // Probe quarantine can retain an entry supplied by a concurrent project
    // writer. Keep it outside generation garbage collection, including restart
    // cleanup of manifest-less staging directories.
    let probe_root = data_dir.join("projection-write-probes");
    let mut directories_to_sync = Vec::new();
    for path in probe_root.ancestors() {
        directories_to_sync.push(path.to_path_buf());
        if tokio::fs::try_exists(path).await? {
            break;
        }
    }
    create_private_directory(&probe_root, true).await?;
    tokio::task::spawn_blocking(move || {
        for path in directories_to_sync {
            std::fs::File::open(path)?.sync_all()?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .context("write-probe root durability task failed")??;
    verify_prepared_projection_writes(prepared, &probe_root).await
}

fn validate_prepared_projection_filesystems(
    private_identity: &ProjectionFilesystemIdentity,
    prepared: &PreparedGeneratedFileSet,
) -> Result<()> {
    for projection in prepared
        .sources
        .values()
        .filter_map(|source| source.projection.as_ref())
    {
        ensure_projection_same_mount(private_identity, &projection.parent_filesystem_identity)?;
    }
    Ok(())
}

fn ensure_projection_same_mount(
    private_identity: &ProjectionFilesystemIdentity,
    projection_parent_identity: &ProjectionFilesystemIdentity,
) -> Result<()> {
    anyhow::ensure!(
        private_identity == projection_parent_identity,
        "generated-file project_path requires locald's private data directory and the project to be on the same filesystem mount"
    );
    Ok(())
}

async fn verify_prepared_projection_writes(
    prepared: &PreparedGeneratedFileSet,
    probe_root: &Path,
) -> Result<()> {
    let projections = prepared
        .sources
        .values()
        .filter_map(|source| source.projection.clone())
        .collect::<Vec<_>>();
    let probe_root = probe_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        for projection in projections {
            let private_path = probe_root.join(format!(".write-probe-{}", uuid::Uuid::new_v4()));
            create_private_probe_directory(&private_path)?;
            let parent = open_exact_project_parent(
                &projection.canonical_project_root,
                &projection.project_root_identity,
                &projection.parent_relative_path,
                &projection.parent_identity,
            )?;
            let result = verify_projection_parent_write(
                &parent,
                &private_path,
                &projection
                    .canonical_project_root
                    .join(&projection.parent_relative_path),
            )
            .with_context(|| {
                format!(
                    "generated-file project_path parent `{}` cannot create projection entries",
                    projection
                        .canonical_project_root
                        .join(&projection.parent_relative_path)
                        .display()
                )
            });
            match std::fs::remove_dir(&private_path) {
                Ok(()) => {}
                Err(error) if result.is_ok() => return Err(error.into()),
                Err(_) => {}
            }
            result?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("generated-file project_path write preflight task failed")?
}

fn verify_projection_parent_write(
    parent: &Dir,
    private_path: &Path,
    parent_path: &Path,
) -> Result<()> {
    verify_projection_parent_write_with_operations(
        parent,
        private_path,
        parent_path,
        set_projection_provenance,
        rename_projection_noreplace,
    )
}

fn verify_projection_parent_write_with_operations<F, G>(
    parent: &Dir,
    private_path: &Path,
    parent_path: &Path,
    bind_provenance: F,
    rename: G,
) -> Result<()>
where
    F: FnOnce(&cap_std::fs::File, &str) -> Result<()>,
    G: FnMut(&Dir, &Path, &Dir, &Path) -> std::io::Result<()>,
{
    verify_projection_parent_write_with_checkpoints(
        parent,
        private_path,
        parent_path,
        bind_provenance,
        |from_root, from, to_root, to| from_root.hard_link(from, to_root, to),
        rename,
        |_, _| Ok(()),
    )
}

fn create_private_probe_directory(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

fn write_probe_journal(root: &Dir, journal: &WriteProbeJournal, initial: bool) -> Result<()> {
    use std::io::Write as _;
    let name = if initial {
        WRITE_PROBE_JOURNAL
    } else {
        "ownership.next.json"
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = root.open_with(name, &options)?;
    file.write_all(&serde_json::to_vec(journal)?)?;
    file.sync_all()?;
    if !initial {
        root.rename(name, root, WRITE_PROBE_JOURNAL)?;
    }
    root.open(".")?.sync_all()?;
    Ok(())
}

fn verify_projection_parent_write_with_checkpoints<F, H, G, C>(
    parent: &Dir,
    private_path: &Path,
    parent_path: &Path,
    bind_provenance: F,
    hard_link: H,
    mut rename: G,
    mut checkpoint: C,
) -> Result<()>
where
    F: FnOnce(&cap_std::fs::File, &str) -> Result<()>,
    H: FnOnce(&Dir, &Path, &Dir, &Path) -> std::io::Result<()>,
    G: FnMut(&Dir, &Path, &Dir, &Path) -> std::io::Result<()>,
    C: FnMut(WriteProbeCheckpoint, &WriteProbeJournal) -> Result<()>,
{
    let private_root = Dir::open_ambient_dir(private_path, ambient_authority())?;
    ensure_projection_same_mount(
        &projection_filesystem_identity(&private_root)?,
        &projection_filesystem_identity(parent)?,
    )?;
    let name = PathBuf::from(format!(
        ".locald-projection-write-probe-{}",
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let seed = Path::new("seed");
    let probe = private_root.open_with(seed, &options)?;
    let identity = projection_file_identity(&probe.metadata()?);
    let projection_id = uuid::Uuid::new_v4().to_string();
    let binding = (|| {
        bind_provenance(&probe, &projection_id)?;
        anyhow::ensure!(
            projection_provenance_matches(&probe, &projection_id)?,
            "generated-file project_path provenance probe did not round-trip"
        );
        Ok::<_, anyhow::Error>(())
    })();
    if let Err(error) = binding {
        let metadata = private_root.symlink_metadata(seed)?;
        anyhow::ensure!(
            metadata.is_file()
                && projection_file_identity(&metadata) == identity
                && metadata.len() == 0,
            "private projection write probe changed before exposure; retaining the entry at `{}` for inspection",
            private_path.join(seed).display()
        );
        private_root.remove_file(seed)?;
        return Err(error);
    }
    // Qualify no-replace rename privately before the hard-link operation can
    // expose a name. Keep the canonical seed name stable in the durable journal.
    if let Err(error) = rename(&private_root, seed, &private_root, Path::new("seed-ready")) {
        let metadata = private_root.symlink_metadata(seed)?;
        anyhow::ensure!(
            metadata.is_file()
                && projection_file_identity(&metadata) == identity
                && metadata.len() == 0
                && projection_provenance_matches(&probe, &projection_id)?,
            "private projection write probe changed before exposure; retaining the entry at `{}` for inspection",
            private_path.join(seed).display()
        );
        private_root.remove_file(seed)?;
        return Err(error)
            .context("generated-file project_path requires atomic no-replace quarantine support");
    }
    rename_projection_noreplace(&private_root, Path::new("seed-ready"), &private_root, seed)?;
    let mut seed_options = OpenOptions::new();
    seed_options.read(true);
    #[cfg(unix)]
    seed_options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut selected_seed = private_root.open_with(seed, &seed_options)?;
    anyhow::ensure!(
        projection_file_identity(&selected_seed.metadata()?) == identity
            && projection_provenance_matches(&selected_seed, &projection_id)?
            && projection_contents_match_bounded(
                &mut selected_seed,
                0,
                &Sha256::digest([]).into()
            )?,
        "private projection write probe changed before exposure; retaining `{}`",
        private_path.join(seed).display()
    );
    probe.sync_all()?;
    let quarantine_path = private_path.join("quarantine");
    create_private_probe_directory(&quarantine_path)?;
    let quarantine_root = Dir::open_ambient_dir(&quarantine_path, ambient_authority())?;
    let quarantine_name = PathBuf::from("quarantined-probe");
    let parent_identity = projection_file_identity(&parent.dir_metadata()?);
    let ownership = ProjectionOwnership {
        name: "write-probe".to_owned(),
        projection_id,
        canonical_project_root: parent_path.to_path_buf(),
        project_root_identity: parent_identity.clone(),
        parent_relative_path: PathBuf::new(),
        parent_identity,
        file_name: name.clone(),
        quarantine_root: quarantine_path,
        quarantine_root_identity: projection_file_identity(&quarantine_root.dir_metadata()?),
        relative_path: name.clone(),
        target_quarantine_path: quarantine_name.clone(),
        digest: Sha256::digest([]).into(),
        size: 0,
        identity,
    };
    let mut journal = WriteProbeJournal {
        version: 1,
        attempt_identity: projection_file_identity(&private_root.dir_metadata()?),
        ownership,
        complete: false,
    };
    // The seed pins the owned inode across daemon crashes. Publish only after
    // its provenance and complete recovery authority are durably recorded.
    write_probe_journal(&private_root, &journal, true)?;
    if let Some(root) = private_path.parent() {
        std::fs::File::open(root)?.sync_all()?;
    }
    checkpoint(WriteProbeCheckpoint::Journaled, &journal)?;
    if let Err(error) = hard_link(&private_root, seed, parent, &name) {
        // Failure before publication leaves the public namespace unchanged.
        finish_write_probe(&private_root, &mut journal)?;
        return Err(error)
            .context("generated-file project_path requires hard-link publication support");
    }
    parent.open(".")?.sync_all()?;
    checkpoint(WriteProbeCheckpoint::Exposed, &journal)?;
    if let Err(error) = rename(parent, &name, &quarantine_root, &quarantine_name) {
        // The durable journal retains the exact public entry for startup recovery.
        return Err(error).with_context(|| format!(
            "generated-file project_path requires atomic no-replace quarantine support; write probe recovery is retained at `{}` for public entry `{}`",
            private_path.display(), parent_path.join(&name).display()));
    }
    parent.open(".")?.sync_all()?;
    quarantine_root.open(".")?.sync_all()?;
    checkpoint(WriteProbeCheckpoint::Quarantined, &journal)?;
    cleanup_quarantined_projection_path_with_rename(
        parent,
        &quarantine_root,
        &journal.ownership,
        &name,
        &quarantine_name,
        &mut rename,
    )?;
    checkpoint(WriteProbeCheckpoint::Deleted, &journal)?;
    journal.complete = true;
    write_probe_journal(&private_root, &journal, false)?;
    checkpoint(WriteProbeCheckpoint::Completed, &journal)?;
    finish_write_probe(&private_root, &mut journal)?;
    drop(probe);
    Ok(())
}

fn finish_write_probe(attempt: &Dir, journal: &mut WriteProbeJournal) -> Result<()> {
    if !journal.complete {
        journal.complete = true;
        write_probe_journal(attempt, journal, false)?;
    }
    // A durable completion fence precedes releasing the seed inode. Recovery of
    // a completed attempt only handles private metadata, never a reused public name.
    let mut seed = journal.ownership.clone();
    seed.quarantine_root = journal
        .ownership
        .quarantine_root
        .parent()
        .context("probe attempt root")?
        .to_path_buf();
    seed.quarantine_root_identity = journal.attempt_identity.clone();
    seed.target_quarantine_path = PathBuf::from("seed");
    cleanup_orphaned_projection_quarantine(&seed)?;
    match attempt.symlink_metadata("quarantine") {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir()
                    && projection_file_identity(&metadata)
                        == journal.ownership.quarantine_root_identity,
                "retaining write probe because its quarantine identity changed at `{}`",
                journal.ownership.quarantine_root.display()
            );
            attempt.remove_dir("quarantine")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    anyhow::ensure!(
        attempt
            .entries()?
            .all(|entry| entry.is_ok_and(|entry| entry.file_name() == WRITE_PROBE_JOURNAL)),
        "write-probe attempt retains unrecognized private entries; inspect its journal directory"
    );
    attempt.remove_file(WRITE_PROBE_JOURNAL)?;
    attempt.open(".")?.sync_all()?;
    Ok(())
}

fn read_write_probe_journal(root: &Dir, name: &str) -> Result<WriteProbeJournal> {
    use std::io::Read as _;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut bytes = Vec::new();
    root.open_with(name, &options)?
        .take(MAX_GENERATED_SOURCE_BYTES_U64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_GENERATED_SOURCE_BYTES,
        "write-probe journal exceeds the recovery limit"
    );
    serde_json::from_slice(&bytes)
        .with_context(|| format!("malformed write-probe ownership journal `{name}`"))
}

fn recover_write_probe(path: &Path) -> Result<()> {
    let root = Dir::open_ambient_dir(path, ambient_authority())?;
    if root.entries()?.next().is_none() {
        std::fs::remove_dir(path)?;
        return Ok(());
    }
    let mut journal = read_write_probe_journal(&root, WRITE_PROBE_JOURNAL)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let ownership = &journal.ownership;
    anyhow::ensure!(
        journal.version == 1
            && journal.attempt_identity == projection_file_identity(&root.dir_metadata()?)
            && ownership.quarantine_root == path.join("quarantine")
            && ownership.parent_relative_path.as_os_str().is_empty()
            && ownership.file_name.components().count() == 1
            && matches!(
                ownership.file_name.components().next(),
                Some(Component::Normal(_))
            )
            && ownership.relative_path == ownership.file_name
            && ownership.target_quarantine_path == Path::new("quarantined-probe")
            && ownership.canonical_project_root.is_absolute()
            && ownership.size == 0
            && ownership.digest == <[u8; 32]>::from(Sha256::digest([])),
        "write-probe journal has invalid or changed recovery authority"
    );
    match root.symlink_metadata("ownership.next.json") {
        Ok(_) => {
            let completed = read_write_probe_journal(&root, "ownership.next.json")?;
            let mut expected = journal.clone();
            expected.complete = true;
            anyhow::ensure!(
                completed == expected,
                "retaining mismatched write-probe completion record at `{}`",
                path.join("ownership.next.json").display()
            );
            root.rename("ownership.next.json", &root, WRITE_PROBE_JOURNAL)?;
            root.open(".")?.sync_all()?;
            journal = completed;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let ownership = &journal.ownership;
    if !journal.complete {
        // Unlike a retired service, an incomplete probe keeps its exact parent
        // obligation until it can be inspected; changed roots are actionable.
        open_exact_project_parent(
            &ownership.canonical_project_root,
            &ownership.project_root_identity,
            &ownership.parent_relative_path,
            &ownership.parent_identity,
        )?;
        // Verify the seed without deleting it, pinning the inode throughout any
        // public/quarantine lookup and cleanup after a daemon crash.
        let seed = root.open_with("seed", &options)?;
        ensure_retained_projection(
            projection_file_identity(&seed.metadata()?) == ownership.identity
                && projection_provenance_matches(&seed, &ownership.projection_id)?,
            || "write-probe seed ownership changed; retaining recovery state".to_owned(),
        )?;
        // In-place public edits also edit this hard-linked seed. The shared
        // quarantine validator classifies their contents as retained user data.
        cleanup_owned_projection_path(ownership)?;
    }
    finish_write_probe(&root, &mut journal)?;
    std::fs::remove_dir(path).with_context(|| {
        format!(
            "write-probe attempt retains unrecognized entries at `{}`",
            path.display()
        )
    })?;
    Ok(())
}

async fn recover_write_probes(data_dir: &Path) -> Result<Vec<String>> {
    let root = data_dir.join("projection-write-probes");
    tokio::task::spawn_blocking(move || {
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut errors = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_name().to_string_lossy().starts_with(".write-probe-") { continue; }
            let path = entry.path();
            let result = if entry.file_type()?.is_dir() {
                recover_write_probe(&path)
            } else { anyhow::bail!("write-probe attempt is not a directory at `{}`", path.display()) };
            if let Err(error) = result {
                let message = format!("write-probe recovery retained at `{}`: {error:#}", path.display());
                if error.chain().any(|cause| cause.downcast_ref::<RetainedProjectionError>().is_some()) {
                    tracing::warn!(error = %message, "Retaining write-probe recovery for inspection");
                } else { errors.push(message); }
            }
        }
        Ok::<_, anyhow::Error>(errors)
    }).await.context("write-probe startup recovery task failed")?
}

async fn publish_prepared(
    service_root: &Path,
    key: &ServiceKey,
    bindings: &ServiceRuntimeBindings,
    prepared: &PreparedGeneratedFileSet,
) -> Result<GeneratedFileSet> {
    publish_prepared_with_projection_validator(
        service_root,
        key,
        bindings,
        prepared,
        ensure_projection_same_filesystem,
    )
    .await
}

async fn publish_prepared_with_projection_validator<F>(
    service_root: &Path,
    key: &ServiceKey,
    bindings: &ServiceRuntimeBindings,
    prepared: &PreparedGeneratedFileSet,
    projection_validator: F,
) -> Result<GeneratedFileSet>
where
    F: Fn(&ProjectionFileIdentity, &ProjectionFileIdentity) -> Result<()>,
{
    let generation = uuid::Uuid::new_v4().to_string();
    let staging_dir = service_root.join(format!(".staging-{generation}"));
    let generation_dir = service_root.join(&generation);
    create_private_directory(&staging_dir, false).await?;

    let render_result = render_generation(key, bindings, prepared, &staging_dir).await;
    let rendered = match render_result {
        Ok(rendered) => rendered,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            return Err(error);
        }
    };

    let planned = match plan_projection_files_with_validator(
        &staging_dir,
        &generation,
        prepared,
        &rendered,
        &projection_validator,
    ) {
        Ok(planned) => planned,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            let _ = sync_directory(service_root).await;
            return Err(error);
        }
    };
    let quarantine_root = service_root.join(format!(".projection-quarantine-{generation}"));
    let projections = if planned.is_empty() {
        Vec::new()
    } else {
        let setup = async {
            create_private_directory(&quarantine_root, true).await?;
            sync_directory(service_root).await?;
            let metadata = tokio::fs::symlink_metadata(&quarantine_root).await?;
            Ok::<_, anyhow::Error>(projection_file_identity_from_std(&metadata))
        }
        .await;
        let quarantine_root_identity = match setup {
            Ok(identity) => identity,
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&quarantine_root).await;
                let _ = tokio::fs::remove_dir_all(&staging_dir).await;
                let _ = sync_directory(service_root).await;
                return Err(error);
            }
        };
        planned
            .into_iter()
            .map(|planned| planned.with_quarantine(&quarantine_root, &quarantine_root_identity))
            .collect()
    };
    if !projections.is_empty() {
        let manifest = ProjectionManifest {
            version: PROJECTION_MANIFEST_VERSION,
            generation: generation.clone(),
            projections: projections.clone(),
        };
        if let Err(error) = write_projection_manifest(&staging_dir, &manifest).await {
            let _ = tokio::fs::remove_dir_all(&quarantine_root).await;
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            let _ = sync_directory(service_root).await;
            return Err(error);
        }
        // This manifest is the write-ahead ownership record.  Persist both it and
        // the staging-directory entry before creating any state in the project.
        if let Err(error) = sync_directory(&staging_dir).await {
            let _ = tokio::fs::remove_dir_all(&quarantine_root).await;
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            let _ = sync_directory(service_root).await;
            return Err(error);
        }
        if let Err(error) = sync_directory(service_root).await {
            let _ = tokio::fs::remove_dir_all(&quarantine_root).await;
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            let _ = sync_directory(service_root).await;
            return Err(error);
        }
        if let Err(error) = prepare_projection_files(&staging_dir, &projections, &rendered).await {
            return rollback_or_retain(
                error,
                "projection preparation rollback",
                &staging_dir,
                service_root,
                &projections,
            )
            .await;
        }
    }

    if let Err(error) = sync_directory(&staging_dir).await {
        return rollback_or_retain(
            error,
            "projection staging sync rollback",
            &staging_dir,
            service_root,
            &projections,
        )
        .await;
    }
    if let Err(error) = tokio::fs::rename(&staging_dir, &generation_dir).await {
        let error = anyhow::Error::new(error).context(format!(
            "failed to publish generated-file generation `{}`",
            generation_dir.display()
        ));
        return rollback_or_retain(
            error,
            "projection generation rename rollback",
            &staging_dir,
            service_root,
            &projections,
        )
        .await;
    }
    if let Err(error) = sync_directory(service_root).await {
        return rollback_or_retain(
            error,
            "projection generation publication rollback",
            &generation_dir,
            service_root,
            &projections,
        )
        .await;
    }

    let paths = rendered
        .relative_paths
        .into_iter()
        .map(|(name, relative)| (name, generation_dir.join(relative)))
        .collect();
    Ok(GeneratedFileSet {
        generation_dir,
        paths,
        source_fingerprints: rendered.source_fingerprints,
        projections,
    })
}

async fn rollback_or_retain<T>(
    error: anyhow::Error,
    scope: &str,
    generation_directory: &Path,
    service_root: &Path,
    projections: &[ProjectionOwnership],
) -> Result<T> {
    if let Err(rollback_error) = cleanup_projections(projections).await {
        let source = error.context(format!(
            "{scope} was incomplete; durable ownership state was retained for recovery: {rollback_error:#}"
        ));
        return Err(RetainedGeneratedFileSetError {
            source,
            generated_files: GeneratedFileSet {
                generation_dir: generation_directory.to_path_buf(),
                paths: BTreeMap::new(),
                source_fingerprints: BTreeMap::new(),
                projections: projections.to_vec(),
            },
        }
        .into());
    }
    if let Err(rollback_error) = tokio::fs::remove_dir_all(generation_directory).await {
        return Err(error.context(format!(
            "{scope} removed project projections but could not remove private generation state: {rollback_error}"
        )));
    }
    let _ = sync_directory(service_root).await;
    Err(error)
}

#[cfg(test)]
fn plan_projection_files(
    staging_dir: &Path,
    generation: &str,
    prepared: &PreparedGeneratedFileSet,
    rendered: &RenderedGeneration,
) -> Result<Vec<PlannedProjection>> {
    plan_projection_files_with_validator(
        staging_dir,
        generation,
        prepared,
        rendered,
        &ensure_projection_same_filesystem,
    )
}

fn plan_projection_files_with_validator<F>(
    staging_dir: &Path,
    generation: &str,
    prepared: &PreparedGeneratedFileSet,
    rendered: &RenderedGeneration,
    projection_validator: &F,
) -> Result<Vec<PlannedProjection>>
where
    F: Fn(&ProjectionFileIdentity, &ProjectionFileIdentity) -> Result<()>,
{
    let mut projections = Vec::new();
    for (name, source) in &prepared.sources {
        let Some(projection) = &source.projection else {
            continue;
        };
        let contents = rendered
            .projected_contents
            .get(name)
            .context("rendered projection content is missing")?;
        let projection_id = uuid::Uuid::new_v4().to_string();
        let canonical_relative_path = rendered
            .relative_paths
            .get(name)
            .context("rendered canonical projection path is missing")?;
        let canonical_metadata =
            std::fs::symlink_metadata(staging_dir.join(canonical_relative_path))
                .context("failed to inspect rendered canonical projection")?;
        anyhow::ensure!(
            canonical_metadata.is_file() && !canonical_metadata.file_type().is_symlink(),
            "rendered canonical projection is not a regular file"
        );
        let identity = projection_file_identity_from_std(&canonical_metadata);
        projection_validator(&identity, &projection.parent_identity)?;
        projections.push(PlannedProjection {
            name: name.clone(),
            projection_id: projection_id.clone(),
            canonical_project_root: projection.canonical_project_root.clone(),
            project_root_identity: projection.project_root_identity.clone(),
            parent_relative_path: projection.parent_relative_path.clone(),
            parent_identity: projection.parent_identity.clone(),
            file_name: projection.file_name.clone(),
            relative_path: projection.relative_path.clone(),
            target_quarantine_path: PathBuf::from(format!(
                "{generation}-{projection_id}.target-quarantine"
            )),
            digest: Sha256::digest(contents).into(),
            size: contents.len() as u64,
            identity,
        });
    }
    Ok(projections)
}

impl PlannedProjection {
    fn with_quarantine(
        self,
        quarantine_root: &Path,
        quarantine_root_identity: &ProjectionFileIdentity,
    ) -> ProjectionOwnership {
        ProjectionOwnership {
            name: self.name,
            projection_id: self.projection_id,
            canonical_project_root: self.canonical_project_root,
            project_root_identity: self.project_root_identity,
            parent_relative_path: self.parent_relative_path,
            parent_identity: self.parent_identity,
            file_name: self.file_name,
            quarantine_root: quarantine_root.to_path_buf(),
            quarantine_root_identity: quarantine_root_identity.clone(),
            relative_path: self.relative_path,
            target_quarantine_path: self.target_quarantine_path,
            digest: self.digest,
            size: self.size,
            identity: self.identity,
        }
    }
}

fn ensure_projection_same_filesystem(
    canonical_identity: &ProjectionFileIdentity,
    projection_parent_identity: &ProjectionFileIdentity,
) -> Result<()> {
    #[cfg(unix)]
    {
        anyhow::ensure!(
            canonical_identity.device == projection_parent_identity.device,
            "generated-file project_path requires locald's private data directory and the project to be on the same filesystem"
        );
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (canonical_identity, projection_parent_identity);
        anyhow::bail!("generated-file project_path is supported only on macOS and Linux")
    }
}

async fn prepare_projection_files(
    staging_dir: &Path,
    projections: &[ProjectionOwnership],
    rendered: &RenderedGeneration,
) -> Result<()> {
    for projection in projections {
        let canonical_relative_path = rendered
            .relative_paths
            .get(&projection.name)
            .context("rendered canonical projection path is missing")?
            .clone();
        let contents = rendered
            .projected_contents
            .get(&projection.name)
            .context("rendered projection content is missing")?
            .clone();
        let projection = projection.clone();
        let staging_dir = staging_dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            publish_projection_direct(
                &staging_dir,
                &canonical_relative_path,
                &projection,
                &contents,
            )
        })
        .await
        .context("generated-file projection link task failed")??;
    }
    Ok(())
}

fn publish_projection_direct(
    staging_dir: &Path,
    canonical_relative_path: &Path,
    projection: &ProjectionOwnership,
    contents: &[u8],
) -> Result<()> {
    publish_projection_direct_with_hook(
        staging_dir,
        canonical_relative_path,
        projection,
        contents,
        |_| Ok(()),
    )
}

fn publish_projection_direct_with_hook<F>(
    staging_dir: &Path,
    canonical_relative_path: &Path,
    projection: &ProjectionOwnership,
    contents: &[u8],
    before_link: F,
) -> Result<()>
where
    F: FnOnce(&Dir) -> Result<()>,
{
    publish_projection_direct_with_hooks(
        staging_dir,
        canonical_relative_path,
        projection,
        contents,
        before_link,
        |_| Ok(()),
    )
}

fn publish_projection_direct_with_hooks<F, G>(
    staging_dir: &Path,
    canonical_relative_path: &Path,
    projection: &ProjectionOwnership,
    contents: &[u8],
    before_link: F,
    after_detach: G,
) -> Result<()>
where
    F: FnOnce(&Dir) -> Result<()>,
    G: FnOnce(&Dir) -> Result<()>,
{
    use std::io::Write as _;

    let canonical_root = Dir::open_ambient_dir(staging_dir, ambient_authority())?;
    let parent = open_exact_project_parent(
        &projection.canonical_project_root,
        &projection.project_root_identity,
        &projection.parent_relative_path,
        &projection.parent_identity,
    )?;
    let canonical_metadata = canonical_root.symlink_metadata(canonical_relative_path)?;
    anyhow::ensure!(
        canonical_metadata.is_file()
            && !canonical_metadata.file_type().is_symlink()
            && projection_file_identity(&canonical_metadata) == projection.identity,
        "private canonical generated file identity changed before projection"
    );
    let mut canonical_options = OpenOptions::new();
    canonical_options.read(true);
    #[cfg(unix)]
    canonical_options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let canonical_file = canonical_root.open_with(canonical_relative_path, &canonical_options)?;
    anyhow::ensure!(
        projection_file_identity(&canonical_file.metadata()?) == projection.identity,
        "private canonical generated file identity changed at provenance binding"
    );
    set_projection_provenance(&canonical_file, &projection.projection_id)?;
    canonical_file.sync_all()?;
    before_link(&parent)?;
    let canonical_metadata = canonical_root.symlink_metadata(canonical_relative_path)?;
    anyhow::ensure!(
        canonical_metadata.is_file()
            && !canonical_metadata.file_type().is_symlink()
            && projection_file_identity(&canonical_metadata) == projection.identity,
        "private canonical generated file identity changed at projection selection"
    );
    canonical_root
        .hard_link(canonical_relative_path, &parent, &projection.file_name)
        .with_context(|| {
            format!(
                "refusing to overwrite generated-file project_path `{}`",
                projection
                    .canonical_project_root
                    .join(&projection.relative_path)
                    .display()
            )
        })?;
    parent.open(".")?.sync_all()?;

    // The hard link selected an inode whose identity was durable before the
    // project entry appeared.  Detach the private canonical path onto a fresh
    // private inode before service startup, so project edits cannot mutate the
    // canonical runtime file through the shared seed inode.
    let canonical_name = canonical_relative_path
        .file_name()
        .context("canonical generated path has no file name")?
        .to_string_lossy();
    let detached_path = canonical_relative_path.with_file_name(format!(
        ".{canonical_name}.locald-detach-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut detached = canonical_root.open_with(&detached_path, &options)?;
    detached.write_all(contents)?;
    detached.sync_all()?;
    canonical_root.rename(&detached_path, &canonical_root, canonical_relative_path)?;
    canonical_root.open(".")?.sync_all()?;

    // Detachment is the last publication mutation. Verify the exact visible
    // path only now so an ancestor swap or in-place rewrite during detachment
    // cannot leave a service starting against a detached or modified project
    // entry. Roll back through the pinned parent capability; the quarantine
    // protocol removes only the recorded identity and digest and retains any
    // changed entry for recovery.
    let publication_validation =
        after_detach(&parent).and_then(|()| validate_visible_projection_at_publication(projection));
    if let Err(error) = publication_validation {
        let quarantine_root =
            Dir::open_ambient_dir(&projection.quarantine_root, ambient_authority())?;
        let rollback = cleanup_projection_in_open_parent(
            &parent,
            &quarantine_root,
            projection,
            |_| Ok(()),
            |_| Ok(()),
        );
        return match rollback {
            Ok(()) => Err(error.context(
                "generated-file project_path changed at the final publication boundary; the pinned entry was rolled back",
            )),
            Err(rollback_error) => Err(error.context(format!(
                "generated-file project_path changed at the final publication boundary; pinned-entry rollback retained durable ownership state: {rollback_error:#}"
            ))),
        };
    }
    Ok(())
}

fn validate_visible_projection_at_publication(projection: &ProjectionOwnership) -> Result<()> {
    let parent = open_exact_project_parent(
        &projection.canonical_project_root,
        &projection.project_root_identity,
        &projection.parent_relative_path,
        &projection.parent_identity,
    )?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = parent.open_with(&projection.file_name, &options)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && projection_file_identity(&metadata) == projection.identity,
        "generated-file project_path entry identity changed at the final publication boundary"
    );
    anyhow::ensure!(
        projection_provenance_matches(&file, &projection.projection_id)?,
        "generated-file project_path provenance changed at the final publication boundary"
    );
    anyhow::ensure!(
        projection_contents_match_bounded(&mut file, projection.size, &projection.digest)?,
        "generated-file project_path content changed at the final publication boundary"
    );

    // Re-open through the current project path after reading so a concurrent
    // ancestor or final-entry replacement cannot be hidden by the open file
    // capability used for digest verification.
    let current_parent = open_exact_project_parent(
        &projection.canonical_project_root,
        &projection.project_root_identity,
        &projection.parent_relative_path,
        &projection.parent_identity,
    )?;
    let current_metadata = current_parent.symlink_metadata(&projection.file_name)?;
    anyhow::ensure!(
        current_metadata.is_file()
            && !current_metadata.file_type().is_symlink()
            && projection_file_identity(&current_metadata) == projection.identity,
        "generated-file project_path entry identity changed after final digest verification"
    );
    Ok(())
}

fn projection_file_identity(metadata: &cap_std::fs::Metadata) -> ProjectionFileIdentity {
    ProjectionFileIdentity {
        #[cfg(unix)]
        device: CapMetadataExt::dev(metadata),
        #[cfg(unix)]
        inode: CapMetadataExt::ino(metadata),
        #[cfg(not(unix))]
        unsupported: true,
    }
}

fn projection_contents_match_bounded(
    file: &mut cap_std::fs::File,
    expected_size: u64,
    expected_digest: &[u8; 32],
) -> Result<bool> {
    use std::io::Read as _;

    let mut hasher = Sha256::new();
    let mut remaining = expected_size;
    let mut buffer = [0_u8; 8192];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded digest chunk fits usize");
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            return Ok(false);
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let mut extra = [0_u8; 1];
    if file.read(&mut extra)? != 0 {
        return Ok(false);
    }
    Ok(<[u8; 32]>::from(hasher.finalize()) == *expected_digest)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[allow(unsafe_code)] // File-descriptor xattrs bind provenance to the selected inode.
fn set_projection_provenance(file: &cap_std::fs::File, projection_id: &str) -> Result<()> {
    let name = PROJECTION_PROVENANCE_XATTR.as_ptr().cast();
    let value = projection_id.as_bytes();
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            name,
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            name,
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .context("failed to bind generated-file projection provenance to its selected inode")
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[allow(unsafe_code)] // File-descriptor xattrs verify the already-open selected inode.
fn projection_provenance_matches(file: &cap_std::fs::File, projection_id: &str) -> Result<bool> {
    let mut value = [0_u8; 64];
    let name = PROJECTION_PROVENANCE_XATTR.as_ptr().cast();
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            name,
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            name,
            value.as_mut_ptr().cast(),
            value.len(),
        )
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        #[cfg(target_os = "macos")]
        if error.raw_os_error() == Some(libc::ENOATTR) {
            return Ok(false);
        }
        #[cfg(target_os = "linux")]
        if error.raw_os_error() == Some(libc::ENODATA) {
            return Ok(false);
        }
        if error.raw_os_error() == Some(libc::ERANGE) {
            return Ok(false);
        }
        return Err(error).context("failed to read generated-file projection provenance");
    }
    let size = usize::try_from(result).context("projection provenance length is invalid")?;
    Ok(value.get(..size) == Some(projection_id.as_bytes()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn set_projection_provenance(_file: &cap_std::fs::File, _projection_id: &str) -> Result<()> {
    anyhow::bail!("generated-file projection provenance is supported only on macOS and Linux")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn projection_provenance_matches(_file: &cap_std::fs::File, _projection_id: &str) -> Result<bool> {
    Ok(false)
}

fn open_exact_project_parent(
    canonical_project_root: &Path,
    expected_project_root_identity: &ProjectionFileIdentity,
    parent_relative_path: &Path,
    expected_parent_identity: &ProjectionFileIdentity,
) -> Result<Dir> {
    let current = open_project_parent_nofollow(
        canonical_project_root,
        expected_project_root_identity,
        parent_relative_path,
    )?;
    if projection_file_identity(&current.dir_metadata()?) != *expected_parent_identity {
        return Err(stale_projection_parent_error(format!(
            "generated-file project_path parent identity changed at `{}`",
            canonical_project_root.join(parent_relative_path).display()
        )));
    }
    Ok(current)
}

fn open_project_parent_nofollow(
    canonical_project_root: &Path,
    expected_project_root_identity: &ProjectionFileIdentity,
    parent_relative_path: &Path,
) -> Result<Dir> {
    let root =
        Dir::open_ambient_dir(canonical_project_root, ambient_authority()).map_err(|error| {
            if io_error_proves_stale_projection_parent(&error) {
                stale_projection_parent_error(format!(
                    "generated-file project root is no longer reachable at `{}`: {error}",
                    canonical_project_root.display()
                ))
            } else {
                error.into()
            }
        })?;
    if projection_file_identity(&root.dir_metadata()?) != *expected_project_root_identity {
        return Err(stale_projection_parent_error(format!(
            "generated-file project root identity changed at `{}`",
            canonical_project_root.display()
        )));
    }
    let mut current = root;
    for component in parent_relative_path.components() {
        let Component::Normal(component) = component else {
            anyhow::bail!("generated-file projection parent contains an unsafe component");
        };
        current = open_directory_component_nofollow(&current, component).map_err(|error| {
            if io_error_proves_stale_projection_parent(&error) {
                stale_projection_parent_error(format!(
                    "generated-file project_path parent is no longer reachable at `{}`: {error}",
                    canonical_project_root.join(parent_relative_path).display()
                ))
            } else {
                anyhow::Error::from(error).context(format!(
                    "failed to open generated-file project_path parent `{}` without following symlinks",
                    canonical_project_root.join(parent_relative_path).display()
                ))
            }
        })?;
    }
    Ok(current)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[allow(unsafe_code)] // Converts the owned descriptor returned by openat into a capability.
fn open_directory_component_nofollow(
    parent: &Dir,
    component: &std::ffi::OsStr,
) -> std::io::Result<Dir> {
    use std::os::unix::ffi::OsStrExt as _;

    let component = CString::new(component.as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    // SAFETY: the component C string lives for the call; openat returns a new
    // owned descriptor or -1, and O_NOFOLLOW rejects a final symlink.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh owned descriptor returned by successful openat.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    Ok(Dir::from_std_file(file))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn open_directory_component_nofollow(
    _parent: &Dir,
    _component: &std::ffi::OsStr,
) -> std::io::Result<Dir> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "generated-file project_path is supported only on macOS and Linux",
    ))
}

async fn write_projection_manifest(
    staging_dir: &Path,
    manifest: &ProjectionManifest,
) -> Result<()> {
    write_projection_manifest_with_hook(staging_dir, manifest, || Ok(())).await
}

async fn write_projection_manifest_with_hook<F>(
    staging_dir: &Path,
    manifest: &ProjectionManifest,
    before_publish: F,
) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    let path = staging_dir.join(PROJECTION_MANIFEST_NAME);
    let temporary_path = staging_dir.join(format!(
        ".{PROJECTION_MANIFEST_NAME}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    let result = async {
        match tokio::fs::symlink_metadata(&path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => anyhow::bail!(
                "generated-file projection ownership manifest already exists at `{}`",
                path.display()
            ),
            Err(error) => return Err(error.into()),
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary_path).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        before_publish()?;
        tokio::fs::rename(&temporary_path, &path).await?;
        sync_directory(staging_dir).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary_path).await;
    }
    result
}

async fn cleanup_projections(projections: &[ProjectionOwnership]) -> Result<()> {
    let mut errors = Vec::new();
    for projection in projections {
        if let Err(error) = cleanup_projection(projection).await {
            errors.push(format!("{error:#}"));
        }
    }
    if errors.is_empty() {
        remove_projection_quarantine_roots(projections, &mut errors).await;
    }
    finish_cleanup_errors("generated-file projection cleanup", &errors)
}

async fn cleanup_projection(projection: &ProjectionOwnership) -> Result<()> {
    let projection = projection.clone();
    tokio::task::spawn_blocking(move || cleanup_owned_projection_path(&projection))
        .await
        .context("generated-file projection cleanup task failed")?
}

async fn remove_projection_quarantine_roots(
    projections: &[ProjectionOwnership],
    errors: &mut Vec<String>,
) {
    let quarantine_roots = projections
        .iter()
        .map(|projection| projection.quarantine_root.clone())
        .collect::<BTreeSet<_>>();
    for quarantine_root in quarantine_roots {
        match tokio::fs::remove_dir(&quarantine_root).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!(
                "failed to remove private projection quarantine `{}`: {error}",
                quarantine_root.display()
            )),
        }
    }
}

async fn cleanup_projections_for_startup(projections: &[ProjectionOwnership]) -> Result<bool> {
    let mut errors = Vec::new();
    let mut retained = false;
    for projection in projections {
        match cleanup_projection(projection).await {
            Ok(()) => {}
            Err(error) if error_retains_projection(&error) => {
                retained = true;
                tracing::warn!(
                    project_path = %projection.canonical_project_root.join(&projection.relative_path).display(),
                    error = %format!("{error:#}"),
                    "Retaining modified generated-file projection for recovery while continuing startup"
                );
            }
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    if !retained && errors.is_empty() {
        remove_projection_quarantine_roots(projections, &mut errors).await;
    }
    finish_cleanup_errors("generated-file projection cleanup", &errors)?;
    Ok(!retained)
}

fn finish_cleanup_errors(scope: &str, errors: &[String]) -> Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "{scope} encountered {} error(s): {}",
            errors.len(),
            errors.join("; ")
        )
    }
}

fn cleanup_owned_projection_path(projection: &ProjectionOwnership) -> Result<()> {
    cleanup_owned_projection_path_with_hook(projection, |_| Ok(()))
}

fn cleanup_owned_projection_path_with_hook<F>(
    projection: &ProjectionOwnership,
    after_quarantine: F,
) -> Result<()>
where
    F: FnOnce(&Dir) -> Result<()>,
{
    cleanup_owned_projection_path_with_hooks(projection, |_| Ok(()), after_quarantine)
}

fn cleanup_owned_projection_path_with_hooks<F, G>(
    projection: &ProjectionOwnership,
    before_quarantine: F,
    after_quarantine: G,
) -> Result<()>
where
    F: FnOnce(&Dir) -> Result<()>,
    G: FnOnce(&Dir) -> Result<()>,
{
    let parent = match open_exact_project_parent(
        &projection.canonical_project_root,
        &projection.project_root_identity,
        &projection.parent_relative_path,
        &projection.parent_identity,
    ) {
        Ok(parent) => parent,
        Err(error) if error_proves_stale_projection_parent(&error) => {
            tracing::warn!(
                project_path = %projection.canonical_project_root.join(&projection.relative_path).display(),
                error = %format!("{error:#}"),
                "Retiring stale generated-file projection ownership because the recorded project parent is no longer reachable"
            );
            return cleanup_orphaned_projection_quarantine(projection);
        }
        Err(error) => {
            return Err(retain_inaccessible_projection(error, || {
                format!(
                    "retaining generated-file projection because its project parent is inaccessible at `{}`",
                    projection
                        .canonical_project_root
                        .join(&projection.parent_relative_path)
                        .display()
                )
            }));
        }
    };
    let quarantine_root = match Dir::open_ambient_dir(
        &projection.quarantine_root,
        ambient_authority(),
    ) {
        Ok(root) => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match parent.symlink_metadata(&projection.file_name) {
                Err(path_error) if path_error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Ok(_) => Err(retained_projection_error(format!(
                    "retaining generated-file projection `{}` because its private quarantine root is missing",
                    projection
                        .canonical_project_root
                        .join(&projection.relative_path)
                        .display()
                ))),
                Err(path_error) => Err(path_error.into()),
            };
        }
        Err(error) => return Err(error.into()),
    };
    cleanup_projection_in_open_parent(
        &parent,
        &quarantine_root,
        projection,
        before_quarantine,
        after_quarantine,
    )
}

fn cleanup_orphaned_projection_quarantine(projection: &ProjectionOwnership) -> Result<()> {
    let quarantine_root =
        match Dir::open_ambient_dir(&projection.quarantine_root, ambient_authority()) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
    ensure_retained_projection(
        projection_file_identity(&quarantine_root.dir_metadata()?)
            == projection.quarantine_root_identity,
        || {
            format!(
                "retaining orphaned generated-file projection because its private quarantine root identity changed at `{}`",
                projection.quarantine_root.display()
            )
        },
    )?;
    let quarantine_path = projection.target_quarantine_path.as_path();
    let metadata = match quarantine_root.symlink_metadata(quarantine_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    ensure_retained_projection(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && projection_file_identity(&metadata) == projection.identity,
        || {
            format!(
                "retaining orphaned generated-file projection because its quarantined entry identity changed at `{}`",
                projection.quarantine_root.join(quarantine_path).display()
            )
        },
    )?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = quarantine_root
        .open_with(quarantine_path, &options)
        .map_err(|error| {
            retain_inaccessible_projection(error.into(), || {
                format!(
                    "retaining orphaned generated-file projection because its quarantined entry is unreadable at `{}`",
                    projection.quarantine_root.join(quarantine_path).display()
                )
            })
        })?;
    let opened_metadata = file.metadata().map_err(|error| {
        retain_inaccessible_projection(error.into(), || {
            "retaining orphaned generated-file projection because its quarantined entry is unreadable"
                .to_owned()
        })
    })?;
    ensure_retained_projection(
        opened_metadata.is_file()
            && !opened_metadata.file_type().is_symlink()
            && projection_file_identity(&opened_metadata) == projection.identity,
        || {
            format!(
                "retaining orphaned generated-file projection because its quarantined entry changed while opening `{}`",
                projection.quarantine_root.join(quarantine_path).display()
            )
        },
    )?;
    ensure_retained_projection(
        projection_provenance_matches(&file, &projection.projection_id).map_err(|error| {
            retain_inaccessible_projection(error, || {
                "retaining orphaned generated-file projection because its quarantined provenance is unreadable"
                    .to_owned()
            })
        })?,
        || {
            format!(
                "retaining orphaned generated-file projection because its quarantined provenance changed at `{}`",
                projection.quarantine_root.join(quarantine_path).display()
            )
        },
    )?;
    ensure_retained_projection(
        projection_contents_match_bounded(&mut file, projection.size, &projection.digest).map_err(
            |error| {
                retain_inaccessible_projection(error, || {
                    "retaining orphaned generated-file projection because its quarantined content is unreadable"
                        .to_owned()
                })
            },
        )?,
        || {
            format!(
                "retaining orphaned generated-file projection because its quarantined content changed at `{}`",
                projection.quarantine_root.join(quarantine_path).display()
            )
        },
    )?;
    drop(file);
    quarantine_root.remove_file(quarantine_path)?;
    quarantine_root.open(".")?.sync_all()?;
    Ok(())
}

fn cleanup_projection_in_open_parent<F, G>(
    parent: &Dir,
    quarantine_root: &Dir,
    projection: &ProjectionOwnership,
    before_quarantine: F,
    after_quarantine: G,
) -> Result<()>
where
    F: FnOnce(&Dir) -> Result<()>,
    G: FnOnce(&Dir) -> Result<()>,
{
    ensure_retained_projection(
        projection_file_identity(&quarantine_root.dir_metadata()?)
            == projection.quarantine_root_identity,
        || {
            format!(
                "retaining generated-file projection because its private quarantine root identity changed at `{}`",
                projection.quarantine_root.display()
            )
        },
    )?;
    let quarantine_path = projection.target_quarantine_path.as_path();

    // A prior crash may have happened after the atomic move but before deletion.
    cleanup_quarantined_projection_path(
        parent,
        quarantine_root,
        projection,
        &projection.file_name,
        quarantine_path,
    )?;
    let metadata = match parent.symlink_metadata(&projection.file_name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    ensure_retained_projection(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && projection_file_identity(&metadata) == projection.identity,
        || {
            format!(
                "retaining generated-file projection `{}` because its file identity changed before quarantine",
                projection
                    .canonical_project_root
                    .join(&projection.relative_path)
                    .display()
            )
        },
    )?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = parent
        .open_with(&projection.file_name, &options)
        .map_err(|error| {
            retain_inaccessible_projection(error.into(), || {
                format!(
                    "retaining generated-file projection `{}` because it is unreadable before quarantine",
                    projection.canonical_project_root.join(&projection.relative_path).display()
                )
            })
        })?;
    let opened_metadata = file.metadata().map_err(|error| {
        retain_inaccessible_projection(error.into(), || {
            format!(
                "retaining generated-file projection `{}` because it became unreadable before quarantine",
                projection.canonical_project_root.join(&projection.relative_path).display()
            )
        })
    })?;
    ensure_retained_projection(
        opened_metadata.is_file()
            && !opened_metadata.file_type().is_symlink()
            && projection_file_identity(&opened_metadata) == projection.identity,
        || {
            format!(
                "retaining generated-file projection `{}` because its entry changed while opening before quarantine",
                projection
                    .canonical_project_root
                    .join(&projection.relative_path)
                    .display()
            )
        },
    )?;
    ensure_retained_projection(
        projection_provenance_matches(&file, &projection.projection_id).map_err(|error| {
            retain_inaccessible_projection(error, || {
                format!(
                    "retaining generated-file projection `{}` because its provenance is unreadable before quarantine",
                    projection.canonical_project_root.join(&projection.relative_path).display()
                )
            })
        })?,
        || {
            format!(
                "retaining generated-file projection `{}` because its provenance changed before quarantine",
                projection
                    .canonical_project_root
                    .join(&projection.relative_path)
                    .display()
            )
        },
    )?;
    drop(file);
    before_quarantine(parent)?;
    match rename_projection_noreplace(
        parent,
        &projection.file_name,
        quarantine_root,
        quarantine_path,
    ) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to quarantine generated-file projection `{}`",
                    projection
                        .canonical_project_root
                        .join(&projection.relative_path)
                        .display()
                )
            });
        }
    }
    parent.open(".")?.sync_all()?;
    quarantine_root.open(".")?.sync_all()?;
    after_quarantine(parent)?;
    cleanup_quarantined_projection_path(
        parent,
        quarantine_root,
        projection,
        &projection.file_name,
        quarantine_path,
    )
}

fn cleanup_quarantined_projection_path(
    project_root: &Dir,
    quarantine_root: &Dir,
    projection: &ProjectionOwnership,
    original_path: &Path,
    quarantine_path: &Path,
) -> Result<()> {
    cleanup_quarantined_projection_path_with_rename(
        project_root,
        quarantine_root,
        projection,
        original_path,
        quarantine_path,
        rename_projection_noreplace,
    )
}

fn cleanup_quarantined_projection_path_with_rename<F>(
    project_root: &Dir,
    quarantine_root: &Dir,
    projection: &ProjectionOwnership,
    original_path: &Path,
    quarantine_path: &Path,
    mut rename: F,
) -> Result<()>
where
    F: FnMut(&Dir, &Path, &Dir, &Path) -> std::io::Result<()>,
{
    let metadata = match quarantine_root.symlink_metadata(quarantine_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let validation = (|| -> Result<()> {
        ensure_retained_projection(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            || {
                format!(
                    "retaining generated-file projection `{}` because it is not a regular file",
                    projection.quarantine_root.join(quarantine_path).display()
                )
            },
        )?;
        ensure_retained_projection(
            projection_file_identity(&metadata) == projection.identity,
            || {
                format!(
                    "retaining generated-file projection `{}` because its file identity changed",
                    projection.quarantine_root.join(quarantine_path).display()
                )
            },
        )?;

        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut file = quarantine_root
            .open_with(quarantine_path, &options)
            .map_err(|error| {
                retain_inaccessible_projection(error.into(), || {
                    format!(
                        "retaining generated-file projection `{}` because it is unreadable in quarantine",
                        projection.quarantine_root.join(quarantine_path).display()
                    )
                })
            })?;
        let opened_metadata = file.metadata().map_err(|error| {
            retain_inaccessible_projection(error.into(), || {
                "retaining generated-file projection because it became unreadable in quarantine"
                    .to_owned()
            })
        })?;
        ensure_retained_projection(
            opened_metadata.is_file()
                && !opened_metadata.file_type().is_symlink()
                && projection_file_identity(&opened_metadata) == projection.identity,
            || {
                format!(
                    "retaining generated-file projection `{}` because its entry changed while opening",
                    projection.quarantine_root.join(quarantine_path).display()
                )
            },
        )?;
        ensure_retained_projection(
            projection_provenance_matches(&file, &projection.projection_id).map_err(|error| {
                retain_inaccessible_projection(error, || {
                    "retaining generated-file projection because its quarantined provenance is unreadable"
                        .to_owned()
                })
            })?,
            || {
                format!(
                    "retaining generated-file projection `{}` because its provenance changed",
                    projection.quarantine_root.join(quarantine_path).display()
                )
            },
        )?;
        ensure_retained_projection(
            projection_contents_match_bounded(&mut file, projection.size, &projection.digest)
                .map_err(|error| {
                    retain_inaccessible_projection(error, || {
                        "retaining generated-file projection because its quarantined content is unreadable"
                            .to_owned()
                    })
                })?,
            || {
                format!(
                    "retaining generated-file projection `{}` because its content changed",
                    projection.quarantine_root.join(quarantine_path).display()
                )
            },
        )?;
        Ok(())
    })();

    if let Err(error) = validation {
        match rename(
            quarantine_root,
            quarantine_path,
            project_root,
            original_path,
        ) {
            Ok(()) => return Err(error),
            Err(restore_error) if restore_error.kind() == std::io::ErrorKind::AlreadyExists => {
                let recovered_path =
                    PathBuf::from(format!(".locald-recovered-{}", uuid::Uuid::new_v4()));
                match rename(
                    quarantine_root,
                    quarantine_path,
                    project_root,
                    &recovered_path,
                ) {
                    Ok(()) => {
                        project_root.open(".")?.sync_all()?;
                        quarantine_root.open(".")?.sync_all()?;
                        return Err(error.context(format!(
                            "the quarantined entry changed during cleanup and the original path was occupied; it was returned to the project namespace at `{}`",
                            projection
                                .canonical_project_root
                                .join(&projection.parent_relative_path)
                                .join(&recovered_path)
                                .display()
                        )));
                    }
                    Err(recovery_error) => {
                        return Err(error.context(format!(
                            "the quarantined entry was retained at `{}` because restoration to `{}` failed: {restore_error}; recovery into the project namespace also failed: {recovery_error}",
                            projection.quarantine_root.join(quarantine_path).display(),
                            projection.canonical_project_root.join(original_path).display()
                        )));
                    }
                }
            }
            Err(restore_error) => {
                return Err(error.context(format!(
                    "the quarantined entry was retained at `{}` because restoration to `{}` failed: {restore_error}",
                    projection.quarantine_root.join(quarantine_path).display(),
                    projection.canonical_project_root.join(original_path).display()
                )));
            }
        }
    }

    quarantine_root.remove_file(quarantine_path)?;
    quarantine_root.open(".")?.sync_all()?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[allow(unsafe_code)] // The platform no-replace rename is the atomic ownership boundary.
fn rename_projection_noreplace(
    from_root: &Dir,
    from: &Path,
    to_root: &Dir,
    to: &Path,
) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let from_fd = from_root.as_raw_fd();
    let to_fd = to_root.as_raw_fd();
    #[cfg(target_os = "macos")]
    // SAFETY: both C strings live for the call and both directory descriptors
    // are open capabilities for the recorded source and destination roots.
    let result = unsafe {
        libc::renameatx_np(
            from_fd,
            from.as_ptr(),
            to_fd,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from_fd,
            from.as_ptr(),
            to_fd,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        ) as libc::c_int
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rename_projection_noreplace(
    _from_root: &Dir,
    _from: &Path,
    _to_root: &Dir,
    _to: &Path,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "generated-file project_path cleanup is supported only on macOS and Linux",
    ))
}

fn projection_path_matches_ownership(projection: &ProjectionOwnership) -> Result<bool> {
    projection_path_matches_ownership_with_hook(projection, |_| Ok(()))
}

fn projection_path_matches_ownership_with_hook<F>(
    projection: &ProjectionOwnership,
    after_digest: F,
) -> Result<bool>
where
    F: FnOnce(&Dir) -> Result<()>,
{
    let parent = match open_exact_project_parent(
        &projection.canonical_project_root,
        &projection.project_root_identity,
        &projection.parent_relative_path,
        &projection.parent_identity,
    ) {
        Ok(parent) => parent,
        Err(error) if error_proves_stale_projection_parent(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = match parent.symlink_metadata(&projection.file_name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || projection_file_identity(&metadata) != projection.identity
    {
        return Ok(false);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = parent.open_with(&projection.file_name, &options)?;
    let opened_metadata = file.metadata()?;
    if !opened_metadata.is_file()
        || opened_metadata.file_type().is_symlink()
        || projection_file_identity(&opened_metadata) != projection.identity
        || !projection_provenance_matches(&file, &projection.projection_id)?
    {
        return Ok(false);
    }
    if !projection_contents_match_bounded(&mut file, projection.size, &projection.digest)? {
        return Ok(false);
    }
    after_digest(&parent)?;

    let current_parent = match open_exact_project_parent(
        &projection.canonical_project_root,
        &projection.project_root_identity,
        &projection.parent_relative_path,
        &projection.parent_identity,
    ) {
        Ok(parent) => parent,
        Err(error) if error_proves_stale_projection_parent(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let current_metadata = match current_parent.symlink_metadata(&projection.file_name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(current_metadata.is_file()
        && !current_metadata.file_type().is_symlink()
        && projection_file_identity(&current_metadata) == projection.identity)
}

/// Remove every generated runtime file for one project instance.
#[cfg(test)]
pub(crate) async fn cleanup_instance(
    data_dir: &Path,
    instance_id: ProjectInstanceId,
) -> Result<()> {
    let root = data_dir
        .join("instances")
        .join(instance_id.to_string())
        .join("generated");
    remove_generated_root(&root, Some(instance_id))
        .await
        .with_context(|| {
            format!(
                "failed to remove generated files for project instance {instance_id} at `{}`",
                root.display()
            )
        })
        .map(|_| ())
}

/// Remove ephemeral generated runtime files for every recorded instance root.
pub(crate) async fn cleanup_all_instances(
    data_dir: &Path,
) -> Result<Vec<StartupGeneratedFileCleanup>> {
    let mut errors = recover_write_probes(data_dir).await?;
    let instances_root = data_dir.join("instances");
    let mut entries = match tokio::fs::read_dir(&instances_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            finish_cleanup_errors("generated-file write-probe recovery", &errors)?;
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect generated-file instance roots at `{}`",
                    instances_root.display()
                )
            });
        }
    };

    let mut retained = Vec::new();
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
        let instance_name = entry.file_name();
        let instance_id = instance_name
            .to_str()
            .and_then(|name| name.parse::<ProjectInstanceId>().ok());
        let generated_root = entry.path().join("generated");
        match remove_generated_root(&generated_root, instance_id)
            .await
            .with_context(|| {
                format!(
                    "failed to remove stale generated files at `{}`",
                    generated_root.display()
                )
            }) {
            Ok(mut generated_files) => retained.append(&mut generated_files),
            Err(error) => errors.push(format!("{error:#}")),
        }
    }

    finish_cleanup_errors("generated-file instance recovery", &errors)?;
    Ok(retained)
}

async fn remove_generated_root(
    root: &Path,
    instance_id: Option<ProjectInstanceId>,
) -> Result<Vec<StartupGeneratedFileCleanup>> {
    let metadata = match tokio::fs::symlink_metadata(root).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        tokio::fs::remove_file(root).await?;
        Ok(Vec::new())
    } else {
        let mut errors = Vec::new();
        let mut retained = Vec::new();
        for loaded in load_projection_manifests(root).await? {
            match cleanup_projections_for_startup(&loaded.manifest.projections).await {
                Ok(true) => {
                    if let Err(error) = cleanup_generation_dir(&loaded.generation_dir).await {
                        errors.push(format!(
                            "generation {}: {error:#}",
                            loaded.manifest.generation
                        ));
                    }
                }
                Ok(false) => {
                    retained.push(StartupGeneratedFileCleanup {
                        instance_id,
                        service_resource_ids: BTreeSet::from([loaded.service_resource_id]),
                        generated_files: GeneratedFileSet {
                            generation_dir: loaded.generation_dir,
                            paths: BTreeMap::new(),
                            source_fingerprints: BTreeMap::new(),
                            projections: loaded.manifest.projections,
                        },
                    });
                }
                Err(error) => {
                    errors.push(format!(
                        "generation {}: {error:#}",
                        loaded.manifest.generation
                    ));
                }
            }
        }
        if !errors.is_empty() {
            finish_cleanup_errors("generated-file manifest recovery", &errors)?;
        }
        if retained.is_empty() {
            tokio::fs::remove_dir_all(root).await?;
            Ok(Vec::new())
        } else {
            remove_unretained_generations(root, &retained).await?;
            Ok(retained)
        }
    }
}

async fn remove_unretained_generations(
    root: &Path,
    retained: &[StartupGeneratedFileCleanup],
) -> Result<()> {
    let retained_paths: BTreeSet<_> = retained
        .iter()
        .flat_map(|entry| {
            // Quarantine is a sibling of its generation and remains part of
            // that generation's recovery authority until cleanup succeeds.
            std::iter::once(entry.generated_files.generation_dir.as_path()).chain(
                entry
                    .generated_files
                    .projections
                    .iter()
                    .map(|projection| projection.quarantine_root.as_path()),
            )
        })
        .collect();
    let mut services = tokio::fs::read_dir(root).await?;
    while let Some(service) = services.next_entry().await? {
        let service_path = service.path();
        if retained_paths
            .iter()
            .any(|generation| generation.parent() == Some(service_path.as_path()))
        {
            let mut generations = tokio::fs::read_dir(&service_path).await?;
            while let Some(generation) = generations.next_entry().await? {
                if !retained_paths.contains(generation.path().as_path()) {
                    remove_stale_generated_entry(&generation).await?;
                }
            }
        } else {
            remove_stale_generated_entry(&service).await?;
        }
    }
    Ok(())
}

async fn remove_stale_generated_entry(entry: &tokio::fs::DirEntry) -> Result<()> {
    // DirEntry::file_type does not follow symlinks: cleanup owns only the
    // private entry, never a symlink's destination outside the generated root.
    if entry.file_type().await?.is_dir() {
        tokio::fs::remove_dir_all(entry.path()).await?;
    } else {
        tokio::fs::remove_file(entry.path()).await?;
    }
    Ok(())
}

#[allow(clippy::disallowed_methods)] // This entire directory walk runs inside spawn_blocking.
async fn load_projection_manifests(root: &Path) -> Result<Vec<LoadedProjectionManifest>> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut manifests = Vec::new();
        for service in std::fs::read_dir(&root)? {
            let service = service?;
            let service_type = service.file_type()?;
            if !service_type.is_dir() || service_type.is_symlink() {
                continue;
            }
            for generation in std::fs::read_dir(service.path())? {
                let generation = generation?;
                let generation_type = generation.file_type()?;
                if !generation_type.is_dir() || generation_type.is_symlink() {
                    continue;
                }
                let manifest_path = generation.path().join(PROJECTION_MANIFEST_NAME);
                let bytes = match std::fs::read(&manifest_path) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                let manifest: ProjectionManifest =
                    serde_json::from_slice(&bytes).with_context(|| {
                        format!(
                            "failed to parse generated-file projection ownership manifest `{}`",
                            manifest_path.display()
                        )
                    })?;
                anyhow::ensure!(
                    manifest.version == PROJECTION_MANIFEST_VERSION,
                    "unsupported generated-file projection ownership manifest version {} at `{}`",
                    manifest.version,
                    manifest_path.display()
                );
                let generation_file_name = generation.file_name();
                let directory_name = generation_file_name.to_string_lossy();
                anyhow::ensure!(
                    directory_name == manifest.generation
                        || directory_name == format!(".staging-{}", manifest.generation),
                    "generated-file projection ownership manifest generation does not match `{}`",
                    generation.path().display()
                );
                manifests.push(LoadedProjectionManifest {
                    service_resource_id: service.file_name(),
                    generation_dir: generation.path(),
                    manifest,
                });
            }
        }
        Ok::<_, anyhow::Error>(manifests)
    })
    .await
    .context("generated-file projection recovery scan task failed")?
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
) -> Result<RenderedGeneration> {
    let mut relative_paths = BTreeMap::new();
    let mut source_fingerprints = BTreeMap::new();
    let mut projected_contents = BTreeMap::new();

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
        if source.projection.is_some() {
            projected_contents.insert(name.clone(), rendered);
        }
    }

    Ok(RenderedGeneration {
        relative_paths,
        source_fingerprints,
        projected_contents,
    })
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
    let root_metadata = tokio::fs::symlink_metadata(&canonical_root)
        .await
        .with_context(|| {
            format!(
                "failed to inspect generated-file project root `{}`",
                canonical_root.display()
            )
        })?;
    anyhow::ensure!(
        root_metadata.is_dir(),
        "generated-file project root `{}` is not a directory",
        canonical_root.display()
    );
    #[cfg(unix)]
    let root_identity = {
        use std::os::unix::fs::MetadataExt;

        (root_metadata.dev(), root_metadata.ino())
    };
    #[cfg(not(unix))]
    let root_identity = ();
    let canonical_source = tokio::fs::canonicalize(canonical_root.join(configured))
        .await
        .with_context(|| format!("generated-file source `{}` does not exist", config.source))?;
    anyhow::ensure!(
        canonical_source.starts_with(&canonical_root),
        "generated-file source `{}` resolves outside project root `{}`",
        config.source,
        canonical_root.display()
    );
    let path_metadata = tokio::fs::symlink_metadata(&canonical_source)
        .await
        .with_context(|| {
            format!(
                "failed to inspect generated-file source `{}`",
                canonical_source.display()
            )
        })?;
    anyhow::ensure!(
        path_metadata.is_file(),
        "generated-file source `{}` is not a regular file",
        config.source
    );
    // Open the configured relative path through a directory capability rooted at the
    // canonical project directory. `cap_std` resolves every component beneath that
    // capability, so a replacement of an ancestor after the diagnostic
    // canonicalization above cannot redirect this descriptor outside the project.
    // It still follows supported in-project symlinks while rejecting escapes.
    let capability_root = canonical_root.clone();
    let configured = configured.to_path_buf();
    let source_for_diagnostics = canonical_source.clone();
    let file = tokio::task::spawn_blocking(move || {
        open_source_under_project_capability(&capability_root, &configured, root_identity)
    })
    .await
    .context("generated-file source capability-open task failed")?
    .with_context(|| {
        format!(
            "failed to open generated-file source safely `{}`",
            source_for_diagnostics.display()
        )
    })?;
    let file = tokio::fs::File::from_std(file);
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
        identity: projection_file_identity_from_std(&metadata),
        fingerprint,
        format,
    })
}

async fn prepare_projection_target(
    project_root: &Path,
    configured: &str,
    allowed_existing: Option<&GeneratedFileSet>,
    source_identity: &ProjectionFileIdentity,
) -> Result<PreparedProjection> {
    anyhow::ensure!(
        project_path_supported(),
        "generated-file project_path is supported only on macOS and Linux"
    );
    let relative_path = normalized_relative_path(configured);
    anyhow::ensure!(
        !relative_path.as_os_str().is_empty() && !Path::new(configured).is_absolute(),
        "generated-file project_path must be a non-empty project-relative path"
    );
    anyhow::ensure!(
        !Path::new(configured).components().any(|component| matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )),
        "generated-file project_path may not traverse outside the project"
    );
    source_format(configured)
        .context("generated-file project_path has an unsupported extension")?;

    let canonical_project_root =
        tokio::fs::canonicalize(project_root)
            .await
            .with_context(|| {
                format!(
                    "failed to resolve generated-file project root `{}`",
                    project_root.display()
                )
            })?;
    let root_metadata = tokio::fs::symlink_metadata(&canonical_project_root).await?;
    anyhow::ensure!(
        root_metadata.is_dir() && !root_metadata.file_type().is_symlink(),
        "generated-file project root `{}` is not a regular directory",
        canonical_project_root.display()
    );
    let project_root_identity = projection_file_identity_from_std(&root_metadata);

    let parent_relative_path = relative_path
        .parent()
        .context("generated-file project_path has no parent directory")?
        .to_path_buf();
    let file_name = PathBuf::from(
        relative_path
            .file_name()
            .context("generated-file project_path has no file name")?,
    );
    let root_for_task = canonical_project_root.clone();
    let root_identity_for_task = project_root_identity.clone();
    let parent_for_task = parent_relative_path.clone();
    let file_name_for_task = file_name.clone();
    let (parent_identity, parent_filesystem_identity, target_metadata, selected_entry_name) =
        tokio::task::spawn_blocking(move || {
            let parent = open_project_parent_nofollow(
                &root_for_task,
                &root_identity_for_task,
                &parent_for_task,
            )?;
            let parent_identity = projection_file_identity(&parent.dir_metadata()?);
            let parent_filesystem_identity = projection_filesystem_identity(&parent)?;
            let metadata = match parent.symlink_metadata(&file_name_for_task) {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            let selected_entry_name = metadata
                .as_ref()
                .map(|metadata| {
                    selected_projection_entry_name(&parent, &file_name_for_task, metadata)
                })
                .transpose()?;
            Ok::<_, anyhow::Error>((
                parent_identity,
                parent_filesystem_identity,
                metadata,
                selected_entry_name,
            ))
        })
        .await
        .context("generated-file project_path parent capability task failed")??;
    anyhow::ensure!(
        !target_metadata
            .as_ref()
            .is_some_and(|metadata| projection_file_identity(metadata) == *source_identity),
        "generated-file source resolves to its project_path target; use an independent source file"
    );
    let target_is_owned = match (allowed_existing, target_metadata.as_ref()) {
        (Some(owned), Some(metadata)) => {
            owned
                .owns_projection(
                    &canonical_project_root,
                    &parent_identity,
                    selected_entry_name
                        .as_deref()
                        .context("generated-file project_path selected entry is missing")?,
                    metadata,
                )
                .await
        }
        _ => false,
    };
    let target = canonical_project_root.join(&relative_path);
    match target_metadata {
        Some(_) if target_is_owned => {}
        Some(metadata) => anyhow::bail!(
            "generated-file project_path target `{}` already exists as {}; locald never adopts or overwrites project files",
            target.display(),
            if metadata.file_type().is_symlink() {
                "a symlink"
            } else if metadata.is_file() {
                "a file"
            } else {
                "a non-regular entry"
            }
        ),
        None => {}
    }

    Ok(PreparedProjection {
        canonical_project_root,
        project_root_identity,
        relative_path,
        parent_relative_path,
        parent_identity,
        parent_filesystem_identity,
        file_name,
    })
}

fn canonical_caseless(value: &str) -> String {
    value.nfd().case_fold().nfd().collect()
}

fn selected_projection_entry_name(
    parent: &Dir,
    requested_name: &Path,
    target_metadata: &cap_std::fs::Metadata,
) -> Result<PathBuf> {
    let target_identity = projection_file_identity(target_metadata);
    let mut case_match = None;
    for entry in parent.entries()? {
        let entry = entry?;
        let entry_name = entry.file_name();
        if !entry.file_type()?.is_file() {
            continue;
        }
        let entry_metadata = entry.metadata()?;
        if projection_file_identity(&entry_metadata) != target_identity {
            continue;
        }
        if Path::new(&entry_name) == requested_name {
            return Ok(PathBuf::from(entry_name));
        }
        if entry_name
            .to_str()
            .zip(requested_name.to_str())
            .is_some_and(|(actual, requested)| {
                canonical_caseless(actual) == canonical_caseless(requested)
            })
        {
            anyhow::ensure!(
                case_match.is_none(),
                "generated-file project_path selected multiple case-equivalent directory entries"
            );
            case_match = Some(PathBuf::from(entry_name));
        }
    }
    case_match.context(
        "generated-file project_path changed while locald was selecting its directory entry",
    )
}

const fn project_path_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

fn projection_file_identity_from_std(metadata: &std::fs::Metadata) -> ProjectionFileIdentity {
    ProjectionFileIdentity {
        #[cfg(unix)]
        device: {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        },
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        },
        #[cfg(not(unix))]
        unsupported: true,
    }
}

#[allow(unsafe_code)] // statx binds the comparison to the selected directory mount.
fn projection_filesystem_identity(dir: &Dir) -> Result<ProjectionFilesystemIdentity> {
    let metadata = dir.dir_metadata()?;
    #[cfg(target_os = "linux")]
    let mount_id = {
        let mut stat = std::mem::MaybeUninit::<libc::statx>::zeroed();
        // SAFETY: `dir` owns a valid descriptor, the empty path is NUL-terminated,
        // and `stat` points to writable storage for the duration of the call.
        let result = unsafe {
            libc::statx(
                dir.as_raw_fd(),
                c"".as_ptr(),
                libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
                libc::STATX_MNT_ID,
                stat.as_mut_ptr(),
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to inspect generated-file filesystem mount identity");
        }
        // SAFETY: a successful `statx` initialized the output structure.
        let stat = unsafe { stat.assume_init() };
        anyhow::ensure!(
            stat.stx_mask & libc::STATX_MNT_ID != 0,
            "generated-file filesystem mount identity is unavailable"
        );
        stat.stx_mnt_id
    };
    Ok(ProjectionFilesystemIdentity {
        #[cfg(unix)]
        device: {
            use cap_std::fs::MetadataExt;
            metadata.dev()
        },
        #[cfg(not(unix))]
        device: 0,
        #[cfg(target_os = "linux")]
        mount_id,
    })
}

fn open_source_under_project_capability(
    canonical_root: &Path,
    configured: &Path,
    expected_root_identity: RootIdentity,
) -> std::io::Result<std::fs::File> {
    let root = Dir::open_ambient_dir(canonical_root, ambient_authority())?;
    #[cfg(unix)]
    {
        let opened_root = root.dir_metadata()?;
        if (
            CapMetadataExt::dev(&opened_root),
            CapMetadataExt::ino(&opened_root),
        ) != expected_root_identity
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "generated-file project root changed while opening the source",
            ));
        }
    }
    open_source_from_root_capability(&root, configured)
}

fn open_source_from_root_capability(
    root: &Dir,
    configured: &Path,
) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK);
    root.open_with(configured, &options)
        .map(cap_std::fs::File::into_std)
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
    let references = pattern
        .captures_iter(value)
        .map(|captures| {
            let full = captures
                .get(0)
                .context("generated replacement reference has no complete match")?;
            let body = captures
                .get(1)
                .context("generated replacement reference is missing its body")?
                .as_str();
            let reference = resolve_owned_service_reference(body, service_name)?;
            Ok((full.range(), reference))
        })
        .collect::<Result<Vec<_>>>()?;
    if references.is_empty() {
        return Ok(Value::String(value.to_owned()));
    }

    if references.len() == 1 && references[0].0 == (0..value.len()) {
        let port = resolve_binding_reference(&references[0].1, service_name, bindings)?;
        return Ok(Value::Number(port.into()));
    }

    let mut resolved = value.to_owned();
    let mut replacements = Vec::new();
    for (range, reference) in references {
        let port = resolve_binding_reference(&reference, service_name, bindings)?;
        replacements.push((range, port.to_string()));
    }
    replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, port) in replacements {
        resolved.replace_range(range, &port);
    }
    Ok(Value::String(resolved))
}

fn resolve_binding_reference(
    reference: &ServiceReference,
    service_name: &str,
    bindings: &ServiceRuntimeBindings,
) -> Result<u16> {
    let referenced_service = reference.service_name.as_str();
    let field = reference.field.as_str();
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

fn validate_project_path(
    service_name: &str,
    name: &str,
    source: &str,
    project_path: &str,
) -> Result<()> {
    let path = Path::new(project_path);
    anyhow::ensure!(
        !project_path.trim().is_empty(),
        "service `{service_name}` generated file `{name}` project_path is empty"
    );
    anyhow::ensure!(
        !path.is_absolute(),
        "service `{service_name}` generated file `{name}` project_path must be project-relative"
    );
    anyhow::ensure!(
        !path.components().any(|component| matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )),
        "service `{service_name}` generated file `{name}` project_path may not traverse outside the project"
    );
    source_format(project_path).with_context(|| {
        format!("service `{service_name}` generated file `{name}` has an unsupported project_path")
    })?;
    anyhow::ensure!(
        normalized_relative_path(source) != normalized_relative_path(project_path),
        "service `{service_name}` generated file `{name}` project_path must differ from its source"
    );
    Ok(())
}

fn normalized_relative_path(path: &str) -> PathBuf {
    Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect()
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
                let body = captures
                    .get(1)
                    .context("generated replacement reference is missing its body")?
                    .as_str();
                let reference = resolve_owned_service_reference(body, service_name)?;
                let referenced_service = reference.service_name.as_str();
                let field = reference.field.as_str();
                anyhow::ensure!(
                    referenced_service == service_name,
                    "service `{service_name}` generated file `{name}` references service `{referenced_service}`; generated files may use only their owning service"
                );
                if field == "port" {
                    anyhow::ensure!(
                        ReadinessRequirement::service_requires_port(service),
                        "service `{service_name}` generated file `{name}` references its primary port, but this worker has no configured or probe-assigned primary port"
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
                        project_path: None,
                        replace: replacements,
                    },
                )]),
                ..CommonServiceConfig::default()
            },
            command: Some("true".to_owned()),
            ..ExecServiceConfig::default()
        })
    }

    fn projected_service_config(
        source: &str,
        project_path: &str,
        replacements: BTreeMap<String, Value>,
    ) -> ServiceConfig {
        ServiceConfig::Legacy(ExecServiceConfig {
            common: CommonServiceConfig {
                listeners: vec!["chat".to_owned()],
                generated: BTreeMap::from([(
                    "microfrontends".to_owned(),
                    GeneratedFileConfig {
                        source: source.to_owned(),
                        project_path: Some(project_path.to_owned()),
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

    #[tokio::test]
    async fn preparation_rejects_a_source_symlink_to_its_owned_projection() {
        let root = tempdir().expect("create source-alias root");
        tokio::fs::write(root.path().join("source.json"), r#"{"value":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "runtime.locald.json", BTreeMap::new());
        let generated = materialize(
            &root.path().join("data"),
            root.path(),
            &key(),
            &config,
            &bindings(),
        )
        .await
        .expect("materialize projection")
        .expect("generated set");
        #[cfg(unix)]
        std::os::unix::fs::symlink("runtime.locald.json", root.path().join("alias.json"))
            .expect("alias owned projection");
        #[cfg(not(unix))]
        std::fs::hard_link(
            root.path().join("runtime.locald.json"),
            root.path().join("alias.json"),
        )
        .expect("alias owned projection");
        for target in ["runtime.locald.json", "replacement.locald.json"] {
            let aliased = projected_service_config("alias.json", target, BTreeMap::new());
            let error = prepare(root.path(), &key(), &aliased, Some(&generated))
                .await
                .expect_err("projection output cannot become its own or a successor's source");
            assert!(
                format!("{error:#}").contains("source resolves to a currently owned projection")
            );
        }
        assert!(
            generated.projections_match().await,
            "rejected preparation preserves active output"
        );
        generated.cleanup().await.expect("clean fixture projection");
    }

    #[tokio::test]
    async fn projects_private_generation_with_typed_replacements_and_exact_cleanup() {
        let root = tempdir().expect("create project root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(
            root.path().join("chat/source.jsonc"),
            r#"{"proxy":3000,"chat":3001}"#,
        )
        .await
        .expect("write source");
        let config = projected_service_config(
            "chat/source.jsonc",
            "chat/.microfrontends.locald.json",
            BTreeMap::from([
                (
                    "/proxy".to_owned(),
                    Value::String("${services.web.port}".to_owned()),
                ),
                (
                    "/chat".to_owned(),
                    Value::String("${services.web.listeners.chat.port}".to_owned()),
                ),
            ]),
        );
        let data_dir = root.path().join("data");
        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize projection")
            .expect("generated set");
        let canonical = generated.path("microfrontends").expect("canonical path");
        let projected = root.path().join("chat/.microfrontends.locald.json");
        assert_ne!(canonical, projected);
        let canonical_bytes = tokio::fs::read(canonical).await.expect("read canonical");
        assert_eq!(
            tokio::fs::read(&projected).await.expect("read projection"),
            canonical_bytes
        );
        let value: Value = serde_json::from_slice(&canonical_bytes).expect("valid JSON");
        assert_eq!(value["proxy"], 4100);
        assert_eq!(value["chat"], 4200);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            use std::os::unix::fs::PermissionsExt;
            let canonical_metadata = tokio::fs::metadata(canonical)
                .await
                .expect("canonical metadata");
            let projected_metadata = tokio::fs::metadata(&projected)
                .await
                .expect("projection metadata");
            assert_ne!(
                (canonical_metadata.dev(), canonical_metadata.ino()),
                (projected_metadata.dev(), projected_metadata.ino()),
                "the package projection cannot mutate the private canonical file in place"
            );
            assert_eq!(projected_metadata.permissions().mode() & 0o777, 0o600);
        }
        tokio::fs::write(&projected, b"project-side mutation")
            .await
            .expect("mutate projection in place");
        assert_eq!(
            tokio::fs::read(canonical).await.expect("reread canonical"),
            canonical_bytes,
            "project-side mutation cannot alter private canonical authority"
        );
        tokio::fs::write(&projected, &canonical_bytes)
            .await
            .expect("restore projection fixture");
        generated.cleanup().await.expect("clean generation");
        assert!(!projected.exists());
    }

    async fn projection_intent_fixture(
        root: &Path,
    ) -> (
        PathBuf,
        PathBuf,
        String,
        Vec<ProjectionOwnership>,
        RenderedGeneration,
    ) {
        tokio::fs::create_dir(root.join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "chat/runtime.locald.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let prepared = prepare(root, &key(), &config, None)
            .await
            .expect("prepare")
            .expect("prepared generated set");
        let data_dir = root.join("data");
        let service_root = prepare_service_root(&data_dir, &key())
            .await
            .expect("prepare service root");
        let generation = uuid::Uuid::new_v4().to_string();
        let staging_dir = service_root.join(format!(".staging-{generation}"));
        create_private_directory(&staging_dir, false)
            .await
            .expect("create staging directory");
        let rendered = render_generation(&key(), &bindings(), &prepared, &staging_dir)
            .await
            .expect("render private generation");
        let quarantine_root = service_root.join(".projection-quarantine");
        create_private_directory(&quarantine_root, true)
            .await
            .expect("create private quarantine");
        let quarantine_identity = projection_file_identity_from_std(
            &tokio::fs::symlink_metadata(&quarantine_root)
                .await
                .expect("quarantine metadata"),
        );
        let projections = plan_projection_files(&staging_dir, &generation, &prepared, &rendered)
            .expect("plan projections")
            .into_iter()
            .map(|planned| planned.with_quarantine(&quarantine_root, &quarantine_identity))
            .collect::<Vec<_>>();
        write_projection_manifest(
            &staging_dir,
            &ProjectionManifest {
                version: PROJECTION_MANIFEST_VERSION,
                generation: generation.clone(),
                projections: projections.clone(),
            },
        )
        .await
        .expect("persist ownership intent");
        sync_directory(&staging_dir).await.expect("sync intent");
        sync_directory(&service_root)
            .await
            .expect("sync staging entry");
        (data_dir, staging_dir, generation, projections, rendered)
    }

    #[tokio::test]
    async fn projection_manifest_publication_is_atomic() {
        let root = tempdir().expect("create manifest publication root");
        let staging_dir = root.path().join("staging");
        tokio::fs::create_dir(&staging_dir)
            .await
            .expect("create manifest staging directory");
        let manifest = ProjectionManifest {
            version: PROJECTION_MANIFEST_VERSION,
            generation: "generation".to_owned(),
            projections: Vec::new(),
        };

        let error = write_projection_manifest_with_hook(&staging_dir, &manifest, || {
            anyhow::bail!("injected crash before manifest publication")
        })
        .await
        .expect_err("injected prepublication failure");
        assert!(format!("{error:#}").contains("injected crash"));
        assert!(!staging_dir.join(PROJECTION_MANIFEST_NAME).exists());
        assert!(
            std::fs::read_dir(&staging_dir)
                .expect("read staging directory")
                .next()
                .is_none(),
            "failed publication removes its private temporary manifest"
        );

        write_projection_manifest(&staging_dir, &manifest)
            .await
            .expect("publish manifest atomically");
        let bytes = tokio::fs::read(staging_dir.join(PROJECTION_MANIFEST_NAME))
            .await
            .expect("read published manifest");
        assert_eq!(
            serde_json::from_slice::<ProjectionManifest>(&bytes).expect("parse complete manifest"),
            manifest
        );
    }

    #[test]
    fn projection_digest_validation_is_bounded_by_recorded_size() {
        let root = tempdir().expect("create bounded digest root");
        std::fs::write(root.path().join("projection"), b"expected-and-growing")
            .expect("write oversized projection");
        let directory =
            Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open digest root");
        let mut file = directory.open("projection").expect("open projection");
        let digest: [u8; 32] = Sha256::digest(b"expected").into();
        assert!(
            !projection_contents_match_bounded(&mut file, b"expected".len() as u64, &digest)
                .expect("validate bounded digest"),
            "content beyond the recorded size is rejected after one bounded extra-byte read"
        );
    }

    #[test]
    fn only_missing_or_replaced_project_parents_are_classified_as_stale() {
        assert!(io_error_proves_stale_projection_parent(
            &std::io::Error::from(std::io::ErrorKind::NotFound)
        ));
        assert!(io_error_proves_stale_projection_parent(
            &std::io::Error::from(std::io::ErrorKind::NotADirectory)
        ));
        assert!(io_error_proves_stale_projection_parent(
            &std::io::Error::from_raw_os_error(libc::ELOOP)
        ));
        assert!(!io_error_proves_stale_projection_parent(
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied)
        ));
        assert!(!io_error_proves_stale_projection_parent(
            &std::io::Error::from_raw_os_error(libc::EMFILE)
        ));
        assert!(error_proves_stale_projection_parent(
            &stale_projection_parent_error("stale fixture")
        ));
        assert!(!error_proves_stale_projection_parent(&anyhow::anyhow!(
            "transient fixture"
        )));
    }

    #[tokio::test]
    async fn failed_rollback_returns_retryable_generated_file_ownership() {
        let root = tempdir().expect("create retained rollback root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create projection parent");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
        let data_dir = root.path().join("data");
        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize")
            .expect("generated set");
        let target = root.path().join("chat/runtime.locald.json");
        let owned = tokio::fs::read(&target).await.expect("read owned target");
        tokio::fs::write(&target, b"modified")
            .await
            .expect("modify target");
        let service_root = generated
            .generation_dir
            .parent()
            .expect("generation has service root")
            .to_path_buf();

        let error = rollback_or_retain::<()>(
            anyhow::anyhow!("publication failed"),
            "publication rollback",
            &generated.generation_dir,
            &service_root,
            &generated.projections,
        )
        .await
        .expect_err("modified target prevents rollback");
        let retained = retained_generated_file_set(&error)
            .expect("failed rollback returns independently retryable ownership");
        assert_eq!(retained.generation_dir, generated.generation_dir);
        assert_eq!(retained.projections, generated.projections);

        tokio::fs::write(&target, owned)
            .await
            .expect("restore owned target");
        retained.cleanup().await.expect("retry retained cleanup");
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn same_bytes_and_reused_identity_cannot_adopt_a_replacement() {
        let root = tempdir().expect("create provenance replacement root");
        let (data_dir, staging_dir, _generation, projections, rendered) =
            projection_intent_fixture(root.path()).await;
        let projection = &projections[0];
        publish_projection_direct(
            &staging_dir,
            &rendered.relative_paths["microfrontends"],
            projection,
            &rendered.projected_contents["microfrontends"],
        )
        .expect("publish owned projection");
        let target = root.path().join(&projection.relative_path);
        let original = tokio::fs::read(&target).await.expect("read owned bytes");
        tokio::fs::remove_file(&target)
            .await
            .expect("remove owned projection");
        tokio::fs::write(&target, &original)
            .await
            .expect("write same-byte replacement");

        let mut simulated_reused_identity = projection.clone();
        simulated_reused_identity.identity = projection_file_identity_from_std(
            &std::fs::symlink_metadata(&target).expect("replacement metadata"),
        );
        assert!(
            !projection_path_matches_ownership(&simulated_reused_identity)
                .expect("check replacement provenance"),
            "a same-byte replacement remains foreign even if an inode identity were reused"
        );

        tokio::fs::remove_file(&target)
            .await
            .expect("remove foreign fixture");
        cleanup_all_instances(&data_dir)
            .await
            .expect("recover fixture after foreign removal");
    }

    #[tokio::test]
    async fn stale_project_identity_does_not_block_global_recovery() {
        let root = tempdir().expect("create stale-project recovery root");
        let project_root = root.path().join("project");
        let data_dir = root.path().join("data");
        tokio::fs::create_dir_all(project_root.join("chat"))
            .await
            .expect("create original project");
        tokio::fs::write(project_root.join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write projection source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
        materialize(&data_dir, &project_root, &key(), &config, &bindings())
            .await
            .expect("materialize projection")
            .expect("generated set");

        // Keep the unlinked directories alive so Linux cannot immediately
        // reuse both recorded inodes for the replacement hierarchy.
        let _original_project =
            std::fs::File::open(&project_root).expect("hold original project directory identity");
        let _original_parent = std::fs::File::open(project_root.join("chat"))
            .expect("hold original projection parent identity");
        tokio::fs::remove_dir_all(&project_root)
            .await
            .expect("remove original project");
        tokio::fs::create_dir_all(project_root.join("chat"))
            .await
            .expect("recreate replacement project");
        let replacement = project_root.join("chat/runtime.locald.json");
        tokio::fs::write(&replacement, b"foreign replacement")
            .await
            .expect("write replacement project file");

        cleanup_all_instances(&data_dir)
            .await
            .expect("stale project ownership cannot block global recovery");
        assert_eq!(
            tokio::fs::read(&replacement)
                .await
                .expect("read untouched replacement"),
            b"foreign replacement"
        );
        assert!(
            !data_dir
                .join("instances")
                .join("00000000-0000-4000-8000-000000000001")
                .join("generated")
                .exists()
        );
    }

    #[tokio::test]
    async fn durable_intent_recovers_every_prepublication_projection_phase() {
        for phase in 0..4 {
            let root = tempdir().expect("create crash-phase root");
            let (data_dir, staging_dir, _generation, projections, rendered) =
                projection_intent_fixture(root.path()).await;
            let projection = &projections[0];
            assert!(staging_dir.join(PROJECTION_MANIFEST_NAME).exists());
            assert!(
                !root.path().join(&projection.relative_path).exists(),
                "ownership intent is durable before direct target publication"
            );

            if phase == 1 || phase == 2 {
                let canonical_root = Dir::open_ambient_dir(&staging_dir, ambient_authority())
                    .expect("open private generation capability");
                let project_parent = open_exact_project_parent(
                    &projection.canonical_project_root,
                    &projection.project_root_identity,
                    &projection.parent_relative_path,
                    &projection.parent_identity,
                )
                .expect("open exact project parent");
                let canonical_file = canonical_root
                    .open(&rendered.relative_paths["microfrontends"])
                    .expect("open selected canonical projection");
                set_projection_provenance(&canonical_file, &projection.projection_id)
                    .expect("bind durable projection provenance");
                canonical_file
                    .sync_all()
                    .expect("sync durable projection provenance");
                canonical_root
                    .hard_link(
                        &rendered.relative_paths["microfrontends"],
                        &project_parent,
                        &projection.file_name,
                    )
                    .expect("atomically publish known canonical inode");
                if phase == 2 {
                    project_parent
                        .open(".")
                        .expect("open project package directory")
                        .sync_all()
                        .expect("sync project package directory");
                }
            } else if phase == 3 {
                publish_projection_direct(
                    &staging_dir,
                    &rendered.relative_paths["microfrontends"],
                    projection,
                    &rendered.projected_contents["microfrontends"],
                )
                .expect("complete direct publication");
            }

            cleanup_all_instances(&data_dir)
                .await
                .unwrap_or_else(|error| panic!("recover phase {phase}: {error:#}"));
            assert!(!staging_dir.exists(), "phase {phase} staging state removed");
            assert!(!root.path().join(&projections[0].relative_path).exists());
        }
    }

    #[tokio::test]
    async fn planning_failure_leaves_no_quarantine_directory() {
        let root = tempdir().expect("create planning-failure root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
        let prepared = prepare(root.path(), &key(), &config, None)
            .await
            .expect("prepare")
            .expect("prepared set");
        let data_dir = root.path().join("data");
        let service_root = prepare_service_root(&data_dir, &key())
            .await
            .expect("prepare service root");
        let error = publish_prepared_with_projection_validator(
            &service_root,
            &key(),
            &bindings(),
            &prepared,
            |_, _| anyhow::bail!("injected cross-filesystem planning failure"),
        )
        .await
        .expect_err("injected planning failure");
        assert!(format!("{error:#}").contains("injected cross-filesystem"));
        let entries = std::fs::read_dir(&service_root)
            .expect("read service root")
            .map(|entry| entry.expect("read entry").file_name())
            .collect::<Vec<_>>();
        assert!(
            entries
                .iter()
                .all(|entry| !entry.to_string_lossy().contains("projection-quarantine")),
            "planning rejection precedes private quarantine creation: {entries:?}"
        );
    }

    #[tokio::test]
    async fn direct_publication_never_overwrites_a_boundary_replacement() {
        let root = tempdir().expect("create direct-publication race root");
        let (data_dir, staging_dir, _generation, projections, rendered) =
            projection_intent_fixture(root.path()).await;
        let projection = &projections[0];
        let error = publish_projection_direct_with_hook(
            &staging_dir,
            &rendered.relative_paths["microfrontends"],
            projection,
            &rendered.projected_contents["microfrontends"],
            |parent| {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                use std::io::Write as _;
                let mut foreign = parent.open_with(&projection.file_name, &options)?;
                foreign.write_all(b"foreign before direct link")?;
                Ok(())
            },
        )
        .expect_err("foreign final entry wins no-clobber race");
        assert!(format!("{error:#}").contains("refusing to overwrite"));
        let target = root.path().join(&projection.relative_path);
        assert_eq!(
            tokio::fs::read(&target).await.expect("read foreign target"),
            b"foreign before direct link"
        );
        let project_entries = std::fs::read_dir(target.parent().expect("target parent"))
            .expect("read project package")
            .map(|entry| entry.expect("read project entry").file_name())
            .collect::<Vec<_>>();
        assert!(
            project_entries
                .iter()
                .all(|entry| !entry.to_string_lossy().contains("locald-")
                    || entry == target.file_name().unwrap()),
            "direct publication creates no project temporary entry: {project_entries:?}"
        );
        tokio::fs::remove_file(&target)
            .await
            .expect("remove injected foreign fixture");
        cleanup_all_instances(&data_dir)
            .await
            .expect("recover intent after fixture removal");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn post_detachment_ancestor_swap_rolls_back_the_pinned_projection() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("create direct-publication ancestor race root");
        let (data_dir, staging_dir, _generation, projections, rendered) =
            projection_intent_fixture(root.path()).await;
        let projection = &projections[0];
        let saved_chat = root.path().join("saved-chat");
        let outside = root.path().join("outside");
        tokio::fs::create_dir(&outside)
            .await
            .expect("create outside directory");
        tokio::fs::write(outside.join(&projection.file_name), b"outside foreign")
            .await
            .expect("write outside foreign");

        let error = publish_projection_direct_with_hooks(
            &staging_dir,
            &rendered.relative_paths["microfrontends"],
            projection,
            &rendered.projected_contents["microfrontends"],
            |_| Ok(()),
            |_| {
                std::fs::rename(root.path().join("chat"), &saved_chat)?;
                symlink(&outside, root.path().join("chat"))?;
                Ok(())
            },
        )
        .expect_err("detached pinned parent rejects publication");
        assert!(
            format!("{error:#}").contains("final publication boundary"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            tokio::fs::read(outside.join(&projection.file_name))
                .await
                .expect("outside survives"),
            b"outside foreign"
        );
        assert!(
            !saved_chat.join(&projection.file_name).exists(),
            "the entry published through the detached capability is rolled back"
        );

        tokio::fs::remove_file(root.path().join("chat"))
            .await
            .expect("remove ancestor symlink");
        tokio::fs::rename(&saved_chat, root.path().join("chat"))
            .await
            .expect("restore admitted ancestor");
        cleanup_all_instances(&data_dir)
            .await
            .expect("recover retained private authority");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn post_detachment_same_inode_rewrite_is_retained_for_recovery() {
        let root = tempdir().expect("create direct-publication rewrite race root");
        let (data_dir, staging_dir, _generation, projections, rendered) =
            projection_intent_fixture(root.path()).await;
        let projection = &projections[0];
        let target = root.path().join(&projection.relative_path);
        let owned_contents = rendered.projected_contents["microfrontends"].clone();
        let modified_contents = b"same inode, modified at publication boundary";

        let error = publish_projection_direct_with_hooks(
            &staging_dir,
            &rendered.relative_paths["microfrontends"],
            projection,
            &owned_contents,
            |_| Ok(()),
            |_| {
                std::fs::write(&target, modified_contents)?;
                Ok(())
            },
        )
        .expect_err("same-inode rewrite rejects final publication");
        let message = format!("{error:#}");
        assert!(
            message.contains("content changed at the final publication boundary"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("rollback retained durable ownership state"),
            "modified content cannot be deleted as owned: {message}"
        );
        let target_parent = Dir::open_ambient_dir(
            target.parent().expect("projection has a parent"),
            ambient_authority(),
        )
        .expect("open retained projection parent");
        let metadata = target_parent
            .symlink_metadata(target.file_name().expect("projection has a file name"))
            .expect("stat retained modified target");
        assert_eq!(
            projection_file_identity(&metadata),
            projection.identity,
            "the mutation preserved the originally selected inode"
        );
        assert_eq!(
            tokio::fs::read(&target)
                .await
                .expect("read retained modified projection"),
            modified_contents,
            "cleanup restores modified content instead of deleting it"
        );
        cleanup_all_instances(&data_dir)
            .await
            .expect("modified projection is isolated without blocking global recovery");
        assert!(
            target.exists(),
            "startup recovery retains the modified projection"
        );
        assert!(
            staging_dir.exists(),
            "startup recovery retains its durable ownership manifest"
        );

        tokio::fs::write(&target, &owned_contents)
            .await
            .expect("restore exact owned content");
        cleanup_all_instances(&data_dir)
            .await
            .expect("recover retained authority after exact content restoration");
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn direct_publication_rejects_a_replaced_private_seed_at_selection() {
        let root = tempdir().expect("create private-seed race root");
        let (data_dir, staging_dir, _generation, projections, rendered) =
            projection_intent_fixture(root.path()).await;
        let projection = &projections[0];
        let canonical_relative = &rendered.relative_paths["microfrontends"];
        let saved = staging_dir.join("saved-private-seed");
        let error = publish_projection_direct_with_hook(
            &staging_dir,
            canonical_relative,
            projection,
            &rendered.projected_contents["microfrontends"],
            |_| {
                std::fs::rename(staging_dir.join(canonical_relative), &saved)?;
                std::fs::write(
                    staging_dir.join(canonical_relative),
                    b"foreign private seed",
                )?;
                Ok(())
            },
        )
        .expect_err("replaced private seed is rejected at link selection");
        assert!(format!("{error:#}").contains("identity changed at projection selection"));
        assert!(
            !root.path().join(&projection.relative_path).exists(),
            "replaced seed is never projected"
        );
        std::fs::remove_file(staging_dir.join(canonical_relative))
            .expect("remove replacement fixture");
        std::fs::rename(saved, staging_dir.join(canonical_relative))
            .expect("restore owned private seed");
        cleanup_all_instances(&data_dir)
            .await
            .expect("recover private-seed race fixture");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nested_ancestor_swap_fails_publication_closed() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("create nested publication root");
        let project = root.path().join("project");
        tokio::fs::create_dir_all(project.join("chat/nested"))
            .await
            .expect("create nested parent");
        tokio::fs::write(project.join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "chat/nested/runtime.locald.json",
            BTreeMap::new(),
        );
        let prepared = prepare(&project, &key(), &config, None)
            .await
            .expect("prepare nested projection")
            .expect("prepared set");
        let saved_chat = root.path().join("saved-chat");
        let outside = root.path().join("outside");
        tokio::fs::create_dir_all(outside.join("nested"))
            .await
            .expect("create outside tree");
        tokio::fs::write(
            outside.join("nested/runtime.locald.json"),
            b"outside foreign",
        )
        .await
        .expect("write outside foreign");
        tokio::fs::rename(project.join("chat"), &saved_chat)
            .await
            .expect("move admitted ancestor");
        symlink(&outside, project.join("chat")).expect("replace ancestor with outside symlink");
        let data_dir = root.path().join("data");
        let error = materialize_prepared(&data_dir, &key(), &bindings(), &prepared)
            .await
            .expect_err("swapped ancestor rejects direct publication");
        assert!(format!("{error:#}").contains("no longer reachable"));
        assert_eq!(
            tokio::fs::read(outside.join("nested/runtime.locald.json"))
                .await
                .expect("outside survives"),
            b"outside foreign"
        );
        assert!(!saved_chat.join("nested/runtime.locald.json").exists());
        tokio::fs::remove_file(project.join("chat"))
            .await
            .expect("remove symlink fixture");
        tokio::fs::rename(&saved_chat, project.join("chat"))
            .await
            .expect("restore admitted ancestor");
        cleanup_all_instances(&data_dir)
            .await
            .expect("recover retained private authority");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restoration_uses_the_pinned_parent_after_an_ancestor_swap() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("create restore-swap root");
        tokio::fs::create_dir_all(root.path().join("chat/nested"))
            .await
            .expect("create nested parent");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "chat/nested/runtime.locald.json",
            BTreeMap::new(),
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
        let projection = generated.projections[0].clone();
        let target = root.path().join(&projection.relative_path);
        let original = tokio::fs::read(&target).await.expect("read original");
        tokio::fs::write(&target, b"modified owned inode")
            .await
            .expect("modify target");
        let saved_chat = root.path().join("saved-chat");
        let outside = root.path().join("outside");
        tokio::fs::create_dir_all(outside.join("nested"))
            .await
            .expect("create outside tree");
        tokio::fs::write(
            outside.join("nested/runtime.locald.json"),
            b"outside foreign",
        )
        .await
        .expect("write outside foreign");
        let error = cleanup_owned_projection_path_with_hook(&projection, |_| {
            std::fs::rename(root.path().join("chat"), &saved_chat)?;
            symlink(&outside, root.path().join("chat"))?;
            Ok(())
        })
        .expect_err("modified target is restored through pinned parent");
        assert!(format!("{error:#}").contains("content changed"));
        assert_eq!(
            tokio::fs::read(outside.join("nested/runtime.locald.json"))
                .await
                .expect("outside survives"),
            b"outside foreign"
        );
        assert_eq!(
            tokio::fs::read(saved_chat.join("nested/runtime.locald.json"))
                .await
                .expect("restored into pinned original parent"),
            b"modified owned inode"
        );
        tokio::fs::remove_file(root.path().join("chat"))
            .await
            .expect("remove symlink fixture");
        tokio::fs::rename(&saved_chat, root.path().join("chat"))
            .await
            .expect("restore ancestor");
        tokio::fs::write(&target, original)
            .await
            .expect("restore target contents");
        generated.cleanup().await.expect("finish cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn projection_requires_private_and_project_paths_on_the_same_filesystem() {
        let canonical = ProjectionFileIdentity {
            device: 1,
            inode: 10,
        };
        let same_project = ProjectionFileIdentity {
            device: 1,
            inode: 20,
        };
        let other_project = ProjectionFileIdentity {
            device: 2,
            inode: 20,
        };
        ensure_projection_same_filesystem(&canonical, &same_project)
            .expect("same-filesystem projection supported");
        let error = ensure_projection_same_filesystem(&canonical, &other_project)
            .expect_err("cross-filesystem projection rejected");
        assert!(error.to_string().contains("same filesystem"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preparation_uses_the_projection_parent_filesystem_boundary() {
        let root = tempdir().expect("create projection-filesystem root");
        tokio::fs::create_dir(root.path().join("nested"))
            .await
            .expect("create projection parent");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "nested/runtime.locald.json", BTreeMap::new());
        let mut prepared = prepare(root.path(), &key(), &config, None)
            .await
            .expect("prepare projection")
            .expect("prepared set");
        let projection = prepared
            .sources
            .values_mut()
            .next()
            .and_then(|source| source.projection.as_mut())
            .expect("prepared projection");
        let private_identity = projection.parent_filesystem_identity.clone();
        projection.parent_filesystem_identity.device = private_identity.device.wrapping_add(1);

        let error = validate_prepared_projection_filesystems(&private_identity, &prepared)
            .expect_err(
                "nested cross-filesystem projection parent is rejected before materialization",
            );
        assert!(error.to_string().contains("same filesystem"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preparation_rejects_a_non_writable_projection_parent() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().expect("create projection-write root");
        let parent = root.path().join("nested");
        tokio::fs::create_dir(&parent)
            .await
            .expect("create projection parent");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "nested/runtime.locald.json", BTreeMap::new());
        let original_permissions = std::fs::metadata(&parent)
            .expect("inspect projection parent")
            .permissions();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555))
            .expect("make projection parent non-writable");

        let result = prepare_for_materialization(
            &root.path().join("data"),
            root.path(),
            &key(),
            &config,
            None,
        )
        .await;
        std::fs::set_permissions(&parent, original_permissions)
            .expect("restore projection parent permissions");

        let error = result.expect_err("non-writable parent is rejected before materialization");
        assert!(format!("{error:#}").contains("cannot create projection entries"));
        assert!(
            std::fs::read_dir(&parent)
                .expect("read restored projection parent")
                .next()
                .is_none(),
            "the write probe leaves no project entry behind"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn projection_preflight_exercises_provenance_and_atomic_quarantine() {
        let root = tempdir().expect("create projection capability root");
        let private = tempdir().expect("create private probe root");
        let parent = Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
        verify_projection_parent_write(&parent, private.path(), root.path())
            .expect("supported filesystem passes preflight");
        assert_eq!(parent.entries().expect("inspect probe cleanup").count(), 0);
        assert_eq!(
            std::fs::read_dir(private.path())
                .expect("inspect private cleanup")
                .count(),
            0
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn projection_preflight_rejects_unsupported_provenance_and_cleans_probe() {
        let root = tempdir().expect("create unsupported provenance root");
        let private = tempdir().expect("create private probe root");
        let parent = Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
        let error = verify_projection_parent_write_with_operations(
            &parent,
            private.path(),
            root.path(),
            |_, _| Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP).into()),
            |_, _, _, _| unreachable!("quarantine follows successful provenance verification"),
        )
        .expect_err("unsupported xattrs reject projection admission");
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::EOPNOTSUPP)
        );
        assert_eq!(parent.entries().expect("inspect probe cleanup").count(), 0);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn projection_preflight_verifies_provenance_round_trip() {
        let root = tempdir().expect("create mismatched provenance root");
        let private = tempdir().expect("create private probe root");
        let parent = Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
        let error = verify_projection_parent_write_with_operations(
            &parent,
            private.path(),
            root.path(),
            |file, _| set_projection_provenance(file, "different-projection"),
            |_, _, _, _| unreachable!("quarantine follows successful provenance verification"),
        )
        .expect_err("mismatched provenance rejects projection admission");
        assert!(
            error
                .to_string()
                .contains("provenance probe did not round-trip")
        );
        assert_eq!(parent.entries().expect("inspect probe cleanup").count(), 0);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn projection_preflight_rejects_unavailable_atomic_quarantine_and_cleans_probe() {
        for errno in [libc::ENOSYS, libc::EOPNOTSUPP, libc::EPERM, libc::EINVAL] {
            let root = tempdir().expect("create unsupported quarantine root");
            let private = tempdir().expect("create private probe root");
            let parent =
                Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
            let error = verify_projection_parent_write_with_operations(
                &parent,
                private.path(),
                root.path(),
                set_projection_provenance,
                |_, _, _, _| Err(std::io::Error::from_raw_os_error(errno)),
            )
            .expect_err("unavailable atomic rename rejects projection admission");
            assert!(
                error
                    .to_string()
                    .contains("requires atomic no-replace quarantine support")
            );
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .and_then(std::io::Error::raw_os_error),
                Some(errno)
            );
            assert_eq!(parent.entries().expect("inspect probe cleanup").count(), 0);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn projection_preflight_rejects_hard_link_denial_before_public_exposure() {
        let root = tempdir().expect("public root");
        let private = tempdir().expect("attempt root");
        let parent = Dir::open_ambient_dir(root.path(), ambient_authority()).expect("parent");
        let error = verify_projection_parent_write_with_checkpoints(
            &parent,
            private.path(),
            root.path(),
            set_projection_provenance,
            |_, _, _, _| Err(std::io::Error::from_raw_os_error(libc::EPERM)),
            rename_projection_noreplace,
            |_, _| Ok(()),
        )
        .expect_err("hard-link policy rejects admission");
        assert!(format!("{error:#}").contains("hard-link publication support"));
        assert_eq!(parent.entries().expect("public entries").count(), 0);
        assert_eq!(
            std::fs::read_dir(private.path())
                .expect("private entries")
                .count(),
            0
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn write_probe_startup_recovers_every_durable_checkpoint_without_instance_roots() {
        for stop in [
            WriteProbeCheckpoint::Journaled,
            WriteProbeCheckpoint::Exposed,
            WriteProbeCheckpoint::Quarantined,
            WriteProbeCheckpoint::Deleted,
            WriteProbeCheckpoint::Completed,
        ] {
            let root = tempdir().expect("recovery root");
            let project = root.path().join("project");
            std::fs::create_dir(&project).expect("project");
            let data = root.path().join("data");
            let attempt = data.join("projection-write-probes/.write-probe-test");
            create_private_directory(&attempt, true)
                .await
                .expect("attempt");
            let parent = Dir::open_ambient_dir(&project, ambient_authority()).expect("parent");
            let mut ownership = None;
            verify_projection_parent_write_with_checkpoints(
                &parent,
                &attempt,
                &project,
                set_projection_provenance,
                |from, source, to, target| from.hard_link(source, to, target),
                rename_projection_noreplace,
                |checkpoint, journal| {
                    if checkpoint == stop {
                        ownership = Some(journal.ownership.clone());
                        anyhow::bail!("simulated daemon exit");
                    }
                    Ok(())
                },
            )
            .expect_err("stop at durable boundary");
            let ownership = ownership.expect("checkpoint observed");
            assert!(attempt.join(WRITE_PROBE_JOURNAL).exists());
            assert!(!data.join("instances").exists());
            if stop == WriteProbeCheckpoint::Completed {
                parent
                    .write(&ownership.file_name, b"new public occupant")
                    .expect("name reuse after completion");
            }
            cleanup_all_instances(&data)
                .await
                .expect("startup recovers probe without instances");
            cleanup_all_instances(&data)
                .await
                .expect("repeated recovery is idempotent");
            assert!(!attempt.exists());
            if stop == WriteProbeCheckpoint::Completed {
                assert_eq!(
                    parent
                        .read(&ownership.file_name)
                        .expect("new occupant survives"),
                    b"new public occupant"
                );
            } else {
                assert_eq!(parent.entries().expect("public entries").count(), 0);
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn write_probe_recovery_preserves_changed_authority_and_contents() {
        for change in [
            "parent",
            "quarantine",
            "public-modified",
            "quarantine-modified",
            "journal",
            "version",
            "extra",
        ] {
            let root = tempdir().expect("recovery root");
            let project = root.path().join("project");
            std::fs::create_dir(&project).expect("project");
            let data = root.path().join("data");
            let attempt = data.join("projection-write-probes/.write-probe-test");
            create_private_directory(&attempt, true)
                .await
                .expect("attempt");
            let parent = Dir::open_ambient_dir(&project, ambient_authority()).expect("parent");
            let mut ownership = None;
            let stop = if change == "quarantine-modified" {
                WriteProbeCheckpoint::Quarantined
            } else {
                WriteProbeCheckpoint::Exposed
            };
            verify_projection_parent_write_with_checkpoints(
                &parent,
                &attempt,
                &project,
                set_projection_provenance,
                |from, source, to, target| from.hard_link(source, to, target),
                rename_projection_noreplace,
                |checkpoint, journal| {
                    if checkpoint == stop {
                        ownership = Some(journal.ownership.clone());
                        anyhow::bail!("simulated daemon exit");
                    }
                    Ok(())
                },
            )
            .expect_err("stop with journal");
            let ownership = ownership.expect("ownership");
            match change {
                "parent" => {
                    std::fs::rename(&project, root.path().join("original-project"))
                        .expect("move parent");
                    std::fs::create_dir(&project).expect("replacement parent");
                    std::fs::write(
                        project.join(&ownership.file_name),
                        b"foreign parent contents",
                    )
                    .expect("foreign file");
                }
                "quarantine" => {
                    std::fs::rename(
                        attempt.join("quarantine"),
                        attempt.join("original-quarantine"),
                    )
                    .expect("move quarantine");
                    std::fs::create_dir(attempt.join("quarantine"))
                        .expect("replacement quarantine");
                    std::fs::write(
                        attempt.join("quarantine/quarantined-probe"),
                        b"foreign quarantine contents",
                    )
                    .expect("foreign file");
                }
                "public-modified" => parent
                    .write(&ownership.file_name, b"edited user data")
                    .expect("modify public inode"),
                "quarantine-modified" => std::fs::write(
                    attempt.join("quarantine/quarantined-probe"),
                    b"edited user data",
                )
                .expect("modify quarantined inode"),
                "journal" => std::fs::write(attempt.join(WRITE_PROBE_JOURNAL), b"broken json")
                    .expect("malformed journal"),
                "version" => {
                    let mut journal: WriteProbeJournal = serde_json::from_slice(
                        &std::fs::read(attempt.join(WRITE_PROBE_JOURNAL)).expect("journal"),
                    )
                    .expect("decode");
                    journal.version = 99;
                    std::fs::write(
                        attempt.join(WRITE_PROBE_JOURNAL),
                        serde_json::to_vec(&journal).expect("encode"),
                    )
                    .expect("write version");
                }
                "extra" => {
                    std::fs::write(attempt.join("user-note"), b"preserve note").expect("extra file")
                }
                _ => unreachable!(),
            }
            let error = recover_write_probe(&attempt)
                .expect_err("changed state retains an actionable journal");
            assert!(
                attempt.join(WRITE_PROBE_JOURNAL).exists(),
                "journal retained for {change}: {error:#}"
            );
            match change {
                "public-modified" | "quarantine-modified" => {
                    assert_eq!(
                        parent
                            .read(&ownership.file_name)
                            .expect("edited contents remain public"),
                        b"edited user data"
                    );
                    cleanup_all_instances(&data)
                        .await
                        .expect("retained user edits are warnings, not fatal startup failures");
                }
                "parent" => assert_eq!(
                    std::fs::read(project.join(&ownership.file_name)).expect("foreign parent file"),
                    b"foreign parent contents"
                ),
                "quarantine" => assert_eq!(
                    std::fs::read(attempt.join("quarantine/quarantined-probe"))
                        .expect("foreign quarantine file"),
                    b"foreign quarantine contents"
                ),
                "extra" => assert_eq!(
                    std::fs::read(attempt.join("user-note")).expect("retained note"),
                    b"preserve note"
                ),
                _ => assert!(
                    cleanup_all_instances(&data).await.is_err(),
                    "invalid journal is reported"
                ),
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn write_probe_recovery_promotes_only_exact_completion_records_and_removes_empty_attempts()
     {
        for record in ["exact", "partial", "mismatched"] {
            let root = tempdir().expect("recovery root");
            let project = root.path().join("project");
            std::fs::create_dir(&project).expect("project");
            let data = root.path().join("data");
            let attempt = data.join("projection-write-probes/.write-probe-test");
            create_private_directory(&attempt, true)
                .await
                .expect("attempt");
            let empty = data.join("projection-write-probes/.write-probe-empty");
            create_private_directory(&empty, false)
                .await
                .expect("empty early attempt");
            let parent = Dir::open_ambient_dir(&project, ambient_authority()).expect("parent");
            let mut ownership = None;
            verify_projection_parent_write_with_checkpoints(
                &parent,
                &attempt,
                &project,
                set_projection_provenance,
                |from, source, to, target| from.hard_link(source, to, target),
                rename_projection_noreplace,
                |checkpoint, journal| {
                    if checkpoint == WriteProbeCheckpoint::Deleted {
                        ownership = Some(journal.ownership.clone());
                        let mut complete = journal.clone();
                        complete.complete = true;
                        if record == "mismatched" {
                            complete.ownership.projection_id = "foreign".to_owned();
                        }
                        let bytes = if record == "partial" {
                            b"{partial".to_vec()
                        } else {
                            serde_json::to_vec(&complete)?
                        };
                        std::fs::write(attempt.join("ownership.next.json"), bytes)?;
                        std::fs::File::open(attempt.join("ownership.next.json"))?.sync_all()?;
                        anyhow::bail!("crash before completion rename");
                    }
                    Ok(())
                },
            )
            .expect_err("completion interruption");
            let ownership = ownership.expect("deleted checkpoint");
            parent
                .write(&ownership.file_name, b"later user file")
                .expect("reuse public name");
            let result = cleanup_all_instances(&data).await;
            assert!(
                !empty.exists(),
                "empty attempt recovered regardless of another journal error"
            );
            assert_eq!(
                parent
                    .read(&ownership.file_name)
                    .expect("user file preserved"),
                b"later user file"
            );
            if record == "exact" {
                result.expect("exact completion record recovered");
                assert!(!attempt.exists());
                cleanup_all_instances(&data).await.expect("repeat recovery");
            } else {
                let error = result.expect_err("unverified completion is retained");
                assert!(format!("{error:#}").contains("ownership.next.json"));
                assert!(attempt.join(WRITE_PROBE_JOURNAL).exists());
                assert!(attempt.join("ownership.next.json").exists());
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn projection_preflight_preserves_replaced_and_modified_entries_at_every_return_boundary() {
        for scenario in [
            "replaced",
            "modified",
            "collision",
            "return-failure",
            "restore-failure",
            "recovery-failure",
        ] {
            let root = tempdir().expect("create public probe fixture");
            let private = tempdir().expect("create private probe fixture");
            let parent =
                Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
            let mut calls = 0;
            let mut public_name = PathBuf::new();
            let error = verify_projection_parent_write_with_checkpoints(
                &parent,
                private.path(),
                root.path(),
                set_projection_provenance,
                |from_root, from, to_root, to| from_root.hard_link(from, to_root, to),
                |from_root, from, to_root, to| {
                    let step = calls;
                    calls += 1;
                    if (step == 1 && scenario == "return-failure")
                        || (step == 2 && scenario == "restore-failure")
                        || (step == 3 && scenario == "recovery-failure")
                    {
                        return Err(std::io::Error::from_raw_os_error(libc::EACCES));
                    }
                    rename_projection_noreplace(from_root, from, to_root, to)?;
                    Ok(())
                },
                |step, journal| {
                    if step == WriteProbeCheckpoint::Exposed {
                        public_name = journal.ownership.file_name.clone();
                        if scenario != "modified" {
                            parent.remove_file(&public_name)?;
                        }
                        parent.write(&public_name, b"user contents")?;
                    }
                    if step == WriteProbeCheckpoint::Quarantined
                        && matches!(scenario, "collision" | "recovery-failure")
                    {
                        parent.write(&public_name, b"new occupant")?;
                    }
                    Ok(())
                },
            )
            .expect_err("changed probes remain user-owned entries");
            let public = root.path().join(&public_name);
            match scenario {
                "restore-failure" | "recovery-failure" => {
                    assert_eq!(
                        std::fs::read(private.path().join("quarantine/quarantined-probe"))
                            .expect("retained private contents"),
                        b"user contents"
                    );
                    assert!(format!("{error:#}").contains(&private.path().display().to_string()));
                    if scenario == "recovery-failure" {
                        assert_eq!(
                            std::fs::read(public).expect("public occupant preserved"),
                            b"new occupant"
                        );
                    } else {
                        assert!(!public.exists());
                    }
                }
                "collision" => {
                    assert_eq!(
                        std::fs::read(public).expect("public occupant preserved"),
                        b"new occupant"
                    );
                    let recovered = std::fs::read_dir(root.path())
                        .expect("read recovered entries")
                        .map(|entry| entry.expect("entry").path())
                        .find(|path| {
                            path.file_name().is_some_and(|name| {
                                name.to_string_lossy().starts_with(".locald-recovered-")
                            })
                        })
                        .expect("foreign entry recovered beside occupied public path");
                    assert_eq!(
                        std::fs::read(recovered).expect("read recovered content"),
                        b"user contents"
                    );
                }
                _ => assert_eq!(
                    std::fs::read(public).expect("user entry returned or retained"),
                    b"user contents"
                ),
            }
            if !matches!(scenario, "restore-failure" | "recovery-failure") {
                assert_eq!(
                    std::fs::read_dir(private.path().join("quarantine"))
                        .expect("read empty quarantine")
                        .count(),
                    0
                );
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn projection_preflight_preserves_a_private_name_replaced_before_exposure() {
        let root = tempdir().expect("create public probe fixture");
        let private = tempdir().expect("create private probe fixture");
        let parent = Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
        let error = verify_projection_parent_write_with_operations(
            &parent,
            private.path(),
            root.path(),
            |_, _| {
                let entry = std::fs::read_dir(private.path())?
                    .next()
                    .expect("private probe")?
                    .path();
                std::fs::remove_file(&entry)?;
                std::fs::write(&entry, b"private replacement")?;
                anyhow::bail!("injected provenance failure")
            },
            |_, _, _, _| unreachable!("failed binding never exposes a public entry"),
        )
        .expect_err("private replacement retained");
        assert!(format!("{error:#}").contains("private projection write probe changed"));
        let entry = std::fs::read_dir(private.path())
            .expect("private entries")
            .next()
            .expect("retained entry")
            .expect("entry")
            .path();
        assert_eq!(
            std::fs::read(entry).expect("read retained entry"),
            b"private replacement"
        );
        assert_eq!(parent.entries().expect("public entries").count(), 0);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn projection_preflight_retained_foreign_quarantine_survives_startup_generation_cleanup()
    {
        let root = tempdir().expect("create recovery fixture");
        let data = root.path().join("data");
        let private = data.join("projection-write-probes/.write-probe-retained-attempt");
        create_private_directory(&private, true)
            .await
            .expect("private quarantine");
        let stale = service_root_path(&data, &key()).join("stale-generation");
        create_private_directory(&stale, true)
            .await
            .expect("stale generated directory");
        tokio::fs::write(stale.join("stale.json"), b"{}")
            .await
            .expect("stale generated output");
        let parent = Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
        let mut calls = 0;
        verify_projection_parent_write_with_checkpoints(
            &parent,
            &private,
            root.path(),
            set_projection_provenance,
            |from_root, from, to_root, to| from_root.hard_link(from, to_root, to),
            |from_root, from, to_root, to| {
                let step = calls;
                calls += 1;
                if step == 2 {
                    return Err(std::io::Error::from_raw_os_error(libc::EACCES));
                }
                rename_projection_noreplace(from_root, from, to_root, to)?;
                Ok(())
            },
            |step, journal| {
                if step == WriteProbeCheckpoint::Exposed {
                    parent.remove_file(&journal.ownership.file_name)?;
                    parent.write(&journal.ownership.file_name, b"retained user data")?;
                }
                Ok(())
            },
        )
        .expect_err("failed restoration keeps foreign entry quarantined");
        cleanup_all_instances(&data).await.expect("startup cleanup");
        assert!(
            !stale.exists(),
            "startup cleanup exercised stale generation removal"
        );
        let restored = std::fs::read_dir(root.path())
            .expect("public entries")
            .map(|entry| entry.expect("entry").path())
            .find(|path| {
                path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with(".locald-projection-write-probe-")
                })
            })
            .expect("foreign quarantine restored with ownership checks");
        assert_eq!(
            tokio::fs::read(restored).await.expect("restored content"),
            b"retained user data"
        );
        assert!(
            private.join(WRITE_PROBE_JOURNAL).exists(),
            "retained disposition remains actionable"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn preparation_rejects_a_distinct_mount_with_the_same_device() {
        let root = tempdir().expect("create projection-mount root");
        tokio::fs::create_dir(root.path().join("nested"))
            .await
            .expect("create projection parent");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "nested/runtime.locald.json", BTreeMap::new());
        let mut prepared = prepare(root.path(), &key(), &config, None)
            .await
            .expect("prepare projection")
            .expect("prepared set");
        let projection = prepared
            .sources
            .values_mut()
            .next()
            .and_then(|source| source.projection.as_mut())
            .expect("prepared projection");
        let private_identity = projection.parent_filesystem_identity.clone();
        projection.parent_filesystem_identity.mount_id = private_identity.mount_id.wrapping_add(1);

        let error = validate_prepared_projection_filesystems(&private_identity, &prepared)
            .expect_err("a distinct bind mount is rejected before materialization");
        assert!(error.to_string().contains("same filesystem mount"));
    }

    #[tokio::test]
    async fn cleanup_quarantine_preserves_a_foreign_replacement_at_the_original_path() {
        let root = tempdir().expect("create replacement-race root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
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
        let projection = generated.projections[0].clone();
        cleanup_owned_projection_path_with_hook(&projection, |capability_root| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut foreign = capability_root.open_with(&projection.file_name, &options)?;
            use std::io::Write as _;
            foreign.write_all(b"foreign replacement")?;
            Ok(())
        })
        .expect("delete only quarantined owned inode");
        assert_eq!(
            tokio::fs::read(root.path().join(&projection.relative_path))
                .await
                .expect("foreign replacement survives"),
            b"foreign replacement"
        );
        assert!(
            !projection
                .quarantine_root
                .join(&projection.target_quarantine_path)
                .exists()
        );
        cleanup_generation_dir(&generated.generation_dir)
            .await
            .expect("remove private generation fixture");
    }

    #[tokio::test]
    async fn cleanup_rejects_a_known_foreign_identity_before_quarantine() {
        let root = tempdir().expect("create pre-quarantine identity root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
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
        let projection = generated.projections[0].clone();
        let target = root.path().join(&projection.relative_path);
        tokio::fs::remove_file(&target)
            .await
            .expect("remove owned projection");
        tokio::fs::write(&target, b"foreign before cleanup")
            .await
            .expect("write foreign replacement");

        let mut after_quarantine_ran = false;
        let error = cleanup_owned_projection_path_with_hook(&projection, |capability_root| {
            after_quarantine_ran = true;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut replacement = capability_root.open_with(&projection.file_name, &options)?;
            use std::io::Write as _;
            replacement.write_all(b"concurrent recreation")?;
            Ok(())
        })
        .expect_err("known foreign identity is rejected before quarantine");
        assert!(
            format!("{error:#}").contains("changed before quarantine"),
            "unexpected error: {error:#}"
        );
        assert!(
            !after_quarantine_ran,
            "cleanup must not expose an absent project path for a known foreign identity"
        );
        assert_eq!(
            tokio::fs::read(&target)
                .await
                .expect("foreign replacement remains in place"),
            b"foreign before cleanup"
        );
        assert!(
            !projection
                .quarantine_root
                .join(&projection.target_quarantine_path)
                .exists(),
            "known foreign entry is never moved into locald's quarantine"
        );
        cleanup_generation_dir(&generated.generation_dir)
            .await
            .expect("remove private generation fixture");
    }

    #[tokio::test]
    async fn cleanup_returns_a_raced_foreign_entry_to_the_project_namespace() {
        let root = tempdir().expect("create quarantine selection-race root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
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
        let projection = generated.projections[0].clone();
        let target = root.path().join(&projection.relative_path);

        let error = cleanup_owned_projection_path_with_hooks(
            &projection,
            |capability_root| {
                capability_root.remove_file(&projection.file_name)?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                let mut foreign = capability_root.open_with(&projection.file_name, &options)?;
                use std::io::Write as _;
                foreign.write_all(b"foreign selected by raced rename")?;
                Ok(())
            },
            |capability_root| {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                let mut replacement = capability_root.open_with(&projection.file_name, &options)?;
                use std::io::Write as _;
                replacement.write_all(b"newest project entry")?;
                Ok(())
            },
        )
        .expect_err("raced foreign selection is retained outside private quarantine");
        assert!(
            format!("{error:#}").contains("returned to the project namespace"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            tokio::fs::read(&target).await.expect("read newest target"),
            b"newest project entry"
        );
        let recovered = std::fs::read_dir(root.path().join("chat"))
            .expect("read project package")
            .map(|entry| entry.expect("read project entry").path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".locald-recovered-"))
            })
            .expect("raced foreign entry is recovered beside the project target");
        assert_eq!(
            tokio::fs::read(&recovered)
                .await
                .expect("read recovered foreign entry"),
            b"foreign selected by raced rename"
        );
        assert!(
            !projection
                .quarantine_root
                .join(&projection.target_quarantine_path)
                .exists(),
            "raced foreign content is not stranded in locald's private quarantine"
        );
        cleanup_generation_dir(&generated.generation_dir)
            .await
            .expect("remove private generation fixture");
    }

    #[tokio::test]
    async fn reuse_rechecks_the_visible_projection_after_digest_verification() {
        let root = tempdir().expect("create reuse recheck root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
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
        let projection = generated.projections[0].clone();
        let original = tokio::fs::read(root.path().join(&projection.relative_path))
            .await
            .expect("read projected contents");

        let matches = projection_path_matches_ownership_with_hook(&projection, |capability_root| {
            capability_root.remove_file(&projection.file_name)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut replacement = capability_root.open_with(&projection.file_name, &options)?;
            use std::io::Write as _;
            replacement.write_all(&original)?;
            Ok(())
        })
        .expect("reuse validation completes");
        assert!(
            !matches,
            "an identical-byte replacement after digest verification cannot authorize reuse"
        );
        cleanup_generation_dir(&generated.generation_dir)
            .await
            .expect("remove private generation fixture");
    }

    #[tokio::test]
    async fn cleanup_visits_later_projections_after_an_ownership_mismatch() {
        let root = tempdir().expect("create aggregate cleanup root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("one.json"), r#"{"port":1}"#)
            .await
            .expect("write first source");
        tokio::fs::write(root.path().join("two.json"), r#"{"port":2}"#)
            .await
            .expect("write second source");
        let config: ServiceConfig = toml::from_str(
            r#"
command = "true"
[generated.one]
source = "one.json"
project_path = "chat/one.locald.json"
[generated.two]
source = "two.json"
project_path = "chat/two.locald.json"
"#,
        )
        .expect("parse two-projection service");
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
        let first = root.path().join("chat/one.locald.json");
        let second = root.path().join("chat/two.locald.json");
        let first_original = tokio::fs::read(&first).await.expect("read first");
        tokio::fs::write(&first, b"modified")
            .await
            .expect("modify first");
        let error = generated.cleanup().await.expect_err("aggregate mismatch");
        assert!(format!("{error:#}").contains("1 error(s)"));
        assert_eq!(
            tokio::fs::read(&first).await.expect("retained first"),
            b"modified"
        );
        assert!(!second.exists(), "later still-owned sibling is removed");
        tokio::fs::write(&first, first_original)
            .await
            .expect("restore first");
        generated.cleanup().await.expect("finish fixture cleanup");
    }

    #[tokio::test]
    async fn same_basename_projections_have_independent_quarantine_identity() {
        let root = tempdir().expect("create same-basename cleanup root");
        tokio::fs::create_dir_all(root.path().join("one"))
            .await
            .expect("create first parent");
        tokio::fs::create_dir_all(root.path().join("two"))
            .await
            .expect("create second parent");
        tokio::fs::write(root.path().join("one.json"), r#"{"port":1}"#)
            .await
            .expect("write first source");
        tokio::fs::write(root.path().join("two.json"), r#"{"port":2}"#)
            .await
            .expect("write second source");
        let config: ServiceConfig = toml::from_str(
            r#"
command = "true"
[generated.one]
source = "one.json"
project_path = "one/runtime.locald.json"
[generated.two]
source = "two.json"
project_path = "two/runtime.locald.json"
"#,
        )
        .expect("parse same-basename service");
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
        assert_ne!(
            generated.projections[0].projection_id,
            generated.projections[1].projection_id
        );
        assert_ne!(
            generated.projections[0].target_quarantine_path,
            generated.projections[1].target_quarantine_path
        );
        let first = root.path().join("one/runtime.locald.json");
        let second = root.path().join("two/runtime.locald.json");
        let first_original = tokio::fs::read(&first).await.expect("read first");
        tokio::fs::write(&first, b"modified")
            .await
            .expect("modify first");
        generated
            .cleanup()
            .await
            .expect_err("first mismatch retained");
        assert_eq!(
            tokio::fs::read(&first).await.expect("retained first"),
            b"modified"
        );
        assert!(
            !second.exists(),
            "same-basename sibling cleaned independently"
        );
        tokio::fs::write(&first, first_original)
            .await
            .expect("restore first");
        generated.cleanup().await.expect("retry first cleanup");
        assert!(!first.exists());
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn project_path_is_rejected_on_platforms_without_identity_safe_cleanup() {
        let mut config = LocaldConfig::default();
        config.services.insert(
            "web".to_owned(),
            projected_service_config("source.json", "chat/runtime.json", BTreeMap::new()),
        );
        let error = validate_declarations(&config).expect_err("reject unsupported projection");
        assert!(
            error
                .to_string()
                .contains("supported only on macOS and Linux")
        );
    }

    #[tokio::test]
    async fn projection_publication_collision_rolls_back_prior_targets() {
        let root = tempdir().expect("create project root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("one.json"), r#"{"port":1}"#)
            .await
            .expect("write first source");
        tokio::fs::write(root.path().join("two.json"), r#"{"port":2}"#)
            .await
            .expect("write second source");
        let config: ServiceConfig = toml::from_str(
            r#"
command = "true"

[generated.one]
source = "one.json"
project_path = "chat/one.locald.json"

[generated.one.replace]
"/port" = "${services.web.port}"

[generated.two]
source = "two.json"
project_path = "chat/two.locald.json"

[generated.two.replace]
"/port" = "${services.web.port}"
"#,
        )
        .expect("parse projected service");
        let prepared = prepare(root.path(), &key(), &config, None)
            .await
            .expect("prepare")
            .expect("prepared set");
        let foreign = root.path().join("chat/two.locald.json");
        tokio::fs::write(&foreign, b"foreign")
            .await
            .expect("race in foreign target");
        let error = materialize_prepared(&root.path().join("data"), &key(), &bindings(), &prepared)
            .await
            .expect_err("foreign target rejects admission");
        assert!(
            format!("{error:#}").contains("refusing to overwrite"),
            "unexpected error: {error:#}"
        );
        assert!(!root.path().join("chat/one.locald.json").exists());
        assert_eq!(
            tokio::fs::read(&foreign).await.expect("read foreign"),
            b"foreign"
        );
        let leftovers = std::fs::read_dir(root.path().join("chat"))
            .expect("read projection directory")
            .map(|entry| entry.expect("read projection entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(leftovers.len(), 1);
        assert_eq!(
            leftovers[0],
            foreign.file_name().expect("foreign file name")
        );
    }

    #[tokio::test]
    async fn cleanup_retains_modified_projection_and_manifest_for_recovery() {
        let root = tempdir().expect("create project root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "chat/runtime.locald.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let data_dir = root.path().join("data");
        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize")
            .expect("generated set");
        let projection = root.path().join("chat/runtime.locald.json");
        let original = tokio::fs::read(&projection)
            .await
            .expect("read original projection");
        tokio::fs::write(&projection, b"modified")
            .await
            .expect("modify owned target");
        let error = generated
            .cleanup()
            .await
            .expect_err("retain modified target");
        assert!(
            error
                .to_string()
                .contains("retaining generated-file projection")
        );
        assert_eq!(
            tokio::fs::read(&projection).await.expect("read retained"),
            b"modified"
        );
        assert!(generated.generation_dir.exists());
        let service_dir = generated
            .generation_dir
            .parent()
            .expect("service directory");
        let generated_root = service_dir.parent().expect("generated root");
        let stale_same_service = service_dir.join("interrupted-private-generation");
        let stale_other_service = generated_root.join("other-service/private-generation");
        for stale in [&stale_same_service, &stale_other_service] {
            tokio::fs::create_dir_all(stale)
                .await
                .expect("create stale generation");
            tokio::fs::write(stale.join("secret.json"), b"obsolete secret")
                .await
                .expect("write stale private data");
        }
        let outside = root.path().join("outside-generated-data");
        tokio::fs::create_dir(&outside)
            .await
            .expect("create unrelated directory");
        tokio::fs::write(outside.join("keep"), b"unrelated")
            .await
            .expect("write unrelated file");
        let stale_link = service_dir.join("stale-link");
        #[cfg(unix)]
        tokio::fs::symlink(&outside, &stale_link)
            .await
            .expect("create stale symlink");
        let retained = cleanup_all_instances(&data_dir)
            .await
            .expect("recovery isolates the modified target from unrelated startup cleanup");
        assert_eq!(retained.len(), 1);
        assert!(!stale_same_service.exists());
        assert!(
            !stale_other_service
                .parent()
                .expect("other service")
                .exists()
        );
        assert!(tokio::fs::symlink_metadata(&stale_link).await.is_err());
        assert_eq!(
            tokio::fs::read(outside.join("keep"))
                .await
                .expect("read unrelated file"),
            b"unrelated"
        );
        assert!(projection.exists());
        assert!(
            generated.generation_dir.exists(),
            "recovery retains the ownership manifest for a later exact retry"
        );
        for projection in &generated.projections {
            assert!(
                projection.quarantine_root.exists(),
                "retain exact retry quarantine authority"
            );
        }
        tokio::fs::write(&projection, original)
            .await
            .expect("restore owned projection");
        assert_eq!(
            retained[0].service_resource_ids,
            BTreeSet::from([OsString::from(key().resource_id())])
        );
        retained[0]
            .generated_files
            .cleanup()
            .await
            .expect("startup-retained ownership remains live-retryable");
        assert!(!projection.exists());
        assert!(!generated.generation_dir.exists());
        for projection in &generated.projections {
            assert!(!projection.quarantine_root.exists());
        }
    }

    #[tokio::test]
    async fn startup_retry_is_scoped_to_each_retained_generation() {
        let root = tempdir().expect("create multi-generation recovery root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("one.json"), r#"{"value":1}"#)
            .await
            .expect("write first source");
        tokio::fs::write(root.path().join("two.json"), r#"{"value":2}"#)
            .await
            .expect("write second source");
        let data_dir = root.path().join("data");
        let first = materialize(
            &data_dir,
            root.path(),
            &key(),
            &projected_service_config("one.json", "chat/one.locald.json", BTreeMap::new()),
            &bindings(),
        )
        .await
        .expect("materialize first generation")
        .expect("first generated set");
        let second = materialize(
            &data_dir,
            root.path(),
            &key(),
            &projected_service_config("two.json", "chat/two.locald.json", BTreeMap::new()),
            &bindings(),
        )
        .await
        .expect("materialize second generation")
        .expect("second generated set");
        let first_path = root.path().join("chat/one.locald.json");
        let second_path = root.path().join("chat/two.locald.json");
        let first_original = tokio::fs::read(&first_path).await.expect("read first");
        let second_original = tokio::fs::read(&second_path).await.expect("read second");
        tokio::fs::write(&first_path, b"modified first")
            .await
            .expect("modify first");
        tokio::fs::write(&second_path, b"modified second")
            .await
            .expect("modify second");

        let retained = cleanup_all_instances(&data_dir)
            .await
            .expect("collect both retained generations");
        assert_eq!(retained.len(), 2, "each generation has its own retry");
        assert!(
            retained
                .iter()
                .all(|retry| retry.generated_files.projections.len() == 1)
        );
        assert!(
            retained
                .iter()
                .all(|retry| retry.service_resource_ids.len() == 1)
        );

        tokio::fs::write(root.path().join("three.json"), r#"{"value":3}"#)
            .await
            .expect("write fresh source");
        let fresh = materialize(
            &data_dir,
            root.path(),
            &key(),
            &projected_service_config("three.json", "chat/three.locald.json", BTreeMap::new()),
            &bindings(),
        )
        .await
        .expect("materialize fresh generation")
        .expect("fresh generated set");
        let fresh_path = root.path().join("chat/three.locald.json");

        let first_retry = retained
            .iter()
            .find(|retry| {
                retry.generated_files.projections[0].relative_path
                    == Path::new("chat/one.locald.json")
            })
            .expect("first retained generation");
        let second_retry = retained
            .iter()
            .find(|retry| {
                retry.generated_files.projections[0].relative_path
                    == Path::new("chat/two.locald.json")
            })
            .expect("second retained generation");

        tokio::fs::write(&first_path, first_original)
            .await
            .expect("restore first");
        first_retry
            .generated_files
            .cleanup()
            .await
            .expect("the first retry removes only its generation");
        assert!(!first_path.exists());
        assert!(second_path.exists());
        assert!(!first.generation_dir.exists());
        assert!(second.generation_dir.exists());
        assert!(fresh_path.exists());
        assert!(fresh.generation_dir.exists());

        tokio::fs::write(&second_path, second_original)
            .await
            .expect("restore second");
        second_retry
            .generated_files
            .cleanup()
            .await
            .expect("the second retry removes only its generation");
        assert!(!second_path.exists());
        assert!(!second.generation_dir.exists());
        assert!(fresh_path.exists());
        assert!(fresh.generation_dir.exists());

        fresh.cleanup().await.expect("clean fresh generation");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_isolates_an_unreadable_owned_projection_for_retry() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempdir().expect("create unreadable projection root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
        let data_dir = root.path().join("data");
        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize")
            .expect("generated set");
        let projection = root.path().join("chat/runtime.locald.json");
        let original_permissions = std::fs::metadata(&projection)
            .expect("read original permissions")
            .permissions();
        let mut unreadable = original_permissions.clone();
        unreadable.set_mode(0);
        std::fs::set_permissions(&projection, unreadable).expect("remove projection read access");

        cleanup_all_instances(&data_dir)
            .await
            .expect("unreadable projection cannot block unrelated startup recovery");
        assert!(projection.exists());
        assert!(
            generated.generation_dir.exists(),
            "unreadable projection retains its durable retry authority"
        );

        std::fs::set_permissions(&projection, original_permissions)
            .expect("restore projection read access");
        cleanup_all_instances(&data_dir)
            .await
            .expect("retry removes the restored owned projection");
        assert!(!projection.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_retains_an_inaccessible_projection_parent_for_retry() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempdir().expect("create inaccessible parent root");
        let project_parent = root.path().join("chat");
        tokio::fs::create_dir(&project_parent)
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
        let data_dir = root.path().join("data");
        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize")
            .expect("generated set");
        let projection = project_parent.join("runtime.locald.json");
        let original_permissions = std::fs::metadata(&project_parent)
            .expect("read parent permissions")
            .permissions();
        let mut inaccessible = original_permissions.clone();
        inaccessible.set_mode(0);
        std::fs::set_permissions(&project_parent, inaccessible)
            .expect("remove project-parent access");

        let retained = cleanup_all_instances(&data_dir)
            .await
            .expect("inaccessible project parent cannot block daemon startup");
        std::fs::set_permissions(&project_parent, original_permissions)
            .expect("restore project-parent access");
        assert_eq!(retained.len(), 1);
        assert!(projection.exists());
        assert!(generated.generation_dir.exists());

        retained[0]
            .generated_files
            .cleanup()
            .await
            .expect("live retry removes the projection after parent access returns");
        assert!(!projection.exists());
        assert!(!generated.generation_dir.exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn startup_retains_oversized_projection_provenance_for_retry() {
        let root = tempdir().expect("create oversized-provenance root");
        let project_parent = root.path().join("chat");
        tokio::fs::create_dir(&project_parent)
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
        let data_dir = root.path().join("data");
        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize")
            .expect("generated set");
        let parent = Dir::open_ambient_dir(&project_parent, ambient_authority())
            .expect("open projection parent");
        let projection_file = parent.open("runtime.locald.json").expect("open projection");
        set_projection_provenance(&projection_file, &"x".repeat(128))
            .expect("replace provenance with an oversized value");
        projection_file
            .sync_all()
            .expect("sync oversized provenance");
        drop(projection_file);

        let retained = cleanup_all_instances(&data_dir)
            .await
            .expect("oversized provenance cannot block daemon startup");
        assert_eq!(retained.len(), 1);
        assert!(root.path().join("chat/runtime.locald.json").exists());
        assert!(generated.generation_dir.exists());

        let projection_file = parent
            .open("runtime.locald.json")
            .expect("reopen projection");
        set_projection_provenance(&projection_file, &generated.projections[0].projection_id)
            .expect("restore projection provenance");
        projection_file
            .sync_all()
            .expect("sync restored provenance");
        drop(projection_file);
        retained[0]
            .generated_files
            .cleanup()
            .await
            .expect("retry removes the projection after provenance is restored");
        assert!(!root.path().join("chat/runtime.locald.json").exists());
        assert!(!generated.generation_dir.exists());
    }

    #[tokio::test]
    async fn recovery_manifest_removes_an_owned_projection_after_restart() {
        let root = tempdir().expect("create project root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "chat/runtime.locald.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let data_dir = root.path().join("data");
        let generated = materialize(&data_dir, root.path(), &key(), &config, &bindings())
            .await
            .expect("materialize")
            .expect("generated set");
        let generation_dir = generated.generation_dir.clone();
        let projection = root.path().join("chat/runtime.locald.json");
        drop(generated);

        cleanup_all_instances(&data_dir)
            .await
            .expect("recover owned projection");
        assert!(!projection.exists());
        assert!(!generation_dir.exists());
    }

    #[tokio::test]
    async fn active_owned_projection_is_allowed_but_foreign_content_is_not() {
        let root = tempdir().expect("create project root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "chat/runtime.locald.json",
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
        prepare(root.path(), &key(), &config, Some(&generated))
            .await
            .expect("current owned target passes preflight");
        let projection = root.path().join("chat/runtime.locald.json");
        let owned = tokio::fs::read(&projection)
            .await
            .expect("read owned bytes");
        tokio::fs::write(&projection, b"foreign")
            .await
            .expect("modify projection");
        assert!(
            prepare(root.path(), &key(), &config, Some(&generated))
                .await
                .is_err(),
            "modified content must not inherit locald ownership"
        );
        tokio::fs::write(&projection, owned)
            .await
            .expect("restore owned bytes");
        generated.cleanup().await.expect("cleanup owned projection");
    }

    #[tokio::test]
    async fn unicode_equivalent_owned_projection_names_remain_admissible() {
        for (actual, requested) in [
            ("café.json", "CAFÉ.json"),
            ("café.json", "cafe\u{301}.json"),
        ] {
            let root = tempdir().expect("create Unicode projection root");
            tokio::fs::write(root.path().join("source.json"), "{}")
                .await
                .expect("write source");
            let config = projected_service_config("source.json", actual, BTreeMap::new());
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
            let parent =
                Dir::open_ambient_dir(root.path(), ambient_authority()).expect("open parent");
            let metadata = parent.symlink_metadata(actual).expect("owned metadata");
            assert_eq!(
                selected_projection_entry_name(&parent, Path::new(requested), &metadata)
                    .expect("Unicode selector finds the exact owned entry"),
                Path::new(actual)
            );
            // Exercise the entire preflight on filesystems that resolve these
            // Unicode spellings to the same entry (including default macOS).
            if parent.symlink_metadata(requested).is_ok() {
                let renamed = projected_service_config("source.json", requested, BTreeMap::new());
                prepare(root.path(), &key(), &renamed, Some(&generated))
                    .await
                    .expect("Unicode-equivalent spelling preserves owned-entry authorization");
            }
            generated.cleanup().await.expect("cleanup projection");
        }
    }

    #[tokio::test]
    async fn case_only_projection_path_change_keeps_owned_target_admissible() {
        let root = tempdir().expect("create case-only projection root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
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
        let alias = root.path().join("Chat/Runtime.locald.json");
        if tokio::fs::symlink_metadata(&alias).await.is_err() {
            generated
                .cleanup()
                .await
                .expect("cleanup case-sensitive fixture");
            return;
        }

        let renamed =
            projected_service_config("source.json", "Chat/Runtime.locald.json", BTreeMap::new());
        prepare(root.path(), &key(), &renamed, Some(&generated))
            .await
            .expect("case-only spelling of the same owned entry passes preflight");
        generated.cleanup().await.expect("cleanup owned projection");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hard_link_alias_does_not_inherit_projection_ownership() {
        let root = tempdir().expect("create hard-link alias root");
        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
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
        let target = root.path().join("chat/runtime.locald.json");
        let alias = root.path().join("chat/alias.locald.json");
        std::fs::hard_link(&target, &alias).expect("create unrecorded hard-link alias");
        let aliased =
            projected_service_config("source.json", "chat/alias.locald.json", BTreeMap::new());

        let error = prepare(root.path(), &key(), &aliased, Some(&generated))
            .await
            .expect_err("a distinct hard-link alias cannot inherit ownership");
        let message = format!("{error:#}");
        assert!(message.contains("never adopts or overwrites"), "{message}");

        tokio::fs::remove_file(alias)
            .await
            .expect("remove hard-link alias");
        generated.cleanup().await.expect("cleanup owned projection");
    }

    #[tokio::test]
    async fn projections_are_isolated_by_project_root_and_instance_generation() {
        let root = tempdir().expect("create isolation root");
        let first_root = root.path().join("first");
        let second_root = root.path().join("second");
        for project_root in [&first_root, &second_root] {
            tokio::fs::create_dir_all(project_root.join("chat"))
                .await
                .expect("create package root");
            tokio::fs::write(project_root.join("source.json"), r#"{"port":1}"#)
                .await
                .expect("write source");
        }
        let config = projected_service_config(
            "source.json",
            "chat/runtime.locald.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let first = materialize(
            &root.path().join("data"),
            &first_root,
            &key_for("00000000-0000-4000-8000-000000000001"),
            &config,
            &ServiceRuntimeBindings::new(Some(4101), BTreeMap::new()),
        )
        .await
        .expect("materialize first")
        .expect("first generated set");
        let second = materialize(
            &root.path().join("data"),
            &second_root,
            &key_for("00000000-0000-4000-8000-000000000002"),
            &config,
            &ServiceRuntimeBindings::new(Some(4102), BTreeMap::new()),
        )
        .await
        .expect("materialize second")
        .expect("second generated set");
        let first_projection = first_root.join("chat/runtime.locald.json");
        let second_projection = second_root.join("chat/runtime.locald.json");
        assert_eq!(
            serde_json::from_slice::<Value>(&tokio::fs::read(&first_projection).await.unwrap())
                .unwrap()["port"],
            4101
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&tokio::fs::read(&second_projection).await.unwrap())
                .unwrap()["port"],
            4102
        );
        first.cleanup().await.expect("cleanup first");
        assert!(!first_projection.exists());
        assert!(second_projection.exists());
        second.cleanup().await.expect("cleanup second");
    }

    #[tokio::test]
    async fn project_path_declarations_reject_unsafe_and_duplicate_targets() {
        for invalid in [
            "",
            "/absolute.json",
            "../outside.json",
            "source.json",
            "runtime.toml",
        ] {
            let mut config = LocaldConfig::default();
            config.services.insert(
                "web".to_owned(),
                projected_service_config("source.json", invalid, BTreeMap::new()),
            );
            assert!(
                validate_declarations(&config).is_err(),
                "unsafe project_path unexpectedly admitted: {invalid}"
            );
        }

        let mut duplicate = LocaldConfig::default();
        duplicate.services.insert(
            "web".to_owned(),
            projected_service_config("source.json", "Chat/runtime.locald.json", BTreeMap::new()),
        );
        duplicate.services.insert(
            "worker".to_owned(),
            projected_service_config("other.json", "chat/runtime.locald.json", BTreeMap::new()),
        );
        let error = validate_declarations(&duplicate).expect_err("case-folded collision");
        assert!(error.to_string().contains("case-insensitive filesystem"));
    }

    #[test]
    fn project_path_declarations_reject_unicode_equivalent_targets() {
        for (first, second) in [
            ("Étage/runtime.json", "étage/runtime.json"),
            ("chat/café.json", "chat/cafe\u{301}.json"),
            ("CAFÉ/runtime.json", "cafe\u{301}/runtime.json"),
            ("chat/Σ.json", "chat/ς.json"),
            ("chat/straße.json", "chat/STRASSE.json"),
        ] {
            let mut config = LocaldConfig::default();
            config.services.insert(
                "web".to_owned(),
                projected_service_config("source.json", first, BTreeMap::new()),
            );
            config.services.insert(
                "worker".to_owned(),
                projected_service_config("other.json", second, BTreeMap::new()),
            );
            let error =
                validate_declarations(&config).expect_err("Unicode-equivalent target collision");
            assert!(
                error.to_string().contains("case-insensitive filesystem"),
                "{first} / {second}: {error}"
            );
        }

        let mut distinct = LocaldConfig::default();
        distinct.services.insert(
            "web".to_owned(),
            projected_service_config("source.json", "chat/cafe.json", BTreeMap::new()),
        );
        distinct.services.insert(
            "worker".to_owned(),
            projected_service_config("other.json", "chat/café.json", BTreeMap::new()),
        );
        validate_declarations(&distinct).expect("distinct Unicode names remain available");
    }

    #[tokio::test]
    async fn project_path_preparation_rejects_missing_symlinked_and_foreign_targets() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(root.path().join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "missing/runtime.locald.json",
            BTreeMap::new(),
        );
        assert!(prepare(root.path(), &key(), &config, None).await.is_err());

        tokio::fs::create_dir(root.path().join("chat"))
            .await
            .expect("create package root");
        let foreign = root.path().join("chat/runtime.locald.json");
        tokio::fs::write(&foreign, b"foreign")
            .await
            .expect("write foreign target");
        let config =
            projected_service_config("source.json", "chat/runtime.locald.json", BTreeMap::new());
        assert!(prepare(root.path(), &key(), &config, None).await.is_err());
        tokio::fs::remove_file(&foreign)
            .await
            .expect("remove foreign target");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path(), root.path().join("linked"))
                .expect("create ancestor symlink");
            let linked = projected_service_config(
                "source.json",
                "linked/runtime.locald.json",
                BTreeMap::new(),
            );
            assert!(prepare(root.path(), &key(), &linked, None).await.is_err());

            std::os::unix::fs::symlink(root.path().join("source.json"), &foreign)
                .expect("create final symlink");
            assert!(prepare(root.path(), &key(), &config, None).await.is_err());
        }
    }

    #[tokio::test]
    async fn projection_publication_rejects_a_replaced_project_root() {
        let root = tempdir().expect("create project parent");
        let project_root = root.path().join("project");
        tokio::fs::create_dir_all(project_root.join("chat"))
            .await
            .expect("create package root");
        tokio::fs::write(project_root.join("source.json"), r#"{"port":1}"#)
            .await
            .expect("write source");
        let config = projected_service_config(
            "source.json",
            "chat/runtime.locald.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let prepared = prepare(&project_root, &key(), &config, None)
            .await
            .expect("prepare projection")
            .expect("prepared set");
        let moved_root = root.path().join("moved-project");
        tokio::fs::rename(&project_root, &moved_root)
            .await
            .expect("move prepared project root");
        tokio::fs::create_dir_all(project_root.join("chat"))
            .await
            .expect("replace project path");

        let error = materialize_prepared(&root.path().join("data"), &key(), &bindings(), &prepared)
            .await
            .expect_err("replaced root rejects projection publication");
        assert!(
            format!("{error:#}").contains("project root identity changed"),
            "unexpected error: {error:#}"
        );
        assert!(
            std::fs::read_dir(project_root.join("chat"))
                .expect("read replacement package")
                .next()
                .is_none()
        );
        assert!(!moved_root.join("chat/runtime.locald.json").exists());
    }

    #[tokio::test]
    async fn dotted_owner_materializes_primary_and_listener_bindings() {
        let root = tempdir().expect("create dotted-service project root");
        tokio::fs::write(
            root.path().join("runtime.json"),
            r#"{"primary":3000,"listener":3001}"#,
        )
        .await
        .expect("write dotted-service source");
        let config = service_config(
            "runtime.json",
            BTreeMap::from([
                (
                    "/primary".to_owned(),
                    Value::String("${services.api.worker.port}".to_owned()),
                ),
                (
                    "/listener".to_owned(),
                    Value::String("${services.api.worker.listeners.chat.port}".to_owned()),
                ),
            ]),
        );
        let locald = LocaldConfig {
            services: std::collections::HashMap::from([
                ("api.worker".to_owned(), config.clone()),
                (
                    "api.worker.port".to_owned(),
                    ServiceConfig::Legacy(ExecServiceConfig::default()),
                ),
                (
                    "api.worker.listeners.chat".to_owned(),
                    ServiceConfig::Legacy(ExecServiceConfig::default()),
                ),
            ]),
            ..LocaldConfig::default()
        };
        validate_declarations(&locald)
            .expect("exact sibling names cannot hide the dotted owner's runtime bindings");
        let key = ServiceKey::new(key().instance(), "api.worker");

        let generated = materialize(
            &root.path().join("data"),
            root.path(),
            &key,
            &config,
            &bindings(),
        )
        .await
        .expect("materialize dotted owner")
        .expect("generated dotted-owner set");
        let value: Value = serde_json::from_slice(
            &tokio::fs::read(generated.path("microfrontends").expect("generated path"))
                .await
                .expect("read dotted-owner output"),
        )
        .expect("parse dotted-owner output");
        assert_eq!(value.pointer("/primary"), Some(&Value::from(4100)));
        assert_eq!(value.pointer("/listener"), Some(&Value::from(4200)));
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
        for pointer in ["", "options/port", "/options/~2port"] {
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

    #[tokio::test]
    async fn hyphen_pointer_replaces_object_members_but_never_appends_to_arrays() {
        let root = tempdir().expect("create project root");
        tokio::fs::write(
            root.path().join("runtime.json"),
            r#"{"options":{"-":1},"array":[1]}"#,
        )
        .await
        .expect("write source");

        let object_config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/options/-".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let object_locald = LocaldConfig {
            services: std::collections::HashMap::from([("web".to_owned(), object_config.clone())]),
            ..LocaldConfig::default()
        };
        validate_declarations(&object_locald).expect("hyphen object member is a valid pointer");
        let generated = materialize(
            &root.path().join("data"),
            root.path(),
            &key(),
            &object_config,
            &bindings(),
        )
        .await
        .expect("materialize object member")
        .expect("generated file declarations");
        let value: Value = serde_json::from_slice(
            &tokio::fs::read(generated.path("microfrontends").expect("generated path"))
                .await
                .expect("read generated file"),
        )
        .expect("parse generated file");
        assert_eq!(value.pointer("/options/-"), Some(&Value::from(4100)));

        let array_config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/array/-".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );
        let array_locald = LocaldConfig {
            services: std::collections::HashMap::from([("web".to_owned(), array_config.clone())]),
            ..LocaldConfig::default()
        };
        validate_declarations(&array_locald)
            .expect("array append cannot be rejected before the source is known");
        let error = prepare(root.path(), &key(), &array_config, None)
            .await
            .expect_err("generated replacements never append to arrays");
        assert!(format!("{error:#}").contains("does not identify an existing value"));
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
                identity: ProjectionFileIdentity::default(),
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
                identity: ProjectionFileIdentity::default(),
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
                identity: ProjectionFileIdentity::default(),
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

        let error = prepare(root.path(), &key(), &config, None)
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
        let prepared = prepare(root.path(), &key(), &config, None)
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
        assert!(generated.matches_prepared(&prepared).await);
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
    async fn source_symlink_within_the_project_root_remains_supported() {
        let root = tempdir().expect("create project root");
        tokio::fs::create_dir(root.path().join("config"))
            .await
            .expect("create source directory");
        tokio::fs::write(root.path().join("config/runtime.json"), r#"{"port":3000}"#)
            .await
            .expect("write source");
        std::os::unix::fs::symlink("config/runtime.json", root.path().join("runtime.json"))
            .expect("create in-project source symlink");
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
        .expect("materialize in-project symlink")
        .expect("generated set");
        let output =
            tokio::fs::read_to_string(generated.path("microfrontends").expect("generated path"))
                .await
                .expect("read generated output");
        let value: Value = serde_json::from_str(&output).expect("valid generated JSON");
        assert_eq!(value.pointer("/port"), Some(&Value::from(4100)));
        generated.cleanup().await.expect("clean generation");
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

    #[cfg(unix)]
    #[test]
    fn capability_open_rejects_an_ancestor_replaced_with_an_outside_symlink() {
        let root = tempdir().expect("create project root");
        let outside = tempdir().expect("create outside directory");
        let config_dir = root.path().join("config");
        std::fs::create_dir(&config_dir).expect("create source directory");
        std::fs::write(config_dir.join("runtime.json"), r#"{"port":3000}"#)
            .expect("write in-project source");
        std::fs::write(outside.path().join("runtime.json"), r#"{"port":9999}"#)
            .expect("write outside source");

        let canonical_root = std::fs::canonicalize(root.path()).expect("canonicalize project root");
        let root_capability = Dir::open_ambient_dir(&canonical_root, ambient_authority())
            .expect("acquire root capability");

        std::fs::rename(&config_dir, root.path().join("config-original"))
            .expect("move original source directory");
        std::os::unix::fs::symlink(outside.path(), &config_dir)
            .expect("replace nested ancestor with outside symlink");

        open_source_from_root_capability(&root_capability, Path::new("config/runtime.json"))
            .expect_err("root capability must reject an escaping ancestor replacement");
    }

    #[cfg(unix)]
    #[test]
    fn capability_open_rejects_a_replaced_project_root() {
        use std::os::unix::fs::MetadataExt;

        let parent = tempdir().expect("create project parent");
        let project = parent.path().join("project");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&project).expect("create project root");
        std::fs::create_dir(&outside).expect("create outside root");
        std::fs::write(project.join("runtime.json"), r#"{"port":3000}"#)
            .expect("write in-project source");
        std::fs::write(outside.join("runtime.json"), r#"{"port":9999}"#)
            .expect("write outside source");

        let canonical_root = std::fs::canonicalize(&project).expect("canonicalize project root");
        let original_metadata =
            std::fs::symlink_metadata(&canonical_root).expect("inspect original project root");
        let original_identity = (original_metadata.dev(), original_metadata.ino());

        std::fs::rename(&project, parent.path().join("project-original"))
            .expect("move original project root");
        std::os::unix::fs::symlink(&outside, &project)
            .expect("replace project root with outside symlink");

        open_source_under_project_capability(
            &canonical_root,
            Path::new("runtime.json"),
            original_identity,
        )
        .expect_err("root identity check must reject a substituted capability root");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn special_source_is_rejected_without_waiting_for_a_writer() {
        let root = tempdir().expect("create project root");
        let source = root.path().join("runtime.json");
        let output = std::process::Command::new("mkfifo")
            .arg(&source)
            .output()
            .expect("create FIFO source");
        assert!(
            output.status.success(),
            "mkfifo failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let config = service_config(
            "runtime.json",
            BTreeMap::from([(
                "/port".to_owned(),
                Value::String("${services.web.port}".to_owned()),
            )]),
        );

        let (completed_tx, completed_rx) = std::sync::mpsc::channel();
        let watchdog_source = source.clone();
        let watchdog = std::thread::spawn(move || -> std::io::Result<()> {
            use std::os::unix::fs::OpenOptionsExt;

            if completed_rx
                .recv_timeout(std::time::Duration::from_secs(6))
                .is_ok()
            {
                return Ok(());
            }
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(watchdog_source)?;
            Ok(())
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prepare(root.path(), &key(), &config, None),
        )
        .await;
        if result.is_ok() {
            let _ = completed_tx.send(());
        } else {
            drop(completed_tx);
        }
        watchdog
            .join()
            .expect("FIFO regression watchdog must not panic")
            .expect("FIFO regression watchdog must release a blocked reader");
        let error = result
            .expect("special-file rejection must not wait for a writer")
            .expect_err("FIFO generated source must fail closed");
        assert!(format!("{error:#}").contains("not a regular file"));
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
