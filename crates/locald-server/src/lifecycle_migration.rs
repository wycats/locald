//! Pure planning for the one-time legacy lifecycle-state migration.
//!
//! This module deliberately does not read or write daemon state. Callers
//! provide deterministic attachment evidence and a fixed wall-clock time; the
//! result can then be prepared and committed by the lifecycle transaction
//! journal without re-evaluating liveness or lease deadlines during replay.

#![allow(clippy::redundant_pub_crate)] // Explicitly mark the crate-internal planning surface.

use locald_core::attachments::{Attachment, AttachmentCompatibilityEvidence, AttachmentSource};
use locald_core::{AvailabilityBatch, AvailabilityBatchOperation, DemandKey, DemandKeyError};
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// The complete lifecycle migration target for one catalogued project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectLifecycleMigrationPlan {
    /// Ordered availability operations evaluated at the migration timestamp.
    pub(crate) availability_batch: AvailabilityBatch,
    /// Live legacy owners retained for one beta-cycle compatibility projection.
    pub(crate) compatibility_attachments: Vec<Attachment>,
    /// The legacy manual-stop mirror retained alongside the authoritative pause.
    pub(crate) compatibility_manually_stopped: bool,
    /// Authoritative Always On policy imported from either legacy mirror.
    pub(crate) always_on: bool,
}

/// Plan the authoritative and compatibility targets for one catalog instance.
///
/// Every catalogued instance receives an initialized availability record. A
/// catalog pin or valid legacy `Pin` enables Always On; parked `Runtime`
/// evidence becomes the singleton manual demand; live editor and CLI owners
/// become privacy-preserving demands. The manual-stop pause is deliberately
/// appended last so it suppresses every imported availability reason.
pub(crate) fn plan_project_lifecycle_migration(
    catalog_pinned: bool,
    evidence: &AttachmentCompatibilityEvidence,
    effective_at: SystemTime,
) -> Result<ProjectLifecycleMigrationPlan, DemandKeyError> {
    let mut always_on = catalog_pinned;
    let mut runtime_acquired_at = None;
    let mut demands = BTreeSet::new();
    let mut compatibility = BTreeMap::<CompatibilityOwner, Attachment>::new();

    for item in &evidence.attachments {
        match &item.attachment.source {
            AttachmentSource::Runtime if item.alive => {
                runtime_acquired_at = Some(
                    runtime_acquired_at
                        .map_or(item.attachment.created_at, |current: SystemTime| {
                            current.max(item.attachment.created_at)
                        }),
                );
            }
            AttachmentSource::Pin if item.alive => {
                always_on = true;
                retain_preferred_attachment(
                    &mut compatibility,
                    CompatibilityOwner::Pin,
                    &evidence.project_path,
                    &item.attachment,
                );
            }
            AttachmentSource::CLI { pid } if item.alive => {
                if let Some(demand) =
                    availability_demand_for_attachment_source(&item.attachment.source)?
                {
                    demands.insert(demand);
                }
                retain_preferred_attachment(
                    &mut compatibility,
                    CompatibilityOwner::Cli { pid: *pid },
                    &evidence.project_path,
                    &item.attachment,
                );
            }
            AttachmentSource::Editor { name, id, .. } if item.alive => {
                if let Some(demand) =
                    availability_demand_for_attachment_source(&item.attachment.source)?
                {
                    demands.insert(demand);
                }
                retain_preferred_attachment(
                    &mut compatibility,
                    CompatibilityOwner::Editor {
                        name: name.clone(),
                        id: id.clone(),
                    },
                    &evidence.project_path,
                    &item.attachment,
                );
            }
            AttachmentSource::Editor { .. }
            | AttachmentSource::CLI { .. }
            | AttachmentSource::Runtime
            | AttachmentSource::Pin => {}
        }
    }

    let mut availability_batch =
        AvailabilityBatch::new(effective_at).with_operation(AvailabilityBatchOperation::Initialize);
    if always_on {
        availability_batch.push(AvailabilityBatchOperation::SetAlwaysOn(true));
    }
    if let Some(acquired_at) = runtime_acquired_at {
        availability_batch.push(AvailabilityBatchOperation::ImportDemand {
            key: DemandKey::manual_cli(),
            acquired_at,
        });
    }
    for demand in demands {
        availability_batch.push(AvailabilityBatchOperation::EnsureDemand(demand));
    }
    if evidence.manually_stopped {
        availability_batch.push(AvailabilityBatchOperation::PauseProject);
    }

    Ok(ProjectLifecycleMigrationPlan {
        availability_batch,
        compatibility_attachments: compatibility.into_values().collect(),
        compatibility_manually_stopped: evidence.manually_stopped,
        always_on,
    })
}

