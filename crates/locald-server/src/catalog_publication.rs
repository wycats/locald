//! Durable catalog, domain, and hosts publication transactions.
//!
//! The process manager owns serialization and side effects. This module owns
//! only the exact before/after images and the durable phase that startup must
//! resume before any daemon listener becomes observable.

#![allow(clippy::redundant_pub_crate)]

use locald_core::ProjectCatalog;
use locald_core::catalog::CATALOG_VERSION;
use locald_hosts::HostSet;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

pub(crate) const CATALOG_PUBLICATION_VERSION: u32 = 1;
const JOURNAL_FILE: &str = "catalog-publication.json";
const CATALOG_V5_BACKUP_FILE: &str = "catalog-v5.json";
const FIRST_CATALOG_VERSION_REQUIRING_AGENT_BINDINGS: u64 = 4;
const OLDEST_REPLAYABLE_CATALOG_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogPublicationPhase {
    Prepared,
    HostsApplied,
    StateCommitted,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogPublicationTransaction {
    version: u32,
    // The loader injects this from equal raw embedded image versions for
    // predecessor-authored version-1 journals that predate the field.
    catalog_source_version: u32,
    #[serde(default)]
    schema_migration: bool,
    target_generation: Uuid,
    phase: CatalogPublicationPhase,
    catalog_path: PathBuf,
    catalog_base: ProjectCatalog,
    catalog_target: ProjectCatalog,
    previous_hosts: Vec<String>,
    candidate_hosts: Vec<String>,
}

impl CatalogPublicationTransaction {
    pub(crate) fn new(
        catalog_base: ProjectCatalog,
        catalog_target: ProjectCatalog,
        previous_hosts: &HostSet,
        candidate_hosts: &HostSet,
    ) -> Result<Self, CatalogPublicationError> {
        if catalog_base.storage_path() != catalog_target.storage_path() {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: format!(
                    "catalog base path `{}` does not match target path `{}`",
                    catalog_base.storage_path().display(),
                    catalog_target.storage_path().display()
                ),
            });
        }
        let transaction = Self {
            version: CATALOG_PUBLICATION_VERSION,
            catalog_source_version: CATALOG_VERSION,
            schema_migration: false,
            target_generation: Uuid::new_v4(),
            phase: CatalogPublicationPhase::Prepared,
            catalog_path: catalog_base.storage_path().to_path_buf(),
            catalog_base,
            catalog_target,
            previous_hosts: previous_hosts.as_strings(),
            candidate_hosts: candidate_hosts.as_strings(),
        };
        transaction.validate()?;
        Ok(transaction)
    }

    pub(crate) fn new_from_catalog_source(
        catalog_base: ProjectCatalog,
        catalog_target: ProjectCatalog,
        previous_hosts: &HostSet,
        candidate_hosts: &HostSet,
        catalog_source_version: u32,
    ) -> Result<Self, CatalogPublicationError> {
        let mut transaction = Self::new(
            catalog_base,
            catalog_target,
            previous_hosts,
            candidate_hosts,
        )?;
        transaction.catalog_source_version = catalog_source_version;
        transaction.validate()?;
        Ok(transaction)
    }

    /// Prepare the one journaled schema transition from the exact v5 image.
    ///
    /// The two logical images are equal because v6 adds an empty projection;
    /// `catalog_source_version` records that the durable base still requires a
    /// physical v6 publication after its exact raw backup is established.
    pub(crate) fn new_v5_migration(
        catalog: ProjectCatalog,
        hosts: &HostSet,
    ) -> Result<Self, CatalogPublicationError> {
        let mut transaction = Self::new_from_catalog_source(
            catalog.clone(),
            catalog,
            hosts,
            hosts,
            CATALOG_VERSION - 1,
        )?;
        transaction.schema_migration = true;
        transaction.validate()?;
        Ok(transaction)
    }

    #[must_use]
    pub(crate) const fn target_generation(&self) -> Uuid {
        self.target_generation
    }

    #[must_use]
    pub(crate) const fn phase(&self) -> CatalogPublicationPhase {
        self.phase
    }

    #[must_use]
    pub(crate) const fn is_schema_migration(&self) -> bool {
        self.schema_migration
    }

    #[must_use]
    pub(crate) const fn source_requires_v5_backup(&self) -> bool {
        self.catalog_source_version == CATALOG_VERSION - 1
    }

    #[must_use]
    pub(crate) const fn catalog_source_version(&self) -> u32 {
        self.catalog_source_version
    }

    #[must_use]
    pub(crate) const fn catalog_base(&self) -> &ProjectCatalog {
        &self.catalog_base
    }

    #[must_use]
    pub(crate) const fn catalog_target(&self) -> &ProjectCatalog {
        &self.catalog_target
    }

    #[must_use]
    pub(crate) fn catalog_path(&self) -> &Path {
        &self.catalog_path
    }

    pub(crate) fn previous_hosts(&self) -> Result<HostSet, CatalogPublicationError> {
        canonical_host_set(&self.previous_hosts, "previous")
    }

    pub(crate) fn candidate_hosts(&self) -> Result<HostSet, CatalogPublicationError> {
        canonical_host_set(&self.candidate_hosts, "candidate")
    }

    pub(crate) fn catalog_for_missing_storage(
        &self,
    ) -> Result<ProjectCatalog, CatalogPublicationError> {
        match self.phase {
            CatalogPublicationPhase::Prepared
            | CatalogPublicationPhase::HostsApplied
            | CatalogPublicationPhase::Aborted => Ok(self.catalog_base.clone()),
            CatalogPublicationPhase::StateCommitted => Err(CatalogPublicationError::InvalidPlan {
                reason: format!(
                    "catalog publication generation {} is state_committed, but `{}` is missing",
                    self.target_generation,
                    self.catalog_path.display()
                ),
            }),
        }
    }

    pub(crate) fn normalize_catalog_storage_path(&mut self, catalog_path: &Path) {
        self.catalog_path = catalog_path.to_path_buf();
        self.catalog_base
            .set_storage_path(catalog_path.to_path_buf());
        self.catalog_target
            .set_storage_path(catalog_path.to_path_buf());
    }

    fn normalize_deserialized_catalogs(
        &mut self,
        catalog_path: &Path,
    ) -> Result<(), CatalogPublicationError> {
        self.normalize_catalog_storage_path(catalog_path);
        self.catalog_base
            .upgrade_embedded_schema()
            .map_err(|error| CatalogPublicationError::InvalidPlan {
                reason: format!("failed to upgrade embedded catalog base: {error}"),
            })?;
        self.catalog_target
            .upgrade_embedded_schema()
            .map_err(|error| CatalogPublicationError::InvalidPlan {
                reason: format!("failed to upgrade embedded catalog target: {error}"),
            })?;
        Ok(())
    }

    fn validate(&self) -> Result<(), CatalogPublicationError> {
        if self.version != CATALOG_PUBLICATION_VERSION {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: format!(
                    "transaction version {} does not match {}",
                    self.version, CATALOG_PUBLICATION_VERSION
                ),
            });
        }
        if !(u64::from(OLDEST_REPLAYABLE_CATALOG_VERSION)..=u64::from(CATALOG_VERSION))
            .contains(&u64::from(self.catalog_source_version))
        {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: format!(
                    "catalog source version {} cannot be replayed by schema version {CATALOG_VERSION}",
                    self.catalog_source_version
                ),
            });
        }
        if self.target_generation.is_nil() {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: "target generation must not be nil".to_owned(),
            });
        }
        if self.catalog_base.storage_path() != self.catalog_path
            || self.catalog_target.storage_path() != self.catalog_path
        {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: "embedded catalog storage paths do not match the publication path"
                    .to_owned(),
            });
        }
        self.catalog_base
            .validate()
            .map_err(|error| CatalogPublicationError::InvalidPlan {
                reason: format!("invalid base catalog: {error}"),
            })?;
        self.catalog_target
            .validate()
            .map_err(|error| CatalogPublicationError::InvalidPlan {
                reason: format!("invalid target catalog: {error}"),
            })?;
        if self.catalog_source_version < CATALOG_VERSION {
            for (name, image) in [
                ("base", &self.catalog_base),
                ("target", &self.catalog_target),
            ] {
                if image.published_declarations().next().is_some()
                    || image.retired_configuration_revisions().next().is_some()
                    || image
                        .instances
                        .values()
                        .any(|instance| instance.configuration_revision != 0)
                {
                    return Err(CatalogPublicationError::InvalidPlan {
                        reason: format!(
                            "predecessor-authored catalog {name} contains version-{CATALOG_VERSION} declaration state"
                        ),
                    });
                }
            }
        }
        if self.is_schema_migration() {
            if self.catalog_source_version != CATALOG_VERSION - 1 {
                return Err(CatalogPublicationError::InvalidPlan {
                    reason:
                        "catalog schema migration must originate from the supported predecessor"
                            .to_owned(),
                });
            }
            if self.catalog_base != self.catalog_target {
                return Err(CatalogPublicationError::InvalidPlan {
                    reason: "catalog schema migration must preserve one equal logical image"
                        .to_owned(),
                });
            }
            if self
                .catalog_target
                .published_declarations()
                .next()
                .is_some()
                || self
                    .catalog_target
                    .retired_configuration_revisions()
                    .next()
                    .is_some()
                || self
                    .catalog_target
                    .instances
                    .values()
                    .any(|instance| instance.configuration_revision != 0)
            {
                return Err(CatalogPublicationError::InvalidPlan {
                    reason: "version-5 migration target must begin with empty published and retired-revision projections and zero live configuration revisions".to_owned(),
                });
            }
        }
        let previous_hosts = canonical_host_set(&self.previous_hosts, "previous")?;
        let candidate_hosts = canonical_host_set(&self.candidate_hosts, "candidate")?;
        if previous_hosts != host_set_for_catalog(&self.catalog_base)? {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: "previous host set does not match the base catalog domain projection"
                    .to_owned(),
            });
        }
        if candidate_hosts != host_set_for_catalog(&self.catalog_target)? {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: "candidate host set does not match the target catalog domain projection"
                    .to_owned(),
            });
        }
        Ok(())
    }

    fn advanced_to(&self, phase: CatalogPublicationPhase) -> Result<Self, CatalogPublicationError> {
        let allowed = valid_phase_transition(self.phase, phase);
        if !allowed {
            return Err(CatalogPublicationError::InvalidPhaseTransition {
                from: self.phase,
                to: phase,
            });
        }
        let mut advanced = self.clone();
        advanced.phase = phase;
        advanced.validate()?;
        Ok(advanced)
    }
}

