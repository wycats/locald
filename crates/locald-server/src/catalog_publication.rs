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
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub(crate) const CATALOG_PUBLICATION_VERSION: u32 = 1;
const JOURNAL_FILE: &str = "catalog-publication.json";

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

    #[must_use]
    pub(crate) const fn target_generation(&self) -> Uuid {
        self.target_generation
    }

    #[must_use]
    pub(crate) const fn phase(&self) -> CatalogPublicationPhase {
        self.phase
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
        let value: serde_json::Value = serde_json::from_slice(&content).map_err(|source| {
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

fn validate_embedded_catalog_agent_bindings(value: &serde_json::Value) -> Result<(), String> {
    for image_name in ["catalog_base", "catalog_target"] {
        let Some(image) = value.get(image_name) else {
            continue;
        };
        if image.get("version").and_then(serde_json::Value::as_u64)
            == Some(u64::from(CATALOG_VERSION))
            && image.get("agent_bindings").is_none()
        {
            return Err(format!(
                "current embedded {image_name} is missing `agent_bindings`"
            ));
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
        CATALOG_PUBLICATION_VERSION, CatalogPublicationError, CatalogPublicationJournal,
        CatalogPublicationPhase, CatalogPublicationTransaction, host_set_for_catalog,
    };
    use locald_core::ProjectCatalog;
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
    async fn malformed_current_catalog_image_is_preserved_and_rejected() {
        let (_temporary, journal, transaction) = fixture();
        let catalog_path = transaction.catalog_path().to_path_buf();
        let mut value = serde_json::to_value(&transaction).expect("serialize transaction");
        value["catalog_base"]
            .as_object_mut()
            .expect("catalog base object")
            .remove("agent_bindings");
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
            .expect_err("reject missing required current field");
        let after = tokio::fs::read(journal.journal_path())
            .await
            .expect("read malformed journal after load");
        assert_eq!(before, after);
        assert!(error.to_string().contains("agent_bindings"));
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