/// Map one live legacy attachment owner to its availability demand.
///
/// The same mapping is used during one-time import and by the compatibility
/// IPC adapters so durable owner identity cannot drift between those paths.
pub(crate) fn availability_demand_for_attachment_source(
    source: &AttachmentSource,
) -> Result<Option<DemandKey>, DemandKeyError> {
    match source {
        AttachmentSource::Editor { name, id, .. } => {
            DemandKey::vs_code_window(&editor_private_identity(name, id)).map(Some)
        }
        AttachmentSource::CLI { pid } => {
            DemandKey::legacy_process_attachment(&format!("legacy-cli-pid:{pid}")).map(Some)
        }
        AttachmentSource::Runtime | AttachmentSource::Pin => Ok(None),
    }
}

/// A stable compatibility owner key independent of file order and mutable PID
/// metadata on editor attachments.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CompatibilityOwner {
    Pin,
    Editor { name: String, id: String },
    Cli { pid: u32 },
}

fn editor_private_identity(name: &str, id: &str) -> String {
    // Length prefixes keep distinct pairs distinct without storing this private
    // source value in availability state: DemandKey hashes it immediately.
    format!("legacy-editor:{}:{name}:{}:{id}", name.len(), id.len())
}

fn retain_preferred_attachment(
    compatibility: &mut BTreeMap<CompatibilityOwner, Attachment>,
    owner: CompatibilityOwner,
    project_path: &std::path::Path,
    candidate: &Attachment,
) {
    let mut candidate = candidate.clone();
    candidate.project_path = project_path.to_path_buf();
    match compatibility.entry(owner) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(candidate);
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            if attachment_preference(&candidate) > attachment_preference(entry.get()) {
                entry.insert(candidate);
            }
        }
    }
}