const fn valid_phase_transition(
    from: CatalogPublicationPhase,
    to: CatalogPublicationPhase,
) -> bool {
    matches!(
        (from, to),
        (
            CatalogPublicationPhase::Prepared,
            CatalogPublicationPhase::HostsApplied | CatalogPublicationPhase::Aborted
        ) | (
            CatalogPublicationPhase::HostsApplied,
            CatalogPublicationPhase::StateCommitted | CatalogPublicationPhase::Aborted
        )
    )
}

pub(crate) fn host_set_for_catalog(
    catalog: &ProjectCatalog,
) -> Result<HostSet, CatalogPublicationError> {
    #[cfg(target_os = "macos")]
    let domains = catalog
        .domain_index()
        .macos_hosts_domain_names()
        .into_iter()
        .map(|domain| domain.to_string());
    #[cfg(not(target_os = "macos"))]
    let domains = catalog
        .domain_index()
        .hosts_domain_names()
        .into_iter()
        .map(|domain| domain.to_string());

    HostSet::try_from_strings(domains).map_err(|error| CatalogPublicationError::InvalidPlan {
        reason: format!("catalog domain projection produced an invalid host set: {error}"),
    })
}

fn canonical_host_set(
    domains: &[String],
    image: &'static str,
) -> Result<HostSet, CatalogPublicationError> {
    let host_set = HostSet::try_from_strings(domains).map_err(|error| {
        CatalogPublicationError::InvalidPlan {
            reason: format!("invalid {image} host set: {error}"),
        }
    })?;
    if host_set.as_strings() != domains {
        return Err(CatalogPublicationError::InvalidPlan {
            reason: format!("{image} host set is not canonical, sorted, and unique"),
        });
    }
    Ok(host_set)
}

#[derive(Debug, Error)]
pub(crate) enum CatalogPublicationError {
    #[error("failed to {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "catalog publication journal `{path}` uses unsupported version {found}; expected {expected}"
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u64,
        expected: u32,
    },
    #[error("invalid catalog publication journal `{path}`: {reason}")]
    InvalidData { path: PathBuf, reason: String },
    #[error("invalid catalog publication plan: {reason}")]
    InvalidPlan { reason: String },
    #[error("catalog version-5 backup `{path}` already exists with different raw content")]
    BackupConflict { path: PathBuf },
    #[error("catalog publication journal `{path}` already contains generation {existing}")]
    ActiveTransaction { path: PathBuf, existing: Uuid },
    #[error("catalog publication journal `{path}` does not exist")]
    MissingTransaction { path: PathBuf },
    #[error(
        "catalog publication journal owns generation {actual}, not requested generation {requested}"
    )]
    TransactionMismatch { requested: Uuid, actual: Uuid },
    #[error(
        "catalog publication generation {generation} is at phase {actual:?}, not expected phase {expected:?}"
    )]
    PhaseMismatch {
        generation: Uuid,
        expected: CatalogPublicationPhase,
        actual: CatalogPublicationPhase,
    },
    #[error("invalid catalog publication phase transition from {from:?} to {to:?}")]
    InvalidPhaseTransition {
        from: CatalogPublicationPhase,
        to: CatalogPublicationPhase,
    },
    #[error("catalog publication generation {generation} cannot be cleared at phase {phase:?}")]
    IncompleteTransaction {
        generation: Uuid,
        phase: CatalogPublicationPhase,
    },
    #[error("{operation} published `{path}`, but its parent-directory sync failed: {reason}")]
    PublishedNotDurable {
        operation: &'static str,
        path: PathBuf,
        reason: String,
    },
}

/// Read the exact on-disk catalog schema without accepting semantic migration.
pub(crate) async fn catalog_schema_version(
    catalog_path: &Path,
) -> Result<Option<u64>, CatalogPublicationError> {
    let Some(content) = read_optional_bytes(catalog_path).await? else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_slice(&content).map_err(|source| {
        CatalogPublicationError::InvalidData {
            path: catalog_path.to_path_buf(),
            reason: source.to_string(),
        }
    })?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CatalogPublicationError::InvalidData {
            path: catalog_path.to_path_buf(),
            reason: "missing unsigned integer `version`".to_owned(),
        })?;
    Ok(Some(version))
}

