//! Durable, replayable lifecycle transactions.
//!
//! The manager owns lifecycle locking and recovery. This module only persists
//! exact before/after images and the phase that recovery must resume from.

#![allow(clippy::redundant_pub_crate)] // Explicitly mark the crate-internal journal surface.

use locald_core::attachments::AttachmentStoreSnapshot;
use locald_core::{PreparedAvailabilityBatch, ProjectCatalog};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub(crate) const LIFECYCLE_TRANSACTION_VERSION: u32 = 1;
pub(crate) const V1_MIGRATION_MARKER_VERSION: u32 = 1;

const JOURNAL_FILE: &str = "lifecycle-transaction.json";
const MIGRATION_MARKER_FILE: &str = "v1-migration-complete.json";
const V1_BACKUP_DIRECTORY: &str = "v1-backups";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleTransactionKind {
    LegacyV1Migration,
    LifecycleMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleTransactionPhase {
    Prepared,
    CatalogPublished,
    AvailabilityPublished,
    CompatibilityPublished,
    Complete,
}

impl LifecycleTransactionPhase {
    #[must_use]
    pub(crate) const fn next(self) -> Option<Self> {
        match self {
            Self::Prepared => Some(Self::CatalogPublished),
            Self::CatalogPublished => Some(Self::AvailabilityPublished),
            Self::AvailabilityPublished => Some(Self::CompatibilityPublished),
            Self::CompatibilityPublished => Some(Self::Complete),
            Self::Complete => None,
        }
    }

    #[must_use]
    pub(crate) const fn requires_catalog_target(self) -> bool {
        !matches!(self, Self::Prepared)
    }

    #[must_use]
    pub(crate) const fn requires_availability_targets(self) -> bool {
        matches!(
            self,
            Self::AvailabilityPublished | Self::CompatibilityPublished | Self::Complete
        )
    }

    #[must_use]
    pub(crate) const fn requires_compatibility_target(self) -> bool {
        matches!(self, Self::CompatibilityPublished | Self::Complete)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogTransactionImages {
    storage_path: PathBuf,
    base: ProjectCatalog,
    target: ProjectCatalog,
}

impl CatalogTransactionImages {
    pub(crate) fn new(
        base: ProjectCatalog,
        target: ProjectCatalog,
    ) -> Result<Self, LifecycleJournalError> {
        if base.storage_path() != target.storage_path() {
            return Err(LifecycleJournalError::InvalidPlan {
                reason: format!(
                    "catalog base path `{}` does not match target path `{}`",
                    base.storage_path().display(),
                    target.storage_path().display()
                ),
            });
        }
        let storage_path = base.storage_path().to_path_buf();
        Ok(Self {
            storage_path,
            base,
            target,
        })
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    #[must_use]
    pub(crate) const fn base(&self) -> &ProjectCatalog {
        &self.base
    }

    #[must_use]
    pub(crate) const fn target(&self) -> &ProjectCatalog {
        &self.target
    }

    pub(crate) fn normalize_storage_path(&mut self, storage_path: &Path) {
        self.storage_path = storage_path.to_path_buf();
        self.base.set_storage_path(storage_path.to_path_buf());
        self.target.set_storage_path(storage_path.to_path_buf());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachmentTransactionImages {
    base: AttachmentStoreSnapshot,
    target: AttachmentStoreSnapshot,
}

impl AttachmentTransactionImages {
    #[must_use]
    pub(crate) const fn new(
        base: AttachmentStoreSnapshot,
        target: AttachmentStoreSnapshot,
    ) -> Self {
        Self { base, target }
    }

    #[must_use]
    pub(crate) const fn base(&self) -> &AttachmentStoreSnapshot {
        &self.base
    }

    #[must_use]
    pub(crate) const fn target(&self) -> &AttachmentStoreSnapshot {
        &self.target
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LifecycleTransaction {
    version: u32,
    id: Uuid,
    effective_at: SystemTime,
    kind: LifecycleTransactionKind,
    phase: LifecycleTransactionPhase,
    catalog: Option<CatalogTransactionImages>,
    availability: Vec<PreparedAvailabilityBatch>,
    attachments: AttachmentTransactionImages,
}

impl LifecycleTransaction {
    pub(crate) fn new(
        kind: LifecycleTransactionKind,
        effective_at: SystemTime,
        catalog: Option<CatalogTransactionImages>,
        availability: Vec<PreparedAvailabilityBatch>,
        attachments: AttachmentTransactionImages,
    ) -> Result<Self, LifecycleJournalError> {
        let transaction = Self {
            version: LIFECYCLE_TRANSACTION_VERSION,
            id: Uuid::new_v4(),
            effective_at,
            kind,
            phase: LifecycleTransactionPhase::Prepared,
            catalog,
            availability,
            attachments,
        };
        transaction.validate()?;
        Ok(transaction)
    }

    #[must_use]
    pub(crate) const fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub(crate) const fn effective_at(&self) -> SystemTime {
        self.effective_at
    }

    #[must_use]
    pub(crate) const fn kind(&self) -> LifecycleTransactionKind {
        self.kind
    }

    #[must_use]
    pub(crate) const fn phase(&self) -> LifecycleTransactionPhase {
        self.phase
    }

    #[must_use]
    pub(crate) const fn catalog(&self) -> Option<&CatalogTransactionImages> {
        self.catalog.as_ref()
    }

    #[must_use]
    pub(crate) fn availability(&self) -> &[PreparedAvailabilityBatch] {
        &self.availability
    }

    #[must_use]
    pub(crate) const fn attachments(&self) -> &AttachmentTransactionImages {
        &self.attachments
    }

    pub(crate) fn normalize_catalog_storage_path(&mut self, storage_path: &Path) {
        if let Some(catalog) = &mut self.catalog {
            catalog.normalize_storage_path(storage_path);
        }
    }

    fn normalize_deserialized_paths(&mut self) -> Result<(), LifecycleJournalError> {
        if let Some(catalog) = &mut self.catalog {
            let storage_path = catalog.storage_path.clone();
            catalog.normalize_storage_path(&storage_path);
            catalog.base.upgrade_embedded_schema().map_err(|error| {
                LifecycleJournalError::InvalidPlan {
                    reason: format!("failed to upgrade embedded catalog base: {error}"),
                }
            })?;
            catalog.target.upgrade_embedded_schema().map_err(|error| {
                LifecycleJournalError::InvalidPlan {
                    reason: format!("failed to upgrade embedded catalog target: {error}"),
                }
            })?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), LifecycleJournalError> {
        if self.version != LIFECYCLE_TRANSACTION_VERSION {
            return Err(LifecycleJournalError::InvalidPlan {
                reason: format!("unexpected transaction version {}", self.version),
            });
        }
        if self.id.is_nil() {
            return Err(LifecycleJournalError::InvalidPlan {
                reason: "transaction ID must not be nil".to_owned(),
            });
        }
        if let Some(catalog) = &self.catalog {
            if catalog.base.storage_path() != catalog.storage_path
                || catalog.target.storage_path() != catalog.storage_path
            {
                return Err(LifecycleJournalError::InvalidPlan {
                    reason: "catalog images are not aligned to their recorded storage path"
                        .to_owned(),
                });
            }
            catalog
                .base
                .validate()
                .map_err(|error| LifecycleJournalError::InvalidPlan {
                    reason: format!("invalid catalog base: {error}"),
                })?;
            catalog
                .target
                .validate()
                .map_err(|error| LifecycleJournalError::InvalidPlan {
                    reason: format!("invalid catalog target: {error}"),
                })?;
        }
        let mut availability_instances = BTreeSet::new();
        for batch in &self.availability {
            if !availability_instances.insert(batch.project_instance_id()) {
                return Err(LifecycleJournalError::InvalidPlan {
                    reason: format!(
                        "more than one availability batch targets {}",
                        batch.project_instance_id()
                    ),
                });
            }
            batch
                .validate()
                .map_err(|error| LifecycleJournalError::InvalidPlan {
                    reason: format!(
                        "invalid prepared availability batch for {}: {error}",
                        batch.project_instance_id()
                    ),
                })?;
            if batch.batch().effective_at() != self.effective_at {
                return Err(LifecycleJournalError::InvalidPlan {
                    reason: format!(
                        "availability batch for {} does not use the transaction effective time",
                        batch.project_instance_id()
                    ),
                });
            }
        }
        if self.kind == LifecycleTransactionKind::LegacyV1Migration {
            let catalog =
                self.catalog
                    .as_ref()
                    .ok_or_else(|| LifecycleJournalError::InvalidPlan {
                        reason: "legacy v1 migration requires exact catalog images".to_owned(),
                    })?;
            let catalog_instances = catalog
                .target
                .instances
                .keys()
                .copied()
                .collect::<BTreeSet<_>>();
            if availability_instances != catalog_instances {
                return Err(LifecycleJournalError::InvalidPlan {
                    reason: format!(
                        "legacy v1 migration availability owners do not exactly match its {} catalog instances",
                        catalog_instances.len()
                    ),
                });
            }
        }
        // The base is exact legacy evidence. Older files can contain an
        // embedded attachment path that differs from its map key; migration
        // must be able to journal that state before normalizing the target.
        validate_attachment_snapshot(self.attachments.target(), "target")?;
        if let Some(catalog) = &self.catalog {
            validate_attachment_authority(self.attachments.target(), &catalog.target, "target")?;
        }
        Ok(())
    }
}

pub(crate) fn validate_attachment_authority(
    snapshot: &AttachmentStoreSnapshot,
    catalog: &ProjectCatalog,
    image: &'static str,
) -> Result<(), LifecycleJournalError> {
    validate_attachment_snapshot(snapshot, image)?;
    for (project_path, instance_id) in &snapshot.instance_owners {
        if !catalog.instances.contains_key(instance_id) {
            return Err(LifecycleJournalError::InvalidPlan {
                reason: format!(
                    "attachment {image} path `{}` is owned by uncatalogued project instance {instance_id}",
                    project_path.display()
                ),
            });
        }
    }
    Ok(())
}

fn validate_attachment_snapshot(
    snapshot: &AttachmentStoreSnapshot,
    image: &'static str,
) -> Result<(), LifecycleJournalError> {
    snapshot
        .validate_exact()
        .map_err(|error| LifecycleJournalError::InvalidPlan {
            reason: format!("attachment {image} image is invalid: {error}"),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LegacyV1File {
    Catalog,
    Registry,
    Attachments,
    RuntimeState,
}

impl LegacyV1File {
    const fn file_name(self) -> &'static str {
        match self {
            Self::Catalog => "catalog.json",
            Self::Registry => "registry.json",
            Self::Attachments => "attachments.json",
            Self::RuntimeState => "state.json",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct V1MigrationMarker {
    version: u32,
    transaction_id: Uuid,
    completed_at: SystemTime,
}

impl V1MigrationMarker {
    #[must_use]
    pub(crate) const fn transaction_id(&self) -> Uuid {
        self.transaction_id
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) const fn completed_at(&self) -> SystemTime {
        self.completed_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalCreateDisposition {
    Created,
    AlreadyCreated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalAdvanceDisposition {
    Advanced,
    AlreadyAdvanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalClearDisposition {
    Cleared,
    AlreadyClear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationMarkerDisposition {
    Created,
    AlreadyCreated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V1BackupDisposition {
    Created,
    AlreadyCreated,
    SourceMissing,
}

#[derive(Debug, Error)]
pub(crate) enum LifecycleJournalError {
    #[error("failed to {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{entity} `{path}` uses unsupported version {found}; expected {expected}")]
    UnsupportedVersion {
        entity: &'static str,
        path: PathBuf,
        found: u64,
        expected: u32,
    },
    #[error("invalid {entity} `{path}`: {reason}")]
    InvalidData {
        entity: &'static str,
        path: PathBuf,
        reason: String,
    },
    #[error("invalid lifecycle transaction plan: {reason}")]
    InvalidPlan { reason: String },
    #[error("invalid lifecycle recovery state: {reason}")]
    InvalidRecoveryState { reason: String },
    #[error("lifecycle journal `{path}` already contains transaction {existing}")]
    ActiveTransaction { path: PathBuf, existing: Uuid },
    #[error("lifecycle journal `{path}` does not exist")]
    MissingTransaction { path: PathBuf },
    #[error("lifecycle journal owns transaction {actual}, not requested transaction {requested}")]
    TransactionMismatch { requested: Uuid, actual: Uuid },
    #[error("transaction {transaction_id} is at phase {actual:?}, not expected phase {expected:?}")]
    PhaseMismatch {
        transaction_id: Uuid,
        expected: LifecycleTransactionPhase,
        actual: LifecycleTransactionPhase,
    },
    #[error("invalid lifecycle phase transition from {from:?} to {to:?}")]
    InvalidPhaseTransition {
        from: LifecycleTransactionPhase,
        to: LifecycleTransactionPhase,
    },
    #[error("transaction {transaction_id} cannot be cleared at phase {phase:?}")]
    IncompleteTransaction {
        transaction_id: Uuid,
        phase: LifecycleTransactionPhase,
    },
    #[error("v1 backup `{path}` already exists with different raw content")]
    BackupConflict { path: PathBuf },
    #[error(
        "legacy v1 source `{source_path}` is missing while its exact backup `{backup_path}` already exists"
    )]
    BackupSourceMissing {
        source_path: PathBuf,
        backup_path: PathBuf,
    },
    #[error("v1 migration marker already belongs to transaction {existing}")]
    MigrationMarkerConflict { existing: Uuid },
    #[error("{operation} published `{path}`, but its parent-directory sync failed: {reason}")]
    PublishedNotDurable {
        operation: &'static str,
        path: PathBuf,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct LifecycleJournal {
    journal_path: PathBuf,
    migration_marker_path: PathBuf,
    backup_directory: PathBuf,
}

#[derive(Debug)]
pub(crate) struct LifecycleRecoveryPreflight {
    transaction: Option<LifecycleTransaction>,
    marker: Option<V1MigrationMarker>,
}

impl LifecycleRecoveryPreflight {
    /// Return the exact catalog base recorded by a cold v1 migration that has
    /// not published its catalog yet.
    ///
    /// Startup uses this image when no catalog file exists so journal replay
    /// compares against the same generated identities that were prepared,
    /// rather than rediscovering legacy paths into a different catalog image.
    #[must_use]
    pub(crate) fn prepared_legacy_catalog_base(
        &self,
        storage_path: &Path,
    ) -> Option<ProjectCatalog> {
        if self.marker.is_some() {
            return None;
        }
        let mut catalog = self
            .transaction
            .as_ref()
            .filter(|transaction| {
                transaction.kind() == LifecycleTransactionKind::LegacyV1Migration
                    && transaction.phase() == LifecycleTransactionPhase::Prepared
            })
            .and_then(LifecycleTransaction::catalog)
            .map(CatalogTransactionImages::base)
            .cloned()?;
        catalog.set_storage_path(storage_path.to_path_buf());
        Some(catalog)
    }

    #[must_use]
    pub(crate) const fn has_v2_authority(&self) -> bool {
        self.marker.is_some()
            || match &self.transaction {
                Some(transaction) => transaction.phase().requires_catalog_target(),
                None => false,
            }
    }

    #[must_use]
    pub(crate) fn requires_exact_attachment_authority(&self) -> bool {
        self.marker.is_some()
            || self.transaction.as_ref().is_some_and(|transaction| {
                transaction.kind() == LifecycleTransactionKind::LifecycleMutation
                    || transaction.phase().requires_compatibility_target()
            })
    }

    /// Return the exact compatibility images that bound permissive v1 loading
    /// before a legacy migration publishes its compatibility target.
    #[must_use]
    pub(crate) fn pending_legacy_attachment_images(&self) -> Option<&AttachmentTransactionImages> {
        if self.marker.is_some() {
            return None;
        }
        self.transaction
            .as_ref()
            .filter(|transaction| {
                transaction.kind() == LifecycleTransactionKind::LegacyV1Migration
                    && !transaction.phase().requires_compatibility_target()
            })
            .map(LifecycleTransaction::attachments)
    }

    pub(crate) fn into_parts(self) -> (Option<LifecycleTransaction>, Option<V1MigrationMarker>) {
        (self.transaction, self.marker)
    }
}

impl LifecycleJournal {
    #[must_use]
    pub(crate) fn at(data_dir: &Path) -> Self {
        Self {
            journal_path: data_dir.join(JOURNAL_FILE),
            migration_marker_path: data_dir.join(MIGRATION_MARKER_FILE),
            backup_directory: data_dir.join(V1_BACKUP_DIRECTORY),
        }
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    pub(crate) async fn load(&self) -> Result<Option<LifecycleTransaction>, LifecycleJournalError> {
        let loaded: Option<LifecycleTransaction> = read_versioned(
            &self.journal_path,
            LIFECYCLE_TRANSACTION_VERSION,
            "lifecycle transaction",
        )
        .await?;
        let Some(mut transaction) = loaded else {
            return Ok(None);
        };
        transaction
            .normalize_deserialized_paths()
            .map_err(|error| LifecycleJournalError::InvalidData {
                entity: "lifecycle transaction",
                path: self.journal_path.clone(),
                reason: error.to_string(),
            })?;
        transaction
            .validate()
            .map_err(|error| LifecycleJournalError::InvalidData {
                entity: "lifecycle transaction",
                path: self.journal_path.clone(),
                reason: error.to_string(),
            })?;
        Ok(Some(transaction))
    }

    pub(crate) async fn preflight(
        &self,
    ) -> Result<LifecycleRecoveryPreflight, LifecycleJournalError> {
        // Parse both durable authorities before publishing either transaction
        // image. A malformed marker must not be discovered only after replay
        // has advanced or cleared the exact journal.
        let marker = self.migration_marker().await?;
        let transaction = self.load().await?;
        match (&transaction, &marker) {
            (Some(transaction), None)
                if transaction.kind() == LifecycleTransactionKind::LegacyV1Migration
                    && transaction.phase() == LifecycleTransactionPhase::Complete =>
            {
                return Err(LifecycleJournalError::InvalidRecoveryState {
                    reason: format!(
                        "completed v1 migration transaction {} is missing its completion marker",
                        transaction.id()
                    ),
                });
            }
            (Some(transaction), None)
                if transaction.kind() == LifecycleTransactionKind::LifecycleMutation =>
            {
                return Err(LifecycleJournalError::InvalidRecoveryState {
                    reason: format!(
                        "lifecycle mutation {} exists without the completed v1 migration marker",
                        transaction.id()
                    ),
                });
            }
            (Some(transaction), Some(marker))
                if transaction.kind() == LifecycleTransactionKind::LegacyV1Migration =>
            {
                if marker.transaction_id() != transaction.id() {
                    return Err(LifecycleJournalError::InvalidRecoveryState {
                        reason: format!(
                            "v1 migration marker belongs to {}, but active migration is {}",
                            marker.transaction_id(),
                            transaction.id()
                        ),
                    });
                }
                if !matches!(
                    transaction.phase(),
                    LifecycleTransactionPhase::CompatibilityPublished
                        | LifecycleTransactionPhase::Complete
                ) {
                    return Err(LifecycleJournalError::InvalidRecoveryState {
                        reason: format!(
                            "v1 migration marker exists while transaction {} is only at phase {:?}",
                            transaction.id(),
                            transaction.phase()
                        ),
                    });
                }
            }
            _ => {}
        }
        Ok(LifecycleRecoveryPreflight {
            transaction,
            marker,
        })
    }

    pub(crate) async fn create(
        &self,
        transaction: &LifecycleTransaction,
    ) -> Result<JournalCreateDisposition, LifecycleJournalError> {
        transaction.validate()?;
        if transaction.phase != LifecycleTransactionPhase::Prepared {
            return Err(LifecycleJournalError::InvalidPlan {
                reason: "a new lifecycle transaction must be in the prepared phase".to_owned(),
            });
        }
        if let Some(existing) = self.load().await? {
            if same_serialized_payload(&existing, transaction, &self.journal_path)? {
                self.write_transaction(&existing).await?;
                return Ok(JournalCreateDisposition::AlreadyCreated);
            }
            return Err(LifecycleJournalError::ActiveTransaction {
                path: self.journal_path.clone(),
                existing: existing.id,
            });
        }
        self.write_transaction(transaction).await?;
        Ok(JournalCreateDisposition::Created)
    }

    pub(crate) async fn advance(
        &self,
        transaction_id: Uuid,
        expected: LifecycleTransactionPhase,
        next: LifecycleTransactionPhase,
    ) -> Result<JournalAdvanceDisposition, LifecycleJournalError> {
        if expected.next() != Some(next) {
            return Err(LifecycleJournalError::InvalidPhaseTransition {
                from: expected,
                to: next,
            });
        }
        let mut transaction =
            self.load()
                .await?
                .ok_or_else(|| LifecycleJournalError::MissingTransaction {
                    path: self.journal_path.clone(),
                })?;
        if transaction.id != transaction_id {
            return Err(LifecycleJournalError::TransactionMismatch {
                requested: transaction_id,
                actual: transaction.id,
            });
        }
        if transaction.phase == next {
            self.write_transaction(&transaction).await?;
            return Ok(JournalAdvanceDisposition::AlreadyAdvanced);
        }
        if transaction.phase != expected {
            return Err(LifecycleJournalError::PhaseMismatch {
                transaction_id,
                expected,
                actual: transaction.phase,
            });
        }
        transaction.phase = next;
        self.write_transaction(&transaction).await?;
        Ok(JournalAdvanceDisposition::Advanced)
    }

    pub(crate) async fn clear(
        &self,
        transaction_id: Uuid,
    ) -> Result<JournalClearDisposition, LifecycleJournalError> {
        let Some(transaction) = self.load().await? else {
            ensure_parent(&self.journal_path).await?;
            sync_parent(&self.journal_path, "confirm cleared lifecycle transaction").await?;
            return Ok(JournalClearDisposition::AlreadyClear);
        };
        if transaction.id != transaction_id {
            return Err(LifecycleJournalError::TransactionMismatch {
                requested: transaction_id,
                actual: transaction.id,
            });
        }
        if transaction.phase != LifecycleTransactionPhase::Complete {
            return Err(LifecycleJournalError::IncompleteTransaction {
                transaction_id,
                phase: transaction.phase,
            });
        }
        fs::remove_file(&self.journal_path)
            .await
            .map_err(|source| LifecycleJournalError::Io {
                operation: "remove completed lifecycle transaction",
                path: self.journal_path.clone(),
                source,
            })?;
        sync_parent(&self.journal_path, "clear lifecycle transaction").await?;
        Ok(JournalClearDisposition::Cleared)
    }

    pub(crate) async fn migration_marker(
        &self,
    ) -> Result<Option<V1MigrationMarker>, LifecycleJournalError> {
        read_versioned(
            &self.migration_marker_path,
            V1_MIGRATION_MARKER_VERSION,
            "v1 migration marker",
        )
        .await
    }

    pub(crate) async fn mark_migration_complete(
        &self,
        transaction_id: Uuid,
        completed_at: SystemTime,
    ) -> Result<MigrationMarkerDisposition, LifecycleJournalError> {
        let marker = V1MigrationMarker {
            version: V1_MIGRATION_MARKER_VERSION,
            transaction_id,
            completed_at,
        };
        if let Some(existing) = self.migration_marker().await? {
            if existing != marker {
                return Err(LifecycleJournalError::MigrationMarkerConflict {
                    existing: existing.transaction_id,
                });
            }
            self.write_value(
                &self.migration_marker_path,
                &existing,
                "repair v1 migration marker durability",
            )
            .await?;
            return Ok(MigrationMarkerDisposition::AlreadyCreated);
        }
        self.write_value(
            &self.migration_marker_path,
            &marker,
            "publish v1 migration marker",
        )
        .await?;
        Ok(MigrationMarkerDisposition::Created)
    }

    pub(crate) async fn backup_v1_file(
        &self,
        kind: LegacyV1File,
        source_path: &Path,
    ) -> Result<V1BackupDisposition, LifecycleJournalError> {
        let backup_path = self.backup_directory.join(kind.file_name());
        let source =
            match read_optional_bytes(source_path, "read legacy v1 state for backup").await? {
                Some(source) => source,
                None => {
                    return match read_optional_bytes(
                        &backup_path,
                        "inspect existing v1 backup for missing source",
                    )
                    .await?
                    {
                        Some(_) => Err(LifecycleJournalError::BackupSourceMissing {
                            source_path: source_path.to_path_buf(),
                            backup_path,
                        }),
                        None => Ok(V1BackupDisposition::SourceMissing),
                    };
                }
            };
        match read_optional_bytes(&backup_path, "read existing v1 backup").await? {
            Some(existing) if existing == source => {
                sync_parent(&backup_path, "confirm v1 backup durability").await?;
                return Ok(V1BackupDisposition::AlreadyCreated);
            }
            Some(_) => return Err(LifecycleJournalError::BackupConflict { path: backup_path }),
            None => {}
        }
        persist_bytes_create_once(&backup_path, &source).await
    }

    async fn write_transaction(
        &self,
        transaction: &LifecycleTransaction,
    ) -> Result<(), LifecycleJournalError> {
        self.write_value(
            &self.journal_path,
            transaction,
            "publish lifecycle transaction",
        )
        .await
    }

    async fn write_value<T: Serialize + Sync>(
        &self,
        path: &Path,
        value: &T,
        operation: &'static str,
    ) -> Result<(), LifecycleJournalError> {
        let mut content = serde_json::to_vec_pretty(value).map_err(|source| {
            LifecycleJournalError::InvalidData {
                entity: "lifecycle journal value",
                path: path.to_path_buf(),
                reason: source.to_string(),
            }
        })?;
        content.push(b'\n');
        write_atomic_bytes(path, &content, operation).await
    }
}

fn same_serialized_payload<T: Serialize>(
    left: &T,
    right: &T,
    path: &Path,
) -> Result<bool, LifecycleJournalError> {
    let left = serde_json::to_value(left).map_err(|source| LifecycleJournalError::InvalidData {
        entity: "lifecycle transaction",
        path: path.to_path_buf(),
        reason: source.to_string(),
    })?;
    let right =
        serde_json::to_value(right).map_err(|source| LifecycleJournalError::InvalidData {
            entity: "lifecycle transaction",
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
    Ok(left == right)
}

async fn persist_bytes_create_once(
    path: &Path,
    content: &[u8],
) -> Result<V1BackupDisposition, LifecycleJournalError> {
    ensure_parent(path).await?;
    match read_optional_bytes(path, "read existing v1 backup").await? {
        Some(existing) if existing == content => {
            sync_parent(path, "confirm v1 backup durability").await?;
            return Ok(V1BackupDisposition::AlreadyCreated);
        }
        Some(_) => {
            return Err(LifecycleJournalError::BackupConflict {
                path: path.to_path_buf(),
            });
        }
        None => {}
    }

    let parent = path
        .parent()
        .ok_or_else(|| LifecycleJournalError::InvalidData {
            entity: "v1 backup path",
            path: path.to_path_buf(),
            reason: "path has no parent directory".to_owned(),
        })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("v1-backup");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| LifecycleJournalError::Io {
            operation: "create temporary v1 backup",
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
        return Err(LifecycleJournalError::Io {
            operation: "write and sync temporary v1 backup",
            path: temporary,
            source,
        });
    }

    let link_result = fs::hard_link(&temporary, path).await;
    finish_create_once_publication(path, content, &temporary, link_result).await
}

async fn finish_create_once_publication(
    path: &Path,
    content: &[u8],
    temporary: &Path,
    link_result: io::Result<()>,
) -> Result<V1BackupDisposition, LifecycleJournalError> {
    let cleanup_result = fs::remove_file(temporary).await;
    match link_result {
        Ok(()) => {
            // The create-once path is authoritative once the link succeeds.
            // A leftover private temporary file is recoverable debris rather
            // than a failed backup publication.
            let _ = cleanup_result;
            sync_parent(path, "create v1 backup").await?;
            Ok(V1BackupDisposition::Created)
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            // A matching existing path already satisfies idempotent replay.
            // Temporary cleanup remains best effort after publication wins.
            let _ = cleanup_result;
            let existing = fs::read(path)
                .await
                .map_err(|source| LifecycleJournalError::Io {
                    operation: "read concurrently created v1 backup",
                    path: path.to_path_buf(),
                    source,
                })?;
            if existing != content {
                return Err(LifecycleJournalError::BackupConflict {
                    path: path.to_path_buf(),
                });
            }
            sync_parent(path, "confirm v1 backup durability").await?;
            Ok(V1BackupDisposition::AlreadyCreated)
        }
        Err(source) => {
            if let Err(cleanup_source) = cleanup_result {
                return Err(LifecycleJournalError::Io {
                    operation: "create v1 backup and clean temporary file",
                    path: path.to_path_buf(),
                    source: io::Error::new(
                        source.kind(),
                        format!("{source}; temporary cleanup also failed: {cleanup_source}"),
                    ),
                });
            }
            Err(LifecycleJournalError::Io {
                operation: "atomically create v1 backup",
                path: path.to_path_buf(),
                source,
            })
        }
    }
}

async fn read_optional_bytes(
    path: &Path,
    operation: &'static str,
) -> Result<Option<Vec<u8>>, LifecycleJournalError> {
    match fs::read(path).await {
        Ok(content) => Ok(Some(content)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            match fs::symlink_metadata(path).await {
                Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => Ok(None),
                Ok(_) => Err(LifecycleJournalError::Io {
                    operation,
                    path: path.to_path_buf(),
                    source,
                }),
                Err(metadata_error) => Err(LifecycleJournalError::Io {
                    operation,
                    path: path.to_path_buf(),
                    source: io::Error::new(
                        metadata_error.kind(),
                        format!(
                            "{source}; failed to inspect the unreadable directory entry: {metadata_error}"
                        ),
                    ),
                }),
            }
        }
        Err(source) => Err(LifecycleJournalError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }),
    }
}

async fn read_versioned<T: DeserializeOwned>(
    path: &Path,
    expected: u32,
    entity: &'static str,
) -> Result<Option<T>, LifecycleJournalError> {
    let content = match read_optional_bytes(path, "read lifecycle journal state").await? {
        Some(content) => content,
        None => return Ok(None),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&content).map_err(|source| LifecycleJournalError::InvalidData {
            entity,
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
    let found = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| LifecycleJournalError::InvalidData {
            entity,
            path: path.to_path_buf(),
            reason: "missing unsigned integer `version`".to_owned(),
        })?;
    if found != u64::from(expected) {
        return Err(LifecycleJournalError::UnsupportedVersion {
            entity,
            path: path.to_path_buf(),
            found,
            expected,
        });
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|source| LifecycleJournalError::InvalidData {
            entity,
            path: path.to_path_buf(),
            reason: source.to_string(),
        })
}

async fn write_atomic_bytes(
    path: &Path,
    content: &[u8],
    operation: &'static str,
) -> Result<(), LifecycleJournalError> {
    ensure_parent(path).await?;
    let parent = path
        .parent()
        .ok_or_else(|| LifecycleJournalError::InvalidData {
            entity: "lifecycle journal path",
            path: path.to_path_buf(),
            reason: "path has no parent directory".to_owned(),
        })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("lifecycle-journal");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| LifecycleJournalError::Io {
            operation: "create temporary lifecycle journal state",
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
        return Err(LifecycleJournalError::Io {
            operation: "write and sync temporary lifecycle journal state",
            path: temporary,
            source,
        });
    }
    if let Err(source) = fs::rename(&temporary, path).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(LifecycleJournalError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        });
    }
    sync_parent(path, operation).await
}

async fn ensure_parent(path: &Path) -> Result<(), LifecycleJournalError> {
    let parent = path
        .parent()
        .ok_or_else(|| LifecycleJournalError::InvalidData {
            entity: "lifecycle journal path",
            path: path.to_path_buf(),
            reason: "path has no parent directory".to_owned(),
        })?;
    let mut missing = Vec::new();
    let mut existing_ancestor = parent;
    loop {
        match fs::try_exists(existing_ancestor).await {
            Ok(true) => break,
            Ok(false) => missing.push(existing_ancestor.to_path_buf()),
            Err(source) => {
                return Err(LifecycleJournalError::Io {
                    operation: "inspect lifecycle journal directory",
                    path: existing_ancestor.to_path_buf(),
                    source,
                });
            }
        }
        existing_ancestor =
            existing_ancestor
                .parent()
                .ok_or_else(|| LifecycleJournalError::InvalidData {
                    entity: "lifecycle journal path",
                    path: path.to_path_buf(),
                    reason: "could not find an existing directory ancestor".to_owned(),
                })?;
    }
    fs::create_dir_all(parent)
        .await
        .map_err(|source| LifecycleJournalError::Io {
            operation: "create lifecycle journal directory",
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

async fn sync_directory_before_publication(path: &Path) -> Result<(), LifecycleJournalError> {
    let directory = fs::File::open(path)
        .await
        .map_err(|source| LifecycleJournalError::Io {
            operation: "open newly created lifecycle journal directory for sync",
            path: path.to_path_buf(),
            source,
        })?;
    directory
        .sync_all()
        .await
        .map_err(|source| LifecycleJournalError::Io {
            operation: "sync newly created lifecycle journal directory",
            path: path.to_path_buf(),
            source,
        })
}

async fn sync_parent(path: &Path, operation: &'static str) -> Result<(), LifecycleJournalError> {
    let parent = path
        .parent()
        .ok_or_else(|| LifecycleJournalError::InvalidData {
            entity: "lifecycle journal path",
            path: path.to_path_buf(),
            reason: "path has no parent directory".to_owned(),
        })?;
    let directory = fs::File::open(parent).await.map_err(|source| {
        LifecycleJournalError::PublishedNotDurable {
            operation,
            path: path.to_path_buf(),
            reason: source.to_string(),
        }
    })?;
    directory
        .sync_all()
        .await
        .map_err(|source| LifecycleJournalError::PublishedNotDurable {
            operation,
            path: path.to_path_buf(),
            reason: source.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::{
        AttachmentTransactionImages, CatalogTransactionImages, JournalAdvanceDisposition,
        JournalClearDisposition, JournalCreateDisposition, LegacyV1File, LifecycleJournal,
        LifecycleJournalError, LifecycleRecoveryPreflight, LifecycleTransaction,
        LifecycleTransactionKind, LifecycleTransactionPhase, MigrationMarkerDisposition,
        V1BackupDisposition, finish_create_once_publication,
    };
    use locald_core::attachments::{Attachment, AttachmentSource, AttachmentStoreSnapshot};
    use locald_core::catalog::CATALOG_VERSION;
    use locald_core::{
        AvailabilityBatch, AvailabilityBatchOperation, AvailabilityStore,
        PreparedAvailabilityBatch, ProjectCatalog, ProjectInstanceId,
    };
    use std::str::FromStr;
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::TempDir;
    use uuid::Uuid;

    struct Fixture {
        directory: TempDir,
        journal: LifecycleJournal,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("create lifecycle journal fixture");
            let journal = LifecycleJournal::at(directory.path());
            Self { directory, journal }
        }

        fn transaction(&self) -> LifecycleTransaction {
            let catalog_path = self.directory.path().join("catalog.json");
            let catalog = ProjectCatalog::with_path(catalog_path);
            let catalog = CatalogTransactionImages::new(catalog.clone(), catalog)
                .expect("prepare catalog images");
            LifecycleTransaction::new(
                LifecycleTransactionKind::LifecycleMutation,
                UNIX_EPOCH + Duration::from_secs(42),
                Some(catalog),
                Vec::new(),
                AttachmentTransactionImages::new(
                    AttachmentStoreSnapshot::default(),
                    AttachmentStoreSnapshot::default(),
                ),
            )
            .expect("prepare lifecycle transaction")
        }

        async fn prepared_availability(
            &self,
            effective_at: std::time::SystemTime,
        ) -> PreparedAvailabilityBatch {
            let instance_id = ProjectInstanceId::from_str("00000000-0000-0000-0000-000000000123")
                .expect("parse fixture project instance ID");
            let mut availability = AvailabilityStore::load(self.directory.path(), instance_id)
                .await
                .expect("load fixture availability");
            availability
                .prepare_batch(
                    &AvailabilityBatch::new(effective_at)
                        .with_operation(AvailabilityBatchOperation::Initialize),
                )
                .await
                .expect("prepare fixture availability")
        }
    }

    #[test]
    fn lifecycle_phase_target_requirements_match_completed_publications() {
        let cases = [
            (LifecycleTransactionPhase::Prepared, false, false, false),
            (
                LifecycleTransactionPhase::CatalogPublished,
                true,
                false,
                false,
            ),
            (
                LifecycleTransactionPhase::AvailabilityPublished,
                true,
                true,
                false,
            ),
            (
                LifecycleTransactionPhase::CompatibilityPublished,
                true,
                true,
                true,
            ),
            (LifecycleTransactionPhase::Complete, true, true, true),
        ];

        for (phase, catalog, availability, compatibility) in cases {
            assert_eq!(phase.requires_catalog_target(), catalog);
            assert_eq!(phase.requires_availability_targets(), availability);
            assert_eq!(phase.requires_compatibility_target(), compatibility);
        }
    }

    #[tokio::test]
    async fn create_reload_and_advance_are_replay_idempotent() {
        let fixture = Fixture::new();
        let transaction = fixture.transaction();

        assert_eq!(
            fixture
                .journal
                .create(&transaction)
                .await
                .expect("create transaction"),
            JournalCreateDisposition::Created
        );
        assert_eq!(
            fixture
                .journal
                .create(&transaction)
                .await
                .expect("replay transaction creation"),
            JournalCreateDisposition::AlreadyCreated
        );

        let loaded = fixture
            .journal
            .load()
            .await
            .expect("load transaction")
            .expect("transaction exists");
        assert_eq!(loaded.id(), transaction.id());
        assert_eq!(loaded.phase(), LifecycleTransactionPhase::Prepared);
        assert_eq!(
            loaded.catalog().expect("catalog images").storage_path(),
            fixture.directory.path().join("catalog.json")
        );

        assert_eq!(
            fixture
                .journal
                .advance(
                    transaction.id(),
                    LifecycleTransactionPhase::Prepared,
                    LifecycleTransactionPhase::CatalogPublished,
                )
                .await
                .expect("advance transaction"),
            JournalAdvanceDisposition::Advanced
        );
        assert_eq!(
            fixture
                .journal
                .advance(
                    transaction.id(),
                    LifecycleTransactionPhase::Prepared,
                    LifecycleTransactionPhase::CatalogPublished,
                )
                .await
                .expect("replay phase advancement"),
            JournalAdvanceDisposition::AlreadyAdvanced
        );
    }

    #[tokio::test]
    async fn a_different_pending_transaction_is_rejected() {
        let fixture = Fixture::new();
        let first = fixture.transaction();
        let second = fixture.transaction();
        fixture
            .journal
            .create(&first)
            .await
            .expect("create first transaction");

        let error = fixture
            .journal
            .create(&second)
            .await
            .expect_err("different transaction must not replace the journal");
        assert!(matches!(
            error,
            LifecycleJournalError::ActiveTransaction { existing, .. }
                if existing == first.id()
        ));
    }

    #[tokio::test]
    async fn duplicate_availability_owner_is_rejected_before_journaling() {
        let fixture = Fixture::new();
        let effective_at = UNIX_EPOCH + Duration::from_secs(42);
        let prepared = fixture.prepared_availability(effective_at).await;
        let error = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            effective_at,
            None,
            vec![prepared.clone(), prepared],
            AttachmentTransactionImages::new(
                AttachmentStoreSnapshot::default(),
                AttachmentStoreSnapshot::default(),
            ),
        )
        .expect_err("one transaction cannot publish one availability owner twice");
        assert!(matches!(error, LifecycleJournalError::InvalidPlan { .. }));
    }

    #[tokio::test]
    async fn inconsistent_prepared_availability_is_rejected_before_phase_application() {
        let fixture = Fixture::new();
        let effective_at = UNIX_EPOCH + Duration::from_secs(42);
        let prepared = fixture.prepared_availability(effective_at).await;
        let transaction = LifecycleTransaction::new(
            LifecycleTransactionKind::LifecycleMutation,
            effective_at,
            None,
            vec![prepared],
            AttachmentTransactionImages::new(
                AttachmentStoreSnapshot::default(),
                AttachmentStoreSnapshot::default(),
            ),
        )
        .expect("prepare valid transaction");
        let mut value = serde_json::to_value(transaction).expect("serialize transaction");
        let prepared = value["availability"]
            .as_array_mut()
            .and_then(|availability| availability.first_mut())
            .expect("serialized prepared availability");
        let expected = prepared["expected"].clone();
        prepared["target"] = expected;
        let malformed: LifecycleTransaction =
            serde_json::from_value(value).expect("deserialize malformed transaction shape");

        assert!(matches!(
            malformed
                .validate()
                .expect_err("inconsistent prepared target must be rejected"),
            LifecycleJournalError::InvalidPlan { .. }
        ));
    }

    #[tokio::test]
    async fn legacy_migration_requires_exact_catalog_availability_owners() {
        let fixture = Fixture::new();
        let effective_at = UNIX_EPOCH + Duration::from_secs(42);
        let empty_attachments = || {
            AttachmentTransactionImages::new(
                AttachmentStoreSnapshot::default(),
                AttachmentStoreSnapshot::default(),
            )
        };
        assert!(matches!(
            LifecycleTransaction::new(
                LifecycleTransactionKind::LegacyV1Migration,
                effective_at,
                None,
                Vec::new(),
                empty_attachments(),
            )
            .expect_err("migration must retain exact catalog images"),
            LifecycleJournalError::InvalidPlan { .. }
        ));

        let catalog_path = fixture.directory.path().join("catalog.json");
        let catalog = ProjectCatalog::with_path(catalog_path);
        let catalog = CatalogTransactionImages::new(catalog.clone(), catalog)
            .expect("prepare empty catalog images");
        let unexpected = fixture.prepared_availability(effective_at).await;
        assert!(matches!(
            LifecycleTransaction::new(
                LifecycleTransactionKind::LegacyV1Migration,
                effective_at,
                Some(catalog),
                vec![unexpected],
                empty_attachments(),
            )
            .expect_err("migration cannot contain an availability owner outside the catalog"),
            LifecycleJournalError::InvalidPlan { .. }
        ));
    }

    #[test]
    fn exact_legacy_attachment_base_can_be_normalized_in_the_target() {
        let fixture = Fixture::new();
        let key = fixture.directory.path().join("canonical-project");
        let catalog = ProjectCatalog::with_path(fixture.directory.path().join("catalog.json"));
        let catalog = CatalogTransactionImages::new(catalog.clone(), catalog)
            .expect("prepare empty catalog images");
        let mut legacy_base = AttachmentStoreSnapshot::default();
        legacy_base.attachments.insert(
            key,
            vec![Attachment {
                project_path: fixture.directory.path().join("embedded-legacy-project"),
                source: AttachmentSource::Runtime,
                created_at: UNIX_EPOCH,
            }],
        );

        LifecycleTransaction::new(
            LifecycleTransactionKind::LegacyV1Migration,
            UNIX_EPOCH,
            Some(catalog),
            Vec::new(),
            AttachmentTransactionImages::new(legacy_base, AttachmentStoreSnapshot::default()),
        )
        .expect("exact legacy base must remain journalable");
    }

    #[test]
    fn empty_attachment_target_is_rejected_before_journaling() {
        let fixture = Fixture::new();
        let mut target = AttachmentStoreSnapshot::default();
        target
            .attachments
            .insert(fixture.directory.path().join("empty-project"), Vec::new());

        assert!(matches!(
            LifecycleTransaction::new(
                LifecycleTransactionKind::LifecycleMutation,
                UNIX_EPOCH,
                None,
                Vec::new(),
                AttachmentTransactionImages::new(AttachmentStoreSnapshot::default(), target),
            )
            .expect_err("empty exact attachment target must fail closed"),
            LifecycleJournalError::InvalidPlan { .. }
        ));
    }

    #[tokio::test]
    async fn malformed_and_unsupported_journals_block_loading() {
        let malformed = Fixture::new();
        tokio::fs::write(malformed.journal.journal_path(), b"{")
            .await
            .expect("write malformed journal");
        assert!(matches!(
            malformed
                .journal
                .load()
                .await
                .expect_err("malformed journal must block"),
            LifecycleJournalError::InvalidData { .. }
        ));

        let unsupported = Fixture::new();
        tokio::fs::write(unsupported.journal.journal_path(), br#"{"version": 999}"#)
            .await
            .expect("write unsupported journal");
        assert!(matches!(
            unsupported
                .journal
                .load()
                .await
                .expect_err("unsupported journal must block"),
            LifecycleJournalError::UnsupportedVersion { found: 999, .. }
        ));
    }

    #[tokio::test]
    async fn prepared_journal_upgrades_embedded_v3_catalog_images() {
        let fixture = Fixture::new();
        let transaction = fixture.transaction();
        let mut value = serde_json::to_value(&transaction).expect("serialize lifecycle journal");
        for image in ["base", "target"] {
            let catalog = value
                .get_mut("catalog")
                .and_then(serde_json::Value::as_object_mut)
                .and_then(|catalog| catalog.get_mut(image))
                .and_then(serde_json::Value::as_object_mut)
                .expect("embedded catalog image");
            catalog.insert(
                "version".to_owned(),
                serde_json::Value::from(CATALOG_VERSION - 1),
            );
            catalog.remove("agent_bindings");
        }
        tokio::fs::write(
            fixture.journal.journal_path(),
            serde_json::to_vec_pretty(&value).expect("encode v3 lifecycle journal"),
        )
        .await
        .expect("write v3 lifecycle journal");

        let loaded = fixture
            .journal
            .load()
            .await
            .expect("load upgraded lifecycle journal")
            .expect("journal remains present");
        let loaded = serde_json::to_value(loaded).expect("serialize upgraded journal");
        for image in ["base", "target"] {
            let catalog = &loaded["catalog"][image];
            assert_eq!(catalog["version"], serde_json::Value::from(CATALOG_VERSION));
            assert_eq!(catalog["agent_bindings"], serde_json::json!({}));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_journal_authority_entries_block_without_replacement() {
        let journal_fixture = Fixture::new();
        std::os::unix::fs::symlink(
            journal_fixture.directory.path().join("missing-journal"),
            journal_fixture.journal.journal_path(),
        )
        .expect("create dangling transaction journal");
        assert!(matches!(
            journal_fixture
                .journal
                .preflight()
                .await
                .expect_err("dangling transaction journal must block preflight"),
            LifecycleJournalError::Io { .. }
        ));
        assert!(
            tokio::fs::symlink_metadata(journal_fixture.journal.journal_path())
                .await
                .expect("inspect preserved transaction journal")
                .file_type()
                .is_symlink()
        );

        let marker_fixture = Fixture::new();
        std::os::unix::fs::symlink(
            marker_fixture.directory.path().join("missing-marker"),
            &marker_fixture.journal.migration_marker_path,
        )
        .expect("create dangling migration marker");
        assert!(matches!(
            marker_fixture
                .journal
                .preflight()
                .await
                .expect_err("dangling migration marker must block preflight"),
            LifecycleJournalError::Io { .. }
        ));
        assert!(
            tokio::fs::symlink_metadata(&marker_fixture.journal.migration_marker_path)
                .await
                .expect("inspect preserved migration marker")
                .file_type()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn recovery_preflight_validates_marker_before_advancing_an_active_journal() {
        let fixture = Fixture::new();
        let transaction = fixture.transaction();
        fixture
            .journal
            .create(&transaction)
            .await
            .expect("create active transaction");
        let journal_before = tokio::fs::read(fixture.journal.journal_path())
            .await
            .expect("read active journal");
        tokio::fs::write(&fixture.journal.migration_marker_path, b"{")
            .await
            .expect("write malformed marker");

        assert!(matches!(
            fixture
                .journal
                .preflight()
                .await
                .expect_err("malformed marker must block preflight"),
            LifecycleJournalError::InvalidData { .. }
        ));
        assert_eq!(
            tokio::fs::read(fixture.journal.journal_path())
                .await
                .expect("reread preserved journal"),
            journal_before
        );
    }

    #[test]
    fn recovery_preflight_selects_strict_attachment_authority_at_the_commit_boundary() {
        let fixture = Fixture::new();
        let mut legacy = fixture.transaction();
        legacy.kind = LifecycleTransactionKind::LegacyV1Migration;
        let prepublication = LifecycleRecoveryPreflight {
            transaction: Some(legacy.clone()),
            marker: None,
        };
        assert!(!prepublication.has_v2_authority());
        assert!(!prepublication.requires_exact_attachment_authority());

        legacy.phase = LifecycleTransactionPhase::CatalogPublished;
        let catalog_published = LifecycleRecoveryPreflight {
            transaction: Some(legacy.clone()),
            marker: None,
        };
        assert!(catalog_published.has_v2_authority());

        legacy.phase = LifecycleTransactionPhase::CompatibilityPublished;
        let published = LifecycleRecoveryPreflight {
            transaction: Some(legacy),
            marker: None,
        };
        assert!(published.requires_exact_attachment_authority());

        let mutation = LifecycleRecoveryPreflight {
            transaction: Some(fixture.transaction()),
            marker: None,
        };
        assert!(mutation.requires_exact_attachment_authority());
    }

    #[tokio::test]
    async fn lifecycle_mutation_requires_the_completed_migration_marker() {
        let fixture = Fixture::new();
        let transaction = fixture.transaction();
        fixture
            .journal
            .create(&transaction)
            .await
            .expect("create active lifecycle mutation");

        assert!(matches!(
            fixture
                .journal
                .preflight()
                .await
                .expect_err("mutation without migration marker must fail closed"),
            LifecycleJournalError::InvalidRecoveryState { .. }
        ));
    }

    #[tokio::test]
    async fn migration_completion_marker_is_durable_and_create_once() {
        let fixture = Fixture::new();
        let transaction_id = Uuid::new_v4();
        let completed_at = UNIX_EPOCH + Duration::from_secs(99);

        assert_eq!(
            fixture
                .journal
                .mark_migration_complete(transaction_id, completed_at)
                .await
                .expect("publish marker"),
            MigrationMarkerDisposition::Created
        );
        assert_eq!(
            fixture
                .journal
                .mark_migration_complete(transaction_id, completed_at)
                .await
                .expect("replay marker publication"),
            MigrationMarkerDisposition::AlreadyCreated
        );
        let marker = fixture
            .journal
            .migration_marker()
            .await
            .expect("load marker")
            .expect("marker exists");
        assert_eq!(marker.transaction_id(), transaction_id);
        assert_eq!(marker.completed_at(), completed_at);

        assert!(matches!(
            fixture
                .journal
                .mark_migration_complete(Uuid::new_v4(), completed_at)
                .await
                .expect_err("another migration cannot replace the marker"),
            LifecycleJournalError::MigrationMarkerConflict { existing }
                if existing == transaction_id
        ));
    }

    #[tokio::test]
    async fn raw_v1_backups_are_atomic_and_create_once() {
        let fixture = Fixture::new();
        let source = fixture.directory.path().join("registry.json");
        let original = b"{\"projects\":[\"first\"]}";
        tokio::fs::write(&source, original)
            .await
            .expect("write legacy state");

        assert_eq!(
            fixture
                .journal
                .backup_v1_file(LegacyV1File::Registry, &source)
                .await
                .expect("create raw backup"),
            V1BackupDisposition::Created
        );
        assert_eq!(
            fixture
                .journal
                .backup_v1_file(LegacyV1File::Registry, &source)
                .await
                .expect("replay raw backup"),
            V1BackupDisposition::AlreadyCreated
        );

        tokio::fs::write(&source, b"changed")
            .await
            .expect("change legacy source");
        assert!(matches!(
            fixture
                .journal
                .backup_v1_file(LegacyV1File::Registry, &source)
                .await
                .expect_err("changed source cannot replace backup"),
            LifecycleJournalError::BackupConflict { .. }
        ));
        assert_eq!(
            tokio::fs::read(fixture.journal.backup_directory.join("registry.json"))
                .await
                .expect("read preserved backup"),
            original
        );
    }

    #[tokio::test]
    async fn successful_backup_publication_ignores_temporary_cleanup_failure() {
        let fixture = Fixture::new();
        let backup = fixture.directory.path().join("backup.json");
        let temporary = fixture.directory.path().join("temporary-directory");
        let content = b"published";
        tokio::fs::write(&backup, content)
            .await
            .expect("represent the successful hard-link publication");
        tokio::fs::create_dir(&temporary)
            .await
            .expect("make remove_file fail for the temporary path");

        assert_eq!(
            finish_create_once_publication(&backup, content, &temporary, Ok(()))
                .await
                .expect("published backup remains successful"),
            V1BackupDisposition::Created
        );
        assert_eq!(
            tokio::fs::read(&backup)
                .await
                .expect("read published backup"),
            content
        );
        assert!(temporary.is_dir());
    }

    #[tokio::test]
    async fn matching_concurrent_backup_ignores_temporary_cleanup_failure() {
        let fixture = Fixture::new();
        let backup = fixture.directory.path().join("backup.json");
        let temporary = fixture.directory.path().join("temporary-directory");
        let content = b"published";
        tokio::fs::write(&backup, content)
            .await
            .expect("represent the concurrent create-once winner");
        tokio::fs::create_dir(&temporary)
            .await
            .expect("make remove_file fail for the temporary path");

        assert_eq!(
            finish_create_once_publication(
                &backup,
                content,
                &temporary,
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "simulated concurrent publication",
                )),
            )
            .await
            .expect("matching concurrent backup remains idempotent"),
            V1BackupDisposition::AlreadyCreated
        );
        assert_eq!(
            tokio::fs::read(&backup)
                .await
                .expect("read existing backup"),
            content
        );
        assert!(temporary.is_dir());
    }

    #[tokio::test]
    async fn conflicting_concurrent_backup_still_fails_when_temporary_cleanup_fails() {
        let fixture = Fixture::new();
        let backup = fixture.directory.path().join("backup.json");
        let temporary = fixture.directory.path().join("temporary-directory");
        tokio::fs::write(&backup, b"other")
            .await
            .expect("represent a conflicting create-once winner");
        tokio::fs::create_dir(&temporary)
            .await
            .expect("make remove_file fail for the temporary path");

        assert!(matches!(
            finish_create_once_publication(
                &backup,
                b"expected",
                &temporary,
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "simulated concurrent publication",
                )),
            )
            .await
            .expect_err("conflicting backup must remain fatal"),
            LifecycleJournalError::BackupConflict { path } if path == backup
        ));
        assert!(temporary.is_dir());
    }

    #[tokio::test]
    async fn existing_v1_backup_requires_its_source_on_replay() {
        let fixture = Fixture::new();
        let source = fixture.directory.path().join("registry.json");
        let original = b"{\"projects\":[\"first\"]}";
        tokio::fs::write(&source, original)
            .await
            .expect("write legacy state");
        fixture
            .journal
            .backup_v1_file(LegacyV1File::Registry, &source)
            .await
            .expect("create raw backup");
        tokio::fs::remove_file(&source)
            .await
            .expect("remove legacy source after backup");

        let error = fixture
            .journal
            .backup_v1_file(LegacyV1File::Registry, &source)
            .await
            .expect_err("missing source cannot bypass its exact backup");
        assert!(matches!(
            error,
            LifecycleJournalError::BackupSourceMissing {
                source_path,
                backup_path,
            } if source_path == source
                && backup_path == fixture.journal.backup_directory.join("registry.json")
        ));
        assert_eq!(
            tokio::fs::read(fixture.journal.backup_directory.join("registry.json"))
                .await
                .expect("read preserved backup"),
            original
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dangling_v1_backup_entries_block_without_replacement() {
        let source_fixture = Fixture::new();
        let source = source_fixture.directory.path().join("registry.json");
        std::os::unix::fs::symlink(
            source_fixture.directory.path().join("missing-registry"),
            &source,
        )
        .expect("create dangling legacy source");
        assert!(matches!(
            source_fixture
                .journal
                .backup_v1_file(LegacyV1File::Registry, &source)
                .await
                .expect_err("dangling legacy source must block backup"),
            LifecycleJournalError::Io { .. }
        ));
        assert!(
            tokio::fs::symlink_metadata(&source)
                .await
                .expect("inspect preserved legacy source")
                .file_type()
                .is_symlink()
        );

        let backup_fixture = Fixture::new();
        tokio::fs::create_dir_all(&backup_fixture.journal.backup_directory)
            .await
            .expect("create backup directory");
        let backup = backup_fixture
            .journal
            .backup_directory
            .join("registry.json");
        std::os::unix::fs::symlink(
            backup_fixture.directory.path().join("missing-backup"),
            &backup,
        )
        .expect("create dangling legacy backup");
        let missing_source = backup_fixture.directory.path().join("absent-registry.json");
        assert!(matches!(
            backup_fixture
                .journal
                .backup_v1_file(LegacyV1File::Registry, &missing_source)
                .await
                .expect_err("dangling existing backup must block missing-source replay"),
            LifecycleJournalError::Io { .. }
        ));
        assert!(
            tokio::fs::symlink_metadata(&backup)
                .await
                .expect("inspect preserved legacy backup")
                .file_type()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn only_complete_transactions_can_be_cleared() {
        let fixture = Fixture::new();
        let transaction = fixture.transaction();
        fixture
            .journal
            .create(&transaction)
            .await
            .expect("create transaction");
        assert!(matches!(
            fixture
                .journal
                .clear(transaction.id())
                .await
                .expect_err("prepared transaction must remain replayable"),
            LifecycleJournalError::IncompleteTransaction { .. }
        ));

        let phases = [
            (
                LifecycleTransactionPhase::Prepared,
                LifecycleTransactionPhase::CatalogPublished,
            ),
            (
                LifecycleTransactionPhase::CatalogPublished,
                LifecycleTransactionPhase::AvailabilityPublished,
            ),
            (
                LifecycleTransactionPhase::AvailabilityPublished,
                LifecycleTransactionPhase::CompatibilityPublished,
            ),
            (
                LifecycleTransactionPhase::CompatibilityPublished,
                LifecycleTransactionPhase::Complete,
            ),
        ];
        for (expected, next) in phases {
            fixture
                .journal
                .advance(transaction.id(), expected, next)
                .await
                .expect("advance transaction toward completion");
        }

        assert_eq!(
            fixture
                .journal
                .clear(transaction.id())
                .await
                .expect("clear completed transaction"),
            JournalClearDisposition::Cleared
        );
        assert_eq!(
            fixture
                .journal
                .clear(transaction.id())
                .await
                .expect("replay completed clear"),
            JournalClearDisposition::AlreadyClear
        );
        assert!(
            fixture
                .journal
                .load()
                .await
                .expect("confirm clear")
                .is_none()
        );
    }
}