fn attachment_preference(attachment: &Attachment) -> (SystemTime, Option<u32>) {
    let pid = match &attachment.source {
        AttachmentSource::Editor { pid, .. } => *pid,
        AttachmentSource::CLI { pid } => Some(*pid),
        AttachmentSource::Runtime | AttachmentSource::Pin => None,
    };
    (attachment.created_at, pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use locald_core::DemandKind;
    use locald_core::attachments::AttachmentLivenessEvidence;
    use std::path::PathBuf;
    use std::time::Duration;

    fn time(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn attachment(source: AttachmentSource, created_at: u64) -> Attachment {
        Attachment {
            project_path: PathBuf::from("/projects/example"),
            source,
            created_at: time(created_at),
        }
    }

    fn item(source: AttachmentSource, created_at: u64, alive: bool) -> AttachmentLivenessEvidence {
        AttachmentLivenessEvidence {
            attachment: attachment(source, created_at),
            alive,
        }
    }

    fn evidence(
        attachments: Vec<AttachmentLivenessEvidence>,
        manually_stopped: bool,
    ) -> AttachmentCompatibilityEvidence {
        AttachmentCompatibilityEvidence {
            project_path: PathBuf::from("/projects/example"),
            attachments,
            manually_stopped,
        }
    }

    #[test]
    fn maps_all_legacy_sources_and_applies_pause_last() {
        let evidence = evidence(
            vec![
                item(AttachmentSource::CLI { pid: 9 }, 1, false),
                item(AttachmentSource::Runtime, 2, true),
                item(
                    AttachmentSource::Editor {
                        name: "Code".to_owned(),
                        id: "window-a".to_owned(),
                        pid: Some(20),
                    },
                    3,
                    true,
                ),
                item(AttachmentSource::CLI { pid: 41 }, 4, true),
                item(AttachmentSource::Pin, 5, true),
            ],
            true,
        );

        let plan = plan_project_lifecycle_migration(false, &evidence, time(100))
            .expect("migration plan should be valid");
        let editor = DemandKey::vs_code_window(&editor_private_identity("Code", "window-a"))
            .expect("editor key should be valid");
        let cli = DemandKey::legacy_process_attachment("legacy-cli-pid:41")
            .expect("CLI key should be valid");

        assert_eq!(plan.availability_batch.effective_at(), time(100));
        assert_eq!(
            plan.availability_batch.operations(),
            &[
                AvailabilityBatchOperation::Initialize,
                AvailabilityBatchOperation::SetAlwaysOn(true),
                AvailabilityBatchOperation::ImportDemand {
                    key: DemandKey::manual_cli(),
                    acquired_at: time(2),
                },
                AvailabilityBatchOperation::EnsureDemand(editor),
                AvailabilityBatchOperation::EnsureDemand(cli),
                AvailabilityBatchOperation::PauseProject,
            ]
        );
        assert!(plan.compatibility_manually_stopped);
        assert_eq!(
            plan.compatibility_attachments
                .iter()
                .map(|attachment| &attachment.source)
                .collect::<Vec<_>>(),
            vec![
                &AttachmentSource::Pin,
                &AttachmentSource::Editor {
                    name: "Code".to_owned(),
                    id: "window-a".to_owned(),
                    pid: Some(20),
                },
                &AttachmentSource::CLI { pid: 41 },
            ]
        );
    }

    #[test]
    fn catalog_pin_initializes_always_on_without_legacy_attachments() {
        let plan = plan_project_lifecycle_migration(true, &evidence(Vec::new(), false), time(10))
            .expect("migration plan should be valid");

        assert_eq!(
            plan.availability_batch.operations(),
            &[
                AvailabilityBatchOperation::Initialize,
                AvailabilityBatchOperation::SetAlwaysOn(true),
            ]
        );
        assert!(plan.compatibility_attachments.is_empty());
        assert!(!plan.compatibility_manually_stopped);
    }

    #[test]
    fn dead_owners_and_runtime_are_removed_from_compatibility_projection() {
        let plan = plan_project_lifecycle_migration(
            false,
            &evidence(
                vec![
                    item(AttachmentSource::Runtime, 1, true),
                    item(AttachmentSource::CLI { pid: 99 }, 2, false),
                    item(
                        AttachmentSource::Editor {
                            name: "Code".to_owned(),
                            id: "dead-window".to_owned(),
                            pid: None,
                        },
                        3,
                        false,
                    ),
                    item(AttachmentSource::Pin, 4, false),
                ],
                false,
            ),
            time(10),
        )
        .expect("migration plan should be valid");

        assert_eq!(
            plan.availability_batch.operations(),
            &[
                AvailabilityBatchOperation::Initialize,
                AvailabilityBatchOperation::ImportDemand {
                    key: DemandKey::manual_cli(),
                    acquired_at: time(1),
                },
            ]
        );
        assert!(plan.compatibility_attachments.is_empty());
    }

    #[test]
    fn planning_is_order_independent_and_deduplicates_semantic_owners() {
        let old_editor = item(
            AttachmentSource::Editor {
                name: "Code".to_owned(),
                id: "one".to_owned(),
                pid: Some(10),
            },
            1,
            true,
        );
        let new_editor = item(
            AttachmentSource::Editor {
                name: "Code".to_owned(),
                id: "one".to_owned(),
                pid: Some(11),
            },
            2,
            true,
        );
        let runtime = item(AttachmentSource::Runtime, 3, true);
        let cli = item(AttachmentSource::CLI { pid: 42 }, 4, true);
        let input = vec![
            old_editor.clone(),
            runtime.clone(),
            cli.clone(),
            new_editor.clone(),
            runtime.clone(),
            cli.clone(),
        ];
        let mut reversed = input.clone();
        reversed.reverse();

        let first = plan_project_lifecycle_migration(false, &evidence(input, false), time(20))
            .expect("migration plan should be valid");
        let second = plan_project_lifecycle_migration(false, &evidence(reversed, false), time(20))
            .expect("migration plan should be valid");

        assert_eq!(first, second);
        assert_eq!(first.compatibility_attachments.len(), 2);
        assert!(matches!(
            first.compatibility_attachments[0].source,
            AttachmentSource::Editor { pid: Some(11), .. }
        ));
        assert_eq!(
            first
                .availability_batch
                .operations()
                .iter()
                .filter(|operation| matches!(
                    operation,
                    AvailabilityBatchOperation::ImportDemand { key, .. }
                        if key.kind() == DemandKind::ManualCli
                ))
                .count(),
            1
        );
    }

    #[test]
    fn tied_duplicate_owner_paths_normalize_to_the_evidence_project() {
        let mut first = item(
            AttachmentSource::Editor {
                name: "Code".to_owned(),
                id: "one".to_owned(),
                pid: Some(10),
            },
            1,
            true,
        );
        first.attachment.project_path = PathBuf::from("/stale/first");
        let mut second = first.clone();
        second.attachment.project_path = PathBuf::from("/stale/second");

        let left = plan_project_lifecycle_migration(
            false,
            &evidence(vec![first.clone(), second.clone()], false),
            time(20),
        )
        .expect("migration plan should be valid");
        let right = plan_project_lifecycle_migration(
            false,
            &evidence(vec![second, first], false),
            time(20),
        )
        .expect("migration plan should be valid");

        assert_eq!(left, right);
        assert_eq!(
            left.compatibility_attachments[0].project_path,
            PathBuf::from("/projects/example")
        );
    }

    #[test]
    fn editor_demand_identity_uses_stable_name_and_id_instead_of_pid() {
        let plan_for_pid = |pid| {
            plan_project_lifecycle_migration(
                false,
                &evidence(
                    vec![item(
                        AttachmentSource::Editor {
                            name: "Code".to_owned(),
                            id: "stable-window".to_owned(),
                            pid: Some(pid),
                        },
                        1,
                        true,
                    )],
                    false,
                ),
                time(20),
            )
            .expect("migration plan should be valid")
        };

        assert_eq!(
            plan_for_pid(100).availability_batch,
            plan_for_pid(200).availability_batch
        );
    }

    #[test]
    fn durable_availability_keys_do_not_disclose_private_owner_values() {
        let plan = plan_project_lifecycle_migration(
            false,
            &evidence(
                vec![
                    item(
                        AttachmentSource::Editor {
                            name: "secret-editor-name".to_owned(),
                            id: "secret-window-id".to_owned(),
                            pid: Some(123_456_789),
                        },
                        1,
                        true,
                    ),
                    item(AttachmentSource::CLI { pid: 987_654_321 }, 2, true),
                ],
                false,
            ),
            time(20),
        )
        .expect("migration plan should be valid");
        let serialized = serde_json::to_string(&plan.availability_batch)
            .expect("availability batch should serialize");

        assert!(!serialized.contains("secret-editor-name"));
        assert!(!serialized.contains("secret-window-id"));
        assert!(!serialized.contains("123456789"));
        assert!(!serialized.contains("987654321"));
    }
}