/// Preserve the exact raw v5 catalog before any v6 journal can become active.
///
/// The backup is create-once and content-verified. A conflicting pre-existing
/// file blocks migration without replacing either side.
pub(crate) async fn ensure_v5_backup(
    catalog_path: &Path,
) -> Result<PathBuf, CatalogPublicationError> {
    let content = read_optional_regular_file_no_follow(
        catalog_path,
        "read version-5 catalog migration source",
    )
    .await?
    .ok_or_else(|| CatalogPublicationError::InvalidData {
        path: catalog_path.to_path_buf(),
        reason: "version-5 migration source is missing".to_owned(),
    })?;
    let value: serde_json::Value = serde_json::from_slice(&content).map_err(|source| {
        CatalogPublicationError::InvalidData {
            path: catalog_path.to_path_buf(),
            reason: source.to_string(),
        }
    })?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CatalogPublicationError::InvalidData {
            path: catalog_path.to_path_buf(),
            reason: "missing unsigned integer `version`".to_owned(),
        })?;
    if version != u64::from(CATALOG_VERSION - 1) {
        return Err(CatalogPublicationError::InvalidData {
            path: catalog_path.to_path_buf(),
            reason: format!(
                "catalog migration backup requires version {}, found {version}",
                CATALOG_VERSION - 1
            ),
        });
    }

    let parent = catalog_path
        .parent()
        .ok_or_else(|| CatalogPublicationError::InvalidData {
            path: catalog_path.to_path_buf(),
            reason: "catalog path has no parent directory".to_owned(),
        })?;
    let backup_path = parent.join(CATALOG_V5_BACKUP_FILE);
    match read_optional_regular_file_no_follow(
        &backup_path,
        "read existing version-5 catalog backup",
    )
    .await?
    {
        Some(existing) if existing == content => {
            sync_parent(&backup_path, "confirm version-5 catalog backup").await?;
            return Ok(backup_path);
        }
        Some(_) => return Err(CatalogPublicationError::BackupConflict { path: backup_path }),
        None => {}
    }

    ensure_parent(&backup_path).await?;
    let temporary = parent.join(format!(".{CATALOG_V5_BACKUP_FILE}.{}.tmp", Uuid::new_v4()));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation: "create temporary version-5 catalog backup",
            path: temporary.clone(),
            source,
        })?;
    let write_result = async {
        output.write_all(&content).await?;
        output.sync_all().await
    }
    .await;
    drop(output);
    if let Err(source) = write_result {
        let _ = fs::remove_file(&temporary).await;
        return Err(CatalogPublicationError::Io {
            operation: "write and sync temporary version-5 catalog backup",
            path: temporary,
            source,
        });
    }

    let link_result = fs::hard_link(&temporary, &backup_path).await;
    let cleanup_result = fs::remove_file(&temporary).await;
    match link_result {
        Ok(()) => {
            let _ = cleanup_result;
            sync_parent(&backup_path, "create version-5 catalog backup").await?;
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            let _ = cleanup_result;
            let existing = read_optional_regular_file_no_follow(
                &backup_path,
                "read concurrently created version-5 catalog backup",
            )
            .await?
            .ok_or_else(|| CatalogPublicationError::InvalidData {
                path: backup_path.clone(),
                reason: "concurrently created version-5 catalog backup disappeared".to_owned(),
            })?;
            if existing != content {
                return Err(CatalogPublicationError::BackupConflict { path: backup_path });
            }
            sync_parent(&backup_path, "confirm version-5 catalog backup").await?;
        }
        Err(source) => {
            if let Err(cleanup_source) = cleanup_result {
                return Err(CatalogPublicationError::Io {
                    operation: "create version-5 catalog backup and clean temporary file",
                    path: backup_path,
                    source: io::Error::new(
                        source.kind(),
                        format!("{source}; temporary cleanup also failed: {cleanup_source}"),
                    ),
                });
            }
            return Err(CatalogPublicationError::Io {
                operation: "atomically create version-5 catalog backup",
                path: backup_path,
                source,
            });
        }
    }
    Ok(backup_path)
}

/// Ensure recovery of a predecessor-authored journal retains v5 evidence.
///
/// A prior recovery attempt may already have published v6 before crashing.
/// In that case the create-once backup must already exist and still identify
/// itself as v5. Missing source and missing backup fail closed because no
/// byte-exact predecessor image can be reconstructed after the fact.
pub(crate) async fn ensure_v5_recovery_backup(
    catalog_path: &Path,
    expected_sources: &[&ProjectCatalog],
) -> Result<PathBuf, CatalogPublicationError> {
    if expected_sources.is_empty() {
        return Err(CatalogPublicationError::InvalidPlan {
            reason: "version-5 recovery backup requires at least one expected catalog image"
                .to_owned(),
        });
    }
    let backup_path = match catalog_schema_version(catalog_path).await? {
        Some(version) if version == u64::from(CATALOG_VERSION - 1) => {
            ensure_v5_backup(catalog_path).await?
        }
        Some(version) if version == u64::from(CATALOG_VERSION) => {
            let parent =
                catalog_path
                    .parent()
                    .ok_or_else(|| CatalogPublicationError::InvalidData {
                        path: catalog_path.to_path_buf(),
                        reason: "catalog path has no parent directory".to_owned(),
                    })?;
            parent.join(CATALOG_V5_BACKUP_FILE)
        }
        Some(version) => {
            return Err(CatalogPublicationError::InvalidData {
                path: catalog_path.to_path_buf(),
                reason: format!(
                    "predecessor-authored recovery journal requires catalog version {} or a verified backup after version {CATALOG_VERSION}, found {version}",
                    CATALOG_VERSION - 1
                ),
            });
        }
        None => {
            return Err(CatalogPublicationError::InvalidData {
                path: catalog_path.to_path_buf(),
                reason: "predecessor-authored recovery journal cannot preserve an exact version-5 catalog because the durable catalog is missing"
                    .to_owned(),
            });
        }
    };
    let backup = read_optional_regular_file_no_follow(
        &backup_path,
        "read and verify version-5 recovery backup",
    )
    .await?
    .ok_or_else(|| CatalogPublicationError::InvalidData {
        path: backup_path.clone(),
        reason: "version-5 recovery backup is missing".to_owned(),
    })?;
    let backup_value: serde_json::Value =
        serde_json::from_slice(&backup).map_err(|error| CatalogPublicationError::InvalidData {
            path: backup_path.clone(),
            reason: format!("version-5 recovery backup is invalid: {error}"),
        })?;
    let backup_version = backup_value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| CatalogPublicationError::InvalidData {
            path: backup_path.clone(),
            reason: "version-5 recovery backup is missing unsigned integer `version`".to_owned(),
        })?;
    if backup_version != u64::from(CATALOG_VERSION - 1) {
        return Err(CatalogPublicationError::InvalidData {
            path: backup_path,
            reason: format!(
                "version-5 recovery backup must contain raw catalog schema {}, found {backup_version}",
                CATALOG_VERSION - 1
            ),
        });
    }
    let decoded =
        ProjectCatalog::decode_for_lifecycle_recovery(&backup, catalog_path).map_err(|error| {
            CatalogPublicationError::InvalidData {
                path: backup_path.clone(),
                reason: format!("version-5 recovery backup is invalid: {error}"),
            }
        })?;
    if !expected_sources.contains(&&decoded) {
        return Err(CatalogPublicationError::InvalidData {
            path: backup_path,
            reason: "version-5 recovery backup does not match either catalog image owned by the active journal"
                .to_owned(),
        });
    }
    Ok(backup_path)
}

#[derive(Debug, Clone)]
pub(crate) struct CatalogPublicationJournal {
    journal_path: PathBuf,
}

impl CatalogPublicationJournal {
    pub(crate) fn for_catalog_path(catalog_path: &Path) -> Result<Self, CatalogPublicationError> {
        let parent = catalog_path
            .parent()
            .ok_or_else(|| CatalogPublicationError::InvalidPlan {
                reason: format!(
                    "catalog path `{}` has no parent directory",
                    catalog_path.display()
                ),
            })?;
        Ok(Self {
            journal_path: parent.join(JOURNAL_FILE),
        })
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    pub(crate) async fn load(
        &self,
        catalog_path: &Path,
    ) -> Result<Option<CatalogPublicationTransaction>, CatalogPublicationError> {
        let content = match read_optional_bytes(&self.journal_path).await? {
            Some(content) => content,
            None => return Ok(None),
        };
        let mut value: serde_json::Value = serde_json::from_slice(&content).map_err(|source| {
            CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason: source.to_string(),
            }
        })?;
        let found = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason: "missing unsigned integer `version`".to_owned(),
            })?;
        if found != u64::from(CATALOG_PUBLICATION_VERSION) {
            return Err(CatalogPublicationError::UnsupportedVersion {
                path: self.journal_path.clone(),
                found,
                expected: CATALOG_PUBLICATION_VERSION,
            });
        }
        normalize_embedded_catalog_source_version(&mut value).map_err(|reason| {
            CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason,
            }
        })?;
        validate_embedded_catalog_agent_bindings(&value).map_err(|reason| {
            CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason,
            }
        })?;
        let mut transaction: CatalogPublicationTransaction = serde_json::from_value(value)
            .map_err(|source| CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason: source.to_string(),
            })?;
        if transaction.catalog_path != catalog_path {
            return Err(CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason: format!(
                    "journal catalog path `{}` does not match live path `{}`",
                    transaction.catalog_path.display(),
                    catalog_path.display()
                ),
            });
        }
        transaction
            .normalize_deserialized_catalogs(catalog_path)
            .map_err(|error| CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason: error.to_string(),
            })?;
        transaction
            .validate()
            .map_err(|error| CatalogPublicationError::InvalidData {
                path: self.journal_path.clone(),
                reason: error.to_string(),
            })?;
        Ok(Some(transaction))
    }

    pub(crate) async fn create(
        &self,
        transaction: &CatalogPublicationTransaction,
    ) -> Result<(), CatalogPublicationError> {
        transaction.validate()?;
        if transaction.phase() != CatalogPublicationPhase::Prepared {
            return Err(CatalogPublicationError::InvalidPlan {
                reason: "a new catalog publication must be in the prepared phase".to_owned(),
            });
        }
        if let Some(current) = self.load(&transaction.catalog_path).await? {
            if current == *transaction {
                write_transaction(
                    &self.journal_path,
                    &current,
                    "confirm prepared catalog publication journal",
                )
                .await?;
                return Ok(());
            }
            return Err(CatalogPublicationError::ActiveTransaction {
                path: self.journal_path.clone(),
                existing: current.target_generation(),
            });
        }
        write_transaction(
            &self.journal_path,
            transaction,
            "create catalog publication journal",
        )
        .await
    }

    pub(crate) async fn advance(
        &self,
        generation: Uuid,
        expected: CatalogPublicationPhase,
        next: CatalogPublicationPhase,
        catalog_path: &Path,
    ) -> Result<(), CatalogPublicationError> {
        if !valid_phase_transition(expected, next) {
            return Err(CatalogPublicationError::InvalidPhaseTransition {
                from: expected,
                to: next,
            });
        }
        let current = self.load(catalog_path).await?.ok_or_else(|| {
            CatalogPublicationError::MissingTransaction {
                path: self.journal_path.clone(),
            }
        })?;
        if current.target_generation() != generation {
            return Err(CatalogPublicationError::TransactionMismatch {
                requested: generation,
                actual: current.target_generation(),
            });
        }
        if current.phase() == next {
            write_transaction(
                &self.journal_path,
                &current,
                "confirm advanced catalog publication journal",
            )
            .await?;
            return Ok(());
        }
        if current.phase() != expected {
            return Err(CatalogPublicationError::PhaseMismatch {
                generation,
                expected,
                actual: current.phase(),
            });
        }
        let advanced = current.advanced_to(next)?;
        write_transaction(
            &self.journal_path,
            &advanced,
            "advance catalog publication journal",
        )
        .await
    }

    pub(crate) async fn clear(
        &self,
        generation: Uuid,
        catalog_path: &Path,
    ) -> Result<(), CatalogPublicationError> {
        let Some(current) = self.load(catalog_path).await? else {
            ensure_parent(&self.journal_path).await?;
            sync_parent(
                &self.journal_path,
                "confirm cleared catalog publication journal",
            )
            .await?;
            return Ok(());
        };
        if current.target_generation() != generation {
            return Err(CatalogPublicationError::TransactionMismatch {
                requested: generation,
                actual: current.target_generation(),
            });
        }
        if !matches!(
            current.phase(),
            CatalogPublicationPhase::StateCommitted | CatalogPublicationPhase::Aborted
        ) {
            return Err(CatalogPublicationError::IncompleteTransaction {
                generation,
                phase: current.phase(),
            });
        }
        fs::remove_file(&self.journal_path)
            .await
            .map_err(|source| CatalogPublicationError::Io {
                operation: "clear catalog publication journal",
                path: self.journal_path.clone(),
                source,
            })?;
        sync_parent(&self.journal_path, "clear catalog publication journal").await
    }
}

async fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, CatalogPublicationError> {
    match fs::read(path).await {
        Ok(content) => Ok(Some(content)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            match fs::symlink_metadata(path).await {
                Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => Ok(None),
                Ok(_) => Err(CatalogPublicationError::Io {
                    operation: "read catalog publication journal",
                    path: path.to_path_buf(),
                    source,
                }),
                Err(metadata_error) => Err(CatalogPublicationError::Io {
                    operation: "inspect unreadable catalog publication journal",
                    path: path.to_path_buf(),
                    source: io::Error::new(
                        metadata_error.kind(),
                        format!("{source}; metadata inspection failed: {metadata_error}"),
                    ),
                }),
            }
        }
        Err(source) => Err(CatalogPublicationError::Io {
            operation: "read catalog publication journal",
            path: path.to_path_buf(),
            source,
        }),
    }
}

async fn read_optional_regular_file_no_follow(
    path: &Path,
    operation: &'static str,
) -> Result<Option<Vec<u8>>, CatalogPublicationError> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(CatalogPublicationError::Io {
                operation: "inspect version-5 catalog evidence",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() {
        return Err(CatalogPublicationError::InvalidData {
            path: path.to_path_buf(),
            reason: "version-5 catalog evidence must be a non-symlink regular file".to_owned(),
        });
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut input = options
        .open(path)
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
    let opened_metadata = input
        .metadata()
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation: "inspect opened version-5 catalog evidence",
            path: path.to_path_buf(),
            source,
        })?;
    if !opened_metadata.is_file() {
        return Err(CatalogPublicationError::InvalidData {
            path: path.to_path_buf(),
            reason: "opened version-5 catalog evidence is not a regular file".to_owned(),
        });
    }
    let mut content = Vec::new();
    input
        .read_to_end(&mut content)
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
    Ok(Some(content))
}

fn normalize_embedded_catalog_source_version(value: &mut serde_json::Value) -> Result<(), String> {
    let base_version = value
        .get("catalog_base")
        .and_then(|image| image.get("version"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "embedded catalog base is missing unsigned integer `version`".to_owned())?;
    let target_version = value
        .get("catalog_target")
        .and_then(|image| image.get("version"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            "embedded catalog target is missing unsigned integer `version`".to_owned()
        })?;
    let source_version = match value.get("catalog_source_version") {
        Some(source) => source
            .as_u64()
            .ok_or_else(|| "`catalog_source_version` must be an unsigned integer".to_owned())?,
        None if base_version == target_version => {
            value
                .as_object_mut()
                .expect("catalog publication journal must be an object")
                .insert(
                    "catalog_source_version".to_owned(),
                    serde_json::Value::from(base_version),
                );
            base_version
        }
        None => {
            return Err(format!(
                "journal omits `catalog_source_version`, but base schema {base_version} does not match target schema {target_version}"
            ));
        }
    };
    if base_version != target_version {
        return Err(format!(
            "embedded catalog base schema {base_version} does not match target schema {target_version}"
        ));
    }
    if base_version != source_version && base_version != u64::from(CATALOG_VERSION) {
        return Err(format!(
            "catalog source schema {source_version} cannot own normalized image schema {base_version}; expected {source_version} or {CATALOG_VERSION}"
        ));
    }
    if source_version > u64::from(u32::MAX) {
        return Err(format!(
            "catalog source schema {source_version} exceeds the supported integer range"
        ));
    }
    Ok(())
}

fn validate_embedded_catalog_agent_bindings(value: &serde_json::Value) -> Result<(), String> {
    for image_name in ["catalog_base", "catalog_target"] {
        let Some(image) = value.get(image_name) else {
            continue;
        };
        let Some(version) = image.get("version").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        if (FIRST_CATALOG_VERSION_REQUIRING_AGENT_BINDINGS..=u64::from(CATALOG_VERSION))
            .contains(&version)
            && image.get("agent_bindings").is_none()
        {
            return Err(format!(
                "embedded {image_name} at catalog schema version {version} is missing `agent_bindings`"
            ));
        }
        if (u64::from(OLDEST_REPLAYABLE_CATALOG_VERSION)..u64::from(CATALOG_VERSION))
            .contains(&version)
            && (image.get("published_services").is_some()
                || image.get("retired_configuration_revisions").is_some()
                || image
                    .get("instances")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|instances| {
                        instances
                            .values()
                            .any(|record| record.get("configuration_revision").is_some())
                    }))
        {
            return Err(format!(
                "embedded predecessor {image_name} at catalog schema version {version} contains version-{CATALOG_VERSION} published-declaration fields"
            ));
        }
        if version == u64::from(CATALOG_VERSION) {
            if image.get("published_services").is_none() {
                return Err(format!(
                    "embedded {image_name} at catalog schema version {version} is missing `published_services`"
                ));
            }
            if image.get("retired_configuration_revisions").is_none() {
                return Err(format!(
                    "embedded {image_name} at catalog schema version {version} is missing `retired_configuration_revisions`"
                ));
            }
            let instances = image
                .get("instances")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| format!("embedded {image_name} is missing object `instances`"))?;
            if let Some(instance_id) = instances.iter().find_map(|(instance_id, record)| {
                record
                    .get("configuration_revision")
                    .is_none()
                    .then_some(instance_id)
            }) {
                return Err(format!(
                    "embedded {image_name} instance `{instance_id}` is missing `configuration_revision`"
                ));
            }
        }
    }
    Ok(())
}

async fn write_transaction(
    path: &Path,
    transaction: &CatalogPublicationTransaction,
    operation: &'static str,
) -> Result<(), CatalogPublicationError> {
    let mut content = serde_json::to_vec_pretty(transaction).map_err(|source| {
        CatalogPublicationError::InvalidData {
            path: path.to_path_buf(),
            reason: source.to_string(),
        }
    })?;
    content.push(b'\n');
    write_atomic_bytes(path, &content, operation).await
}

async fn write_atomic_bytes(
    path: &Path,
    content: &[u8],
    operation: &'static str,
) -> Result<(), CatalogPublicationError> {
    let parent = path
        .parent()
        .ok_or_else(|| CatalogPublicationError::InvalidData {
            path: path.to_path_buf(),
            reason: "journal path has no parent directory".to_owned(),
        })?;
    ensure_parent(path).await?;
    let temporary = parent.join(format!(".{JOURNAL_FILE}.{}.tmp", Uuid::new_v4()));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation: "create temporary catalog publication journal",
            path: temporary.clone(),
            source,
        })?;
    let write_result = async {
        output.write_all(content).await?;
        output.sync_all().await
    }
    .await;
    drop(output);
    if let Err(source) = write_result {
        let _ = fs::remove_file(&temporary).await;
        return Err(CatalogPublicationError::Io {
            operation: "write and sync temporary catalog publication journal",
            path: temporary,
            source,
        });
    }
    if let Err(source) = fs::rename(&temporary, path).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(CatalogPublicationError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        });
    }
    sync_parent(path, operation).await
}

async fn ensure_parent(path: &Path) -> Result<(), CatalogPublicationError> {
    let parent = path
        .parent()
        .ok_or_else(|| CatalogPublicationError::InvalidData {
            path: path.to_path_buf(),
            reason: "journal path has no parent directory".to_owned(),
        })?;
    let mut missing = Vec::new();
    let mut existing_ancestor = parent;
    loop {
        match fs::try_exists(existing_ancestor).await {
            Ok(true) => break,
            Ok(false) => missing.push(existing_ancestor.to_path_buf()),
            Err(source) => {
                return Err(CatalogPublicationError::Io {
                    operation: "inspect catalog publication journal directory",
                    path: existing_ancestor.to_path_buf(),
                    source,
                });
            }
        }
        existing_ancestor =
            existing_ancestor
                .parent()
                .ok_or_else(|| CatalogPublicationError::InvalidData {
                    path: path.to_path_buf(),
                    reason: "could not find an existing directory ancestor".to_owned(),
                })?;
    }
    fs::create_dir_all(parent)
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation: "create catalog publication journal directory",
            path: parent.to_path_buf(),
            source,
        })?;
    if !missing.is_empty() {
        for directory in &missing {
            sync_directory_before_publication(directory).await?;
        }
        sync_directory_before_publication(existing_ancestor).await?;
    }
    Ok(())
}

async fn sync_directory_before_publication(path: &Path) -> Result<(), CatalogPublicationError> {
    let directory = fs::File::open(path)
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation: "open newly created catalog publication directory for sync",
            path: path.to_path_buf(),
            source,
        })?;
    directory
        .sync_all()
        .await
        .map_err(|source| CatalogPublicationError::Io {
            operation: "sync newly created catalog publication directory",
            path: path.to_path_buf(),
            source,
        })
}

async fn sync_parent(path: &Path, operation: &'static str) -> Result<(), CatalogPublicationError> {
    let parent = path
        .parent()
        .ok_or_else(|| CatalogPublicationError::InvalidData {
            path: path.to_path_buf(),
            reason: "journal path has no parent directory".to_owned(),
        })?;
    let directory = fs::File::open(parent).await.map_err(|source| {
        CatalogPublicationError::PublishedNotDurable {
            operation,
            path: path.to_path_buf(),
            reason: source.to_string(),
        }
    })?;
    directory
        .sync_all()
        .await
        .map_err(|source| CatalogPublicationError::PublishedNotDurable {
            operation,
            path: path.to_path_buf(),
            reason: source.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::{
        CATALOG_PUBLICATION_VERSION, CATALOG_V5_BACKUP_FILE, CatalogPublicationError,
        CatalogPublicationJournal, CatalogPublicationPhase, CatalogPublicationTransaction,
        FIRST_CATALOG_VERSION_REQUIRING_AGENT_BINDINGS, OLDEST_REPLAYABLE_CATALOG_VERSION,
        catalog_schema_version, ensure_v5_backup, ensure_v5_recovery_backup, host_set_for_catalog,
    };
    use locald_core::ProjectCatalog;
    use locald_core::catalog::CATALOG_VERSION;
    use locald_hosts::HostSet;
    use serde_json::Value;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn fixture() -> (
        TempDir,
        CatalogPublicationJournal,
        CatalogPublicationTransaction,
    ) {
        let temporary = tempfile::tempdir().expect("create temporary publication directory");
        let catalog_path = temporary.path().join("catalog.json");
        let catalog = ProjectCatalog::with_path(catalog_path.clone());
        let catalog_hosts =
            host_set_for_catalog(&catalog).expect("derive platform-specific catalog host set");
        let journal = CatalogPublicationJournal::for_catalog_path(&catalog_path)
            .expect("locate catalog publication journal");
        let transaction = CatalogPublicationTransaction::new(
            catalog.clone(),
            catalog,
            &catalog_hosts,
            &catalog_hosts,
        )
        .expect("build publication transaction");
        (temporary, journal, transaction)
    }

    fn v5_catalog_bytes(catalog: &ProjectCatalog) -> Vec<u8> {
        let mut value = serde_json::to_value(catalog).expect("serialize current catalog");
        value["version"] = Value::from(CATALOG_VERSION - 1);
        value
            .as_object_mut()
            .expect("catalog object")
            .remove("published_services");
        value
            .as_object_mut()
            .expect("catalog object")
            .remove("retired_configuration_revisions");
        for instance in value["instances"]
            .as_object_mut()
            .expect("catalog instances")
            .values_mut()
        {
            instance
                .as_object_mut()
                .expect("catalog instance")
                .remove("configuration_revision");
        }
        let mut bytes = serde_json::to_vec_pretty(&value).expect("serialize v5 catalog");
        bytes.push(b'\n');
        bytes
    }

    #[tokio::test]
    async fn v5_backup_is_create_once_and_content_verified() {
        let temporary = tempfile::tempdir().expect("create migration fixture");
        let catalog_path = temporary.path().join("catalog.json");
        let catalog = ProjectCatalog::with_path(catalog_path.clone());
        let content = v5_catalog_bytes(&catalog);
        tokio::fs::write(&catalog_path, &content)
            .await
            .expect("write v5 catalog");

        assert_eq!(
            catalog_schema_version(&catalog_path)
                .await
                .expect("inspect v5 schema"),
            Some(u64::from(CATALOG_VERSION - 1))
        );
        let backup = ensure_v5_backup(&catalog_path)
            .await
            .expect("create exact v5 backup");
        assert_eq!(backup, temporary.path().join(CATALOG_V5_BACKUP_FILE));
        assert_eq!(
            tokio::fs::read(&backup).await.expect("read v5 backup"),
            content
        );
        ensure_v5_backup(&catalog_path)
            .await
            .expect("confirm matching v5 backup");

        tokio::fs::write(&backup, b"conflict")
            .await
            .expect("replace backup with conflicting evidence");
        let error = ensure_v5_backup(&catalog_path)
            .await
            .expect_err("conflicting backup must fail closed");
        assert!(matches!(
            error,
            CatalogPublicationError::BackupConflict { .. }
        ));
        assert_eq!(
            tokio::fs::read(&catalog_path)
                .await
                .expect("read preserved migration source"),
            content
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn v5_backup_rejects_symlink_without_touching_source_or_target() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("create symlink backup fixture");
        let catalog_path = temporary.path().join("catalog.json");
        let catalog = ProjectCatalog::with_path(catalog_path.clone());
        let content = v5_catalog_bytes(&catalog);
        tokio::fs::write(&catalog_path, &content)
            .await
            .expect("write v5 source");
        let backup_path = temporary.path().join(CATALOG_V5_BACKUP_FILE);
        symlink(&catalog_path, &backup_path).expect("symlink backup to catalog source");

        ensure_v5_backup(&catalog_path)
            .await
            .expect_err("symlink backup must fail closed");
        assert!(
            tokio::fs::symlink_metadata(&backup_path)
                .await
                .expect("inspect preserved backup symlink")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            tokio::fs::read(&catalog_path)
                .await
                .expect("read preserved catalog source"),
            content
        );
    }

    #[tokio::test]
    async fn v5_migration_journal_round_trips_equal_logical_images() {
        let temporary = tempfile::tempdir().expect("create migration journal fixture");
        let catalog_path = temporary.path().join("catalog.json");
        let catalog = ProjectCatalog::with_path(catalog_path.clone());
        let hosts = host_set_for_catalog(&catalog).expect("derive migration hosts");
        let transaction = CatalogPublicationTransaction::new_v5_migration(catalog, &hosts)
            .expect("build v5 migration transaction");
        assert!(transaction.is_schema_migration());
        let journal = CatalogPublicationJournal::for_catalog_path(&catalog_path)
            .expect("locate migration journal");
        journal
            .create(&transaction)
            .await
            .expect("create migration journal");
        assert_eq!(
            journal
                .load(&catalog_path)
                .await
                .expect("load migration journal"),
            Some(transaction)
        );
    }

    #[tokio::test]
    async fn predecessor_authored_journal_retains_source_provenance_and_backup() {
        let temporary = tempfile::tempdir().expect("create predecessor journal fixture");
        let catalog_path = temporary.path().join("catalog.json");
        let catalog = ProjectCatalog::with_path(catalog_path.clone());
        let raw_v5 = v5_catalog_bytes(&catalog);
        tokio::fs::write(&catalog_path, &raw_v5)
            .await
            .expect("write predecessor catalog");
        let hosts = host_set_for_catalog(&catalog).expect("derive predecessor hosts");
        let transaction =
            CatalogPublicationTransaction::new(catalog.clone(), catalog, &hosts, &hosts)
                .expect("build predecessor-era transaction shape");
        let journal = CatalogPublicationJournal::for_catalog_path(&catalog_path)
            .expect("locate predecessor journal");
        let mut value = serde_json::to_value(transaction).expect("serialize predecessor journal");
        let object = value
            .as_object_mut()
            .expect("journal JSON must be an object");
        object.remove("catalog_source_version");
        object.remove("schema_migration");
        for image_name in ["catalog_base", "catalog_target"] {
            let image = object[image_name]
                .as_object_mut()
                .unwrap_or_else(|| panic!("{image_name} must be an object"));
            image.insert("version".to_owned(), Value::from(CATALOG_VERSION - 1));
            image.remove("published_services");
            image.remove("retired_configuration_revisions");
            for record in image["instances"]
                .as_object_mut()
                .expect("catalog instances must be an object")
                .values_mut()
            {
                record
                    .as_object_mut()
                    .expect("catalog instance must be an object")
                    .remove("configuration_revision");
            }
        }
        let mut bytes = serde_json::to_vec_pretty(&value).expect("encode predecessor journal");
        bytes.push(b'\n');
        tokio::fs::write(&journal.journal_path, bytes)
            .await
            .expect("write predecessor journal");

        let loaded = journal
            .load(&catalog_path)
            .await
            .expect("load predecessor journal")
            .expect("predecessor journal exists");
        assert!(loaded.source_requires_v5_backup());
        assert!(!loaded.is_schema_migration());
        let backup = ensure_v5_recovery_backup(
            &catalog_path,
            &[loaded.catalog_base(), loaded.catalog_target()],
        )
        .await
        .expect("preserve predecessor source before recovery");
        assert_eq!(
            tokio::fs::read(backup)
                .await
                .expect("read predecessor backup"),
            raw_v5
        );
    }

    #[tokio::test]
    async fn predecessor_recovery_rejects_an_unrelated_valid_v5_backup() {
        let temporary = tempfile::tempdir().expect("create stale-backup fixture");
        let catalog_path = temporary.path().join("catalog.json");
        let project_path = temporary.path().join("project");
        std::fs::create_dir(&project_path).expect("create catalogued project");
        let mut expected = ProjectCatalog::with_path(catalog_path.clone());
        expected
            .register_project(
                ProjectCatalog::discover(project_path)
                    .await
                    .expect("discover catalogued project"),
                Some("expected".to_owned()),
            )
            .expect("register expected project");
        expected.save().await.expect("persist current catalog");

        let unrelated = ProjectCatalog::with_path(catalog_path.clone());
        let backup_path = temporary.path().join(CATALOG_V5_BACKUP_FILE);
        let unrelated_bytes = v5_catalog_bytes(&unrelated);
        tokio::fs::write(&backup_path, &unrelated_bytes)
            .await
            .expect("write unrelated valid v5 backup");

        let error = ensure_v5_recovery_backup(&catalog_path, &[&expected])
            .await
            .expect_err("unrelated backup must not authorize predecessor recovery");
        assert!(
            error
                .to_string()
                .contains("does not match either catalog image")
        );
        assert_eq!(
            tokio::fs::read(&backup_path)
                .await
                .expect("read preserved unrelated backup"),
            unrelated_bytes
        );
        assert_eq!(
            catalog_schema_version(&catalog_path)
                .await
                .expect("inspect preserved live catalog"),
            Some(u64::from(CATALOG_VERSION))
        );
    }

    #[tokio::test]
    async fn predecessor_recovery_rejects_a_logically_matching_v6_backup() {
        let temporary = tempfile::tempdir().expect("create v6-backup fixture");
        let catalog_path = temporary.path().join("catalog.json");
        let catalog = ProjectCatalog::with_path(catalog_path.clone());
        catalog.save().await.expect("persist current catalog");
        let backup_path = temporary.path().join(CATALOG_V5_BACKUP_FILE);
        let mut matching_v6 =
            serde_json::to_vec_pretty(&catalog).expect("serialize logically matching v6 backup");
        matching_v6.push(b'\n');
        tokio::fs::write(&backup_path, &matching_v6)
            .await
            .expect("write logically matching v6 backup");

        let error = ensure_v5_recovery_backup(&catalog_path, &[&catalog])
            .await
            .expect_err("v6 bytes cannot stand in for exact predecessor evidence");
        assert!(
            error
                .to_string()
                .contains("must contain raw catalog schema 5")
        );
        assert_eq!(
            tokio::fs::read(&backup_path)
                .await
                .expect("read preserved v6 backup"),
            matching_v6
        );
    }

    #[tokio::test]
    async fn journal_round_trip_advances_and_clears() {
        let (_temporary, journal, transaction) = fixture();
        let generation = transaction.target_generation();
        let catalog_path = transaction.catalog_path().to_path_buf();

        journal.create(&transaction).await.expect("create journal");
        journal
            .create(&transaction)
            .await
            .expect("confirm prepared journal durably");
        let prepared = journal
            .load(&catalog_path)
            .await
            .expect("load journal")
            .expect("journal exists");
        assert_eq!(prepared, transaction);

        journal
            .advance(
                generation,
                CatalogPublicationPhase::Prepared,
                CatalogPublicationPhase::HostsApplied,
                &catalog_path,
            )
            .await
            .expect("advance hosts phase");
        journal
            .advance(
                generation,
                CatalogPublicationPhase::Prepared,
                CatalogPublicationPhase::HostsApplied,
                &catalog_path,
            )
            .await
            .expect("confirm hosts phase durably");
        journal
            .advance(
                generation,
                CatalogPublicationPhase::HostsApplied,
                CatalogPublicationPhase::StateCommitted,
                &catalog_path,
            )
            .await
            .expect("advance state phase");
        journal
            .clear(generation, &catalog_path)
            .await
            .expect("clear journal");
        journal
            .clear(generation, &catalog_path)
            .await
            .expect("confirm cleared journal durably");
        assert!(
            journal
                .load(&catalog_path)
                .await
                .expect("reload cleared journal")
                .is_none()
        );
    }

    #[tokio::test]
    async fn aborted_journal_is_the_only_other_clearable_terminal_phase() {
        let (_temporary, journal, transaction) = fixture();
        let generation = transaction.target_generation();
        let catalog_path = transaction.catalog_path().to_path_buf();
        journal.create(&transaction).await.expect("create journal");
        journal
            .advance(
                generation,
                CatalogPublicationPhase::Prepared,
                CatalogPublicationPhase::Aborted,
                &catalog_path,
            )
            .await
            .expect("record abort");
        journal
            .clear(generation, &catalog_path)
            .await
            .expect("clear abort");

        let (_temporary, journal, transaction) = fixture();
        let generation = transaction.target_generation();
        let catalog_path = transaction.catalog_path().to_path_buf();
        journal.create(&transaction).await.expect("create journal");
        journal
            .advance(
                generation,
                CatalogPublicationPhase::Prepared,
                CatalogPublicationPhase::HostsApplied,
                &catalog_path,
            )
            .await
            .expect("record applied hosts");
        journal
            .advance(
                generation,
                CatalogPublicationPhase::HostsApplied,
                CatalogPublicationPhase::Aborted,
                &catalog_path,
            )
            .await
            .expect("abort before catalog commit");
        journal
            .clear(generation, &catalog_path)
            .await
            .expect("clear hosts-applied abort");
    }

    #[tokio::test]
    async fn competing_generation_and_invalid_transition_are_rejected() {
        let (_temporary, journal, transaction) = fixture();
        let catalog_path = transaction.catalog_path().to_path_buf();
        journal.create(&transaction).await.expect("create journal");
        let error = journal
            .advance(
                Uuid::new_v4(),
                CatalogPublicationPhase::Prepared,
                CatalogPublicationPhase::HostsApplied,
                &catalog_path,
            )
            .await
            .expect_err("reject another generation");
        assert!(matches!(
            error,
            CatalogPublicationError::TransactionMismatch { .. }
        ));
        let error = journal
            .advance(
                transaction.target_generation(),
                CatalogPublicationPhase::Prepared,
                CatalogPublicationPhase::StateCommitted,
                &catalog_path,
            )
            .await
            .expect_err("reject skipped phase");
        assert!(matches!(
            error,
            CatalogPublicationError::InvalidPhaseTransition { .. }
        ));
    }

    #[test]
    fn transaction_rejects_host_sets_that_do_not_match_catalog_images() {
        let temporary = tempfile::tempdir().expect("create temporary publication directory");
        let catalog = ProjectCatalog::with_path(temporary.path().join("catalog.json"));
        let catalog_hosts =
            host_set_for_catalog(&catalog).expect("derive platform-specific catalog host set");
        let unrelated_hosts =
            HostSet::try_from_strings(["custom.example"]).expect("construct unrelated host set");
        let error = CatalogPublicationTransaction::new(
            catalog.clone(),
            catalog,
            &catalog_hosts,
            &unrelated_hosts,
        )
        .expect_err("reject host/catalog mismatch");
        assert!(
            error
                .to_string()
                .contains("candidate host set does not match")
        );
    }

    #[tokio::test]
    async fn supported_catalog_images_without_agent_bindings_are_preserved_and_rejected() {
        let (_temporary, journal, transaction) = fixture();
        let catalog_path = transaction.catalog_path().to_path_buf();
        for version in [
            FIRST_CATALOG_VERSION_REQUIRING_AGENT_BINDINGS,
            u64::from(CATALOG_VERSION),
        ] {
            let mut value = serde_json::to_value(&transaction).expect("serialize transaction");
            if version < u64::from(CATALOG_VERSION) {
                let object = value
                    .as_object_mut()
                    .expect("catalog publication journal object");
                object.remove("catalog_source_version");
                object.remove("schema_migration");
                for image_name in ["catalog_base", "catalog_target"] {
                    let image = object[image_name]
                        .as_object_mut()
                        .unwrap_or_else(|| panic!("{image_name} must be an object"));
                    image.insert("version".to_owned(), Value::from(version));
                    image.remove("published_services");
                    image.remove("retired_configuration_revisions");
                    for record in image["instances"]
                        .as_object_mut()
                        .expect("catalog instances")
                        .values_mut()
                    {
                        record
                            .as_object_mut()
                            .expect("catalog instance")
                            .remove("configuration_revision");
                    }
                }
            }
            let catalog_base = value["catalog_base"]
                .as_object_mut()
                .expect("catalog base object");
            catalog_base.insert("version".to_owned(), Value::from(version));
            catalog_base.remove("agent_bindings");
            let bytes = serde_json::to_vec_pretty(&value).expect("encode malformed transaction");
            tokio::fs::write(journal.journal_path(), &bytes)
                .await
                .expect("write malformed journal");

            let before = tokio::fs::read(journal.journal_path())
                .await
                .expect("read malformed journal before load");
            let error = journal
                .load(&catalog_path)
                .await
                .expect_err("reject missing required binding map");
            let after = tokio::fs::read(journal.journal_path())
                .await
                .expect("read malformed journal after load");
            assert_eq!(before, after);
            assert!(error.to_string().contains("agent_bindings"));
            assert!(error.to_string().contains(&version.to_string()));
        }
    }

    #[tokio::test]
    async fn predecessor_catalog_images_with_v6_fields_are_preserved_and_rejected() {
        let (_temporary, journal, transaction) = fixture();
        let catalog_path = transaction.catalog_path().to_path_buf();
        for version in OLDEST_REPLAYABLE_CATALOG_VERSION..CATALOG_VERSION {
            let mut value = serde_json::to_value(&transaction).expect("serialize transaction");
            let object = value
                .as_object_mut()
                .expect("catalog publication journal object");
            object.remove("catalog_source_version");
            object.remove("schema_migration");
            for image_name in ["catalog_base", "catalog_target"] {
                let image = object[image_name]
                    .as_object_mut()
                    .unwrap_or_else(|| panic!("{image_name} must be an object"));
                image.insert("version".to_owned(), Value::from(version));
                image.remove("retired_configuration_revisions");
                image.insert("published_services".to_owned(), serde_json::json!({}));
            }
            let bytes =
                serde_json::to_vec_pretty(&value).expect("encode malformed predecessor journal");
            tokio::fs::write(journal.journal_path(), &bytes)
                .await
                .expect("write malformed predecessor journal");

            let error = journal
                .load(&catalog_path)
                .await
                .expect_err("predecessor images with v6 fields must fail closed");
            let after = tokio::fs::read(journal.journal_path())
                .await
                .expect("read preserved malformed journal");
            assert_eq!(after, bytes);
            assert!(error.to_string().contains("published-declaration fields"));
            assert!(error.to_string().contains(&version.to_string()));
        }
    }

    #[tokio::test]
    async fn unsupported_version_and_wrong_live_path_fail_without_rewrite() {
        let (temporary, journal, transaction) = fixture();
        let catalog_path = transaction.catalog_path().to_path_buf();
        let mut value = serde_json::to_value(&transaction).expect("serialize transaction");
        value["version"] = Value::from(u64::from(CATALOG_PUBLICATION_VERSION) + 1);
        let bytes = serde_json::to_vec_pretty(&value).expect("encode future transaction");
        tokio::fs::write(journal.journal_path(), &bytes)
            .await
            .expect("write future journal");
        let error = journal
            .load(&catalog_path)
            .await
            .expect_err("reject future journal");
        assert!(matches!(
            error,
            CatalogPublicationError::UnsupportedVersion { .. }
        ));

        tokio::fs::write(
            journal.journal_path(),
            serde_json::to_vec_pretty(&transaction).expect("encode transaction"),
        )
        .await
        .expect("restore journal");
        let error = journal
            .load(&temporary.path().join("other-catalog.json"))
            .await
            .expect_err("reject wrong live catalog path");
        assert!(error.to_string().contains("does not match live path"));
    }

    #[test]
    fn nil_generation_and_state_committed_missing_catalog_are_invalid() {
        let (_temporary, _journal, mut transaction) = fixture();
        transaction.target_generation = Uuid::nil();
        assert!(transaction.validate().is_err());

        transaction.target_generation = Uuid::new_v4();
        transaction.phase = CatalogPublicationPhase::StateCommitted;
        let error = transaction
            .catalog_for_missing_storage()
            .expect_err("committed state requires durable catalog storage");
        assert!(error.to_string().contains("state_committed"));
    }
}
