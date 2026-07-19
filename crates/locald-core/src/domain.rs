//! Exact domain ownership shared by routing, status, hosts, and TLS.

use crate::identity::ProjectInstanceId;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use thiserror::Error;

/// Convert a project display name into a deterministic DNS label for its
/// implicit `.localhost` domain.
///
/// Explicitly configured domains remain strict and are never rewritten.
#[must_use]
pub fn sanitize_project_name_for_dns(project_name: &str) -> String {
    sanitize_dns_label(project_name, "project")
}

/// Convert a service display name into the DNS label used for its generated
/// service subdomain. The original name remains the runtime service identity.
#[must_use]
pub fn sanitize_service_name_for_dns(service_name: &str) -> String {
    sanitize_dns_label(service_name, "service")
}

pub(crate) fn sanitize_dns_label(value: &str, empty_fallback: &str) -> String {
    let mut result = String::with_capacity(value.len());

    for character in value.chars() {
        let normalized = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else {
            '-'
        };

        if normalized == '-' && (result.is_empty() || result.ends_with('-')) {
            continue;
        }
        if result.len() == 63 {
            break;
        }
        result.push(normalized);
    }

    let result = result.trim_matches('-');
    if result.is_empty() {
        return empty_fallback.to_owned();
    }

    result.to_owned()
}

/// A normalized exact DNS hostname.
///
/// Names are ASCII lowercase and omit the optional trailing root dot. Wildcard
/// claims are intentionally outside this milestone.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DomainName(String);

impl DomainName {
    /// Return the canonical hostname.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Create a child hostname beneath this exact name.
    pub fn with_prefix(&self, prefix: &str) -> Result<Self, DomainError> {
        format!("{prefix}.{}", self.0).parse()
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for DomainName {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim() != value {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "leading or trailing whitespace is not allowed".to_owned(),
            });
        }
        if !value.is_ascii() {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "only ASCII hostnames are supported".to_owned(),
            });
        }
        if value.contains("://") || value.contains('/') || value.contains('\\') {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "use a hostname without a scheme or path".to_owned(),
            });
        }
        if value.contains(':') {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "ports are internal to locald and cannot appear in a domain claim"
                    .to_owned(),
            });
        }
        if value.contains('*') {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "wildcard claims are not supported by the exact domain index".to_owned(),
            });
        }

        let canonical = value
            .strip_suffix('.')
            .unwrap_or(value)
            .to_ascii_lowercase();
        if canonical.is_empty() {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "the hostname is empty".to_owned(),
            });
        }
        if canonical.len() > 253 {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "the hostname exceeds 253 characters".to_owned(),
            });
        }

        for label in canonical.split('.') {
            if label.is_empty() {
                return Err(DomainError::InvalidName {
                    value: value.to_owned(),
                    reason: "empty DNS labels are not allowed".to_owned(),
                });
            }
            if label.len() > 63 {
                return Err(DomainError::InvalidName {
                    value: value.to_owned(),
                    reason: format!("DNS label `{label}` exceeds 63 characters"),
                });
            }
            let bytes = label.as_bytes();
            let valid_edge = |byte: u8| byte.is_ascii_alphanumeric();
            if !valid_edge(bytes[0]) || !valid_edge(bytes[bytes.len() - 1]) {
                return Err(DomainError::InvalidName {
                    value: value.to_owned(),
                    reason: format!(
                        "DNS label `{label}` must start and end with a letter or digit"
                    ),
                });
            }
            if !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            {
                return Err(DomainError::InvalidName {
                    value: value.to_owned(),
                    reason: format!("DNS label `{label}` contains an unsupported character"),
                });
            }
        }

        Ok(Self(canonical))
    }
}

impl<'de> Deserialize<'de> for DomainName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// A locald-owned platform surface used when no project service overrides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformDomain {
    Dashboard,
    Docs,
    DashboardDev,
    DocsDev,
}

/// The durable owner and routing target for one exact hostname.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DomainTarget {
    Platform {
        surface: PlatformDomain,
    },
    Service {
        project_instance_id: ProjectInstanceId,
        /// Legacy flat claims may not identify their service. All newly applied
        /// configuration records the full runtime service name.
        service_name: Option<String>,
    },
}

impl fmt::Display for DomainTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platform { surface } => write!(formatter, "locald platform {surface:?}"),
            Self::Service {
                project_instance_id,
                service_name,
            } => match service_name {
                Some(service_name) => {
                    write!(
                        formatter,
                        "service `{service_name}` in instance {project_instance_id}"
                    )
                }
                None => write!(formatter, "legacy claim in instance {project_instance_id}"),
            },
        }
    }
}

/// One desired exact hostname claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainClaim {
    pub domain: DomainName,
    pub target: DomainTarget,
}

impl DomainClaim {
    /// Build a service claim with its stable instance and current runtime name.
    #[must_use]
    pub const fn service(
        domain: DomainName,
        project_instance_id: ProjectInstanceId,
        service_name: String,
    ) -> Self {
        Self {
            domain,
            target: DomainTarget::Service {
                project_instance_id,
                service_name: Some(service_name),
            },
        }
    }

    /// Build an ownership-only claim imported from the original flat catalog field.
    #[must_use]
    pub const fn legacy(domain: DomainName, project_instance_id: ProjectInstanceId) -> Self {
        Self {
            domain,
            target: DomainTarget::Service {
                project_instance_id,
                service_name: None,
            },
        }
    }
}

/// A complete immutable snapshot of all exact domains owned by locald.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainIndex {
    claims: BTreeMap<DomainName, DomainTarget>,
}

impl Default for DomainIndex {
    fn default() -> Self {
        let mut claims = BTreeMap::new();
        for (name, surface) in PLATFORM_DOMAINS {
            claims.insert(
                DomainName((*name).to_owned()),
                DomainTarget::Platform { surface: *surface },
            );
        }
        Self { claims }
    }
}

const PLATFORM_DOMAINS: &[(&str, PlatformDomain)] = &[
    ("localhost", PlatformDomain::Dashboard),
    ("locald.localhost", PlatformDomain::Dashboard),
    ("locald.local", PlatformDomain::Dashboard),
    ("docs.localhost", PlatformDomain::Docs),
    ("docs.local", PlatformDomain::Docs),
    ("dev.locald.localhost", PlatformDomain::DashboardDev),
    ("dev.locald.local", PlatformDomain::DashboardDev),
    ("dev.docs.localhost", PlatformDomain::DocsDev),
    ("dev.docs.local", PlatformDomain::DocsDev),
];

impl DomainIndex {
    /// Return the exact target for a normalized or normalizable hostname.
    #[must_use]
    pub fn resolve(&self, domain: &str) -> Option<&DomainTarget> {
        let domain = domain.parse::<DomainName>().ok()?;
        self.claims.get(&domain)
    }

    /// Return the complete persisted map.
    #[must_use]
    pub const fn claims(&self) -> &BTreeMap<DomainName, DomainTarget> {
        &self.claims
    }

    /// Return an instance service's exact domain. Milestone A assigns one claim
    /// per service within each project instance.
    #[must_use]
    pub fn domain_for_service(
        &self,
        instance_id: ProjectInstanceId,
        service_name: &str,
    ) -> Option<&DomainName> {
        self.claims
            .iter()
            .find_map(|(domain, target)| match target {
                DomainTarget::Service {
                    project_instance_id,
                    service_name: Some(candidate),
                } if *project_instance_id == instance_id && candidate == service_name => {
                    Some(domain)
                }
                DomainTarget::Platform { .. }
                | DomainTarget::Service {
                    service_name: None, ..
                }
                | DomainTarget::Service {
                    service_name: Some(_),
                    ..
                } => None,
            })
    }

    /// Return all project-service domains for hosts synchronization.
    #[must_use]
    pub fn service_domains(&self) -> Vec<String> {
        self.claims
            .iter()
            .filter(|(_, target)| matches!(target, DomainTarget::Service { .. }))
            .map(|(domain, _)| domain.to_string())
            .collect()
    }

    /// Return the exact hostnames that require explicit hosts-file mappings.
    ///
    /// Project-service domains remain in the compatibility projection. Platform
    /// `.local` aliases also require an entry because, unlike `.localhost`, they
    /// do not resolve to loopback by definition.
    #[must_use]
    pub fn hosts_domains(&self) -> Vec<String> {
        self.claims
            .iter()
            .filter(|(domain, target)| {
                matches!(target, DomainTarget::Service { .. })
                    || matches!(target, DomainTarget::Platform { .. })
                        && domain
                            .as_str()
                            .rsplit_once('.')
                            .is_some_and(|(_, suffix)| suffix == "local")
            })
            .map(|(domain, _)| domain.to_string())
            .collect()
    }

    /// Return every exact hostname owned by one project instance.
    #[must_use]
    pub fn domains_for_instance(&self, instance_id: ProjectInstanceId) -> BTreeSet<String> {
        self.claims
            .iter()
            .filter_map(|(domain, target)| match target {
                DomainTarget::Service {
                    project_instance_id,
                    ..
                } if *project_instance_id == instance_id => Some(domain.to_string()),
                DomainTarget::Platform { .. } | DomainTarget::Service { .. } => None,
            })
            .collect()
    }

    /// Validate and replace an instance's complete claim set in one new snapshot.
    pub fn replacing_instance(
        &self,
        instance_id: ProjectInstanceId,
        desired: impl IntoIterator<Item = DomainClaim>,
    ) -> Result<Self, DomainError> {
        let mut replacement = self.clone();
        replacement.claims.retain(|_, target| {
            !matches!(
                target,
                DomainTarget::Service {
                    project_instance_id,
                    ..
                } if *project_instance_id == instance_id
            )
        });
        replacement.restore_platform_fallbacks();

        let mut incoming = BTreeMap::<DomainName, DomainTarget>::new();
        for claim in desired {
            match &claim.target {
                DomainTarget::Service {
                    project_instance_id,
                    ..
                } if *project_instance_id == instance_id => {}
                DomainTarget::Service {
                    project_instance_id,
                    ..
                } => {
                    return Err(DomainError::WrongInstance {
                        expected: instance_id,
                        actual: *project_instance_id,
                        domain: claim.domain,
                    });
                }
                DomainTarget::Platform { .. } => {
                    return Err(DomainError::PlatformReplacement {
                        instance_id,
                        domain: claim.domain,
                    });
                }
            }

            if let Some(existing) = incoming.insert(claim.domain.clone(), claim.target.clone()) {
                return Err(DomainError::Conflict {
                    domain: claim.domain,
                    existing,
                    requested: claim.target,
                });
            }
        }

        for (domain, target) in incoming {
            match replacement.claims.get(&domain) {
                Some(DomainTarget::Platform { .. }) if platform_surface(&domain).is_some() => {
                    replacement.claims.insert(domain, target);
                }
                Some(existing) => {
                    return Err(DomainError::Conflict {
                        domain,
                        existing: existing.clone(),
                        requested: target,
                    });
                }
                None => {
                    replacement.claims.insert(domain, target);
                }
            }
        }
        replacement.validate()?;
        Ok(replacement)
    }

    fn restore_platform_fallbacks(&mut self) {
        for (name, surface) in PLATFORM_DOMAINS {
            self.claims
                .entry(DomainName((*name).to_owned()))
                .or_insert(DomainTarget::Platform { surface: *surface });
        }
    }

    /// Check platform fallbacks, exact owners, and per-instance service targets.
    pub fn validate(&self) -> Result<(), DomainError> {
        for (name, surface) in PLATFORM_DOMAINS {
            let domain = DomainName((*name).to_owned());
            let expected = DomainTarget::Platform { surface: *surface };
            match self.claims.get(&domain) {
                Some(actual)
                    if actual == &expected || matches!(actual, DomainTarget::Service { .. }) => {}
                Some(actual) => {
                    return Err(DomainError::Conflict {
                        domain,
                        existing: expected,
                        requested: actual.clone(),
                    });
                }
                None => return Err(DomainError::MissingPlatformDomain { domain }),
            }
        }

        let mut runtime_services = BTreeMap::<(ProjectInstanceId, &str), &DomainName>::new();
        for (domain, target) in &self.claims {
            match target {
                DomainTarget::Platform { surface } => {
                    if platform_surface(domain) != Some(*surface) {
                        return Err(DomainError::UnexpectedPlatformDomain {
                            domain: domain.clone(),
                            surface: *surface,
                        });
                    }
                }
                DomainTarget::Service {
                    project_instance_id,
                    service_name: Some(service_name),
                } => {
                    if let Some(existing_domain) =
                        runtime_services.insert((*project_instance_id, service_name), domain)
                    {
                        return Err(DomainError::RuntimeServiceConflict {
                            service_name: service_name.clone(),
                            existing_domain: existing_domain.clone(),
                            existing_instance: *project_instance_id,
                            requested_domain: domain.clone(),
                            requested_instance: *project_instance_id,
                        });
                    }
                }
                DomainTarget::Service {
                    service_name: None, ..
                } => {}
            }
        }
        Ok(())
    }
}

fn platform_surface(domain: &DomainName) -> Option<PlatformDomain> {
    PLATFORM_DOMAINS
        .iter()
        .find_map(|(name, surface)| (domain.as_str() == *name).then_some(*surface))
}

/// A synchronously readable handle that swaps complete immutable snapshots.
#[derive(Clone, Debug)]
pub struct SharedDomainIndex(Arc<RwLock<Arc<DomainIndex>>>);

impl SharedDomainIndex {
    #[must_use]
    pub fn new(index: DomainIndex) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(index))))
    }

    /// Load one internally consistent immutable snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Arc<DomainIndex> {
        match self.0.read() {
            Ok(snapshot) => Arc::clone(&snapshot),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    /// Publish a fully validated replacement with one synchronous swap.
    pub fn store(&self, index: DomainIndex) {
        match self.0.write() {
            Ok(mut snapshot) => *snapshot = Arc::new(index),
            Err(poisoned) => *poisoned.into_inner() = Arc::new(index),
        }
    }
}

impl Default for SharedDomainIndex {
    fn default() -> Self {
        Self::new(DomainIndex::default())
    }
}

/// An invalid exact claim or ownership conflict.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("invalid domain `{value}`: {reason}")]
    InvalidName { value: String, reason: String },

    #[error(
        "domain `{domain}` is already owned by {existing}; it cannot also be claimed by {requested}"
    )]
    Conflict {
        domain: DomainName,
        existing: DomainTarget,
        requested: DomainTarget,
    },

    #[error("domain `{domain}` belongs to instance {actual}, not replacement instance {expected}")]
    WrongInstance {
        expected: ProjectInstanceId,
        actual: ProjectInstanceId,
        domain: DomainName,
    },

    #[error("project instance {instance_id} cannot install platform ownership for `{domain}`")]
    PlatformReplacement {
        instance_id: ProjectInstanceId,
        domain: DomainName,
    },

    #[error("domain index is missing platform ownership for `{domain}`")]
    MissingPlatformDomain { domain: DomainName },

    #[error("platform surface {surface:?} is assigned to unexpected domain `{domain}`")]
    UnexpectedPlatformDomain {
        domain: DomainName,
        surface: PlatformDomain,
    },

    #[error(
        "service `{service_name}` in instance {existing_instance} is already targeted by `{existing_domain}`; it cannot also be targeted by `{requested_domain}` in instance {requested_instance}"
    )]
    RuntimeServiceConflict {
        service_name: String,
        existing_domain: DomainName,
        existing_instance: ProjectInstanceId,
        requested_domain: DomainName,
        requested_instance: ProjectInstanceId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ProjectId, WorktreeId, derive_project_instance_id};
    use std::str::FromStr;
    use uuid::Uuid;

    fn instance(seed: u128) -> ProjectInstanceId {
        let worktree =
            WorktreeId::from_str(&Uuid::from_u128(seed).to_string()).expect("valid worktree UUID");
        let project = ProjectId::from_str(&Uuid::from_u128(seed + 1).to_string())
            .expect("valid project UUID");
        derive_project_instance_id(worktree, project)
    }

    fn service_claim(domain: &str, instance_id: ProjectInstanceId, service: &str) -> DomainClaim {
        DomainClaim::service(
            domain.parse().expect("valid test domain"),
            instance_id,
            service.to_owned(),
        )
    }

    #[test]
    fn exact_names_normalize_case_and_one_trailing_dot() {
        let domain: DomainName = "API.Example.Localhost.".parse().expect("valid domain");

        assert_eq!(domain.as_str(), "api.example.localhost");
        assert_eq!(
            domain,
            "api.example.localhost".parse().expect("valid domain")
        );
    }

    #[test]
    fn project_names_receive_deterministic_dns_labels() {
        assert_eq!(sanitize_project_name_for_dns("My_App v2!"), "my-app-v2");
        assert_eq!(
            sanitize_project_name_for_dns("--My___App // v2--"),
            "my-app-v2"
        );
        assert_eq!(sanitize_project_name_for_dns("préview"), "pr-view");
        assert_eq!(sanitize_project_name_for_dns("東京_App"), "app");
        assert_eq!(sanitize_project_name_for_dns(""), "project");
        assert_eq!(sanitize_project_name_for_dns("---"), "project");

        let boundary = "a".repeat(63);
        assert_eq!(sanitize_project_name_for_dns(&boundary), boundary);
        assert_eq!(sanitize_project_name_for_dns(&"a".repeat(64)).len(), 63);
        assert_eq!(
            sanitize_project_name_for_dns(&format!("{}-suffix", "a".repeat(62))),
            "a".repeat(62)
        );
    }

    #[test]
    fn service_names_receive_service_specific_dns_labels() {
        assert_eq!(sanitize_service_name_for_dns("My_API v2!"), "my-api-v2");
        assert_eq!(sanitize_service_name_for_dns(""), "service");
        assert_eq!(sanitize_service_name_for_dns("---"), "service");
    }

    #[test]
    fn invalid_exact_names_are_rejected() {
        for invalid in [
            "",
            ".",
            "api..localhost",
            "-api.localhost",
            "api-.localhost",
            "api_localhost",
            "*.localhost",
            "https://api.localhost",
            "api.localhost/path",
            "api.localhost:443",
            " api.localhost",
        ] {
            assert!(
                invalid.parse::<DomainName>().is_err(),
                "`{invalid}` should be invalid"
            );
        }
    }

    #[test]
    fn dns_length_limits_are_enforced() {
        let long_label = "a".repeat(64);
        assert!(
            format!("{long_label}.localhost")
                .parse::<DomainName>()
                .is_err()
        );

        let too_long = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(62)
        );
        assert!(too_long.parse::<DomainName>().is_err());
    }

    #[test]
    fn platform_domains_have_owned_fallbacks() {
        let index = DomainIndex::default();

        assert_eq!(
            index.resolve("DOCS.LOCALHOST."),
            Some(&DomainTarget::Platform {
                surface: PlatformDomain::Docs
            })
        );
        assert!(index.validate().is_ok());
        assert_eq!(
            index.hosts_domains(),
            [
                "dev.docs.local",
                "dev.locald.local",
                "docs.local",
                "locald.local",
            ]
        );
    }

    #[test]
    fn hosts_projection_combines_service_claims_with_platform_aliases() {
        let instance_id = instance(5);
        let index = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_claim("app.localhost", instance_id, "app:web"),
                    service_claim("docs.local", instance_id, "app:docs"),
                ],
            )
            .expect("service claims");

        assert_eq!(
            index.hosts_domains(),
            [
                "app.localhost",
                "dev.docs.local",
                "dev.locald.local",
                "docs.local",
                "locald.local",
            ]
        );
    }

    #[test]
    fn whole_instance_replacement_removes_stale_claims() {
        let instance_id = instance(10);
        let first = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_claim("app.localhost", instance_id, "app:web"),
                    service_claim("api.app.localhost", instance_id, "app:api"),
                ],
            )
            .expect("first claim set");

        let second = first
            .replacing_instance(
                instance_id,
                [service_claim("new.localhost", instance_id, "app:web")],
            )
            .expect("replacement claim set");

        assert!(second.resolve("app.localhost").is_none());
        assert!(second.resolve("api.app.localhost").is_none());
        assert!(matches!(
            second.resolve("new.localhost"),
            Some(DomainTarget::Service { .. })
        ));
    }

    #[test]
    fn same_candidate_duplicates_are_rejected() {
        let instance_id = instance(20);
        let error = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_claim("app.localhost", instance_id, "app:web"),
                    service_claim("app.localhost", instance_id, "app:api"),
                ],
            )
            .expect_err("duplicate must fail");

        assert!(matches!(error, DomainError::Conflict { .. }));
    }

    #[test]
    fn cross_instance_conflict_preserves_the_old_snapshot() {
        let first_instance = instance(30);
        let second_instance = instance(40);
        let current = DomainIndex::default()
            .replacing_instance(
                first_instance,
                [service_claim(
                    "shared.localhost",
                    first_instance,
                    "first:web",
                )],
            )
            .expect("first claim");

        let error = current
            .replacing_instance(
                second_instance,
                [service_claim(
                    "shared.localhost",
                    second_instance,
                    "second:web",
                )],
            )
            .expect_err("conflict must fail");

        assert!(error.to_string().contains("first:web"));
        assert!(error.to_string().contains("second:web"));
        assert!(matches!(
            current.resolve("shared.localhost"),
            Some(DomainTarget::Service {
                project_instance_id,
                ..
            }) if *project_instance_id == first_instance
        ));
    }

    #[test]
    fn projects_override_platform_domains_and_removal_restores_the_fallback() {
        let instance_id = instance(50);

        let overridden = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [service_claim("docs.localhost", instance_id, "app:web")],
            )
            .expect("platform override");

        assert!(matches!(
            overridden.resolve("docs.localhost"),
            Some(DomainTarget::Service {
                project_instance_id,
                ..
            }) if *project_instance_id == instance_id
        ));

        let restored = overridden
            .replacing_instance(instance_id, std::iter::empty())
            .expect("remove platform override");

        assert_eq!(
            restored.resolve("docs.localhost"),
            Some(&DomainTarget::Platform {
                surface: PlatformDomain::Docs
            })
        );
    }

    #[test]
    fn same_runtime_service_name_is_allowed_in_distinct_instances() {
        let first_instance = instance(70);
        let second_instance = instance(80);
        let index = DomainIndex::default()
            .replacing_instance(
                first_instance,
                [service_claim("first.localhost", first_instance, "same:web")],
            )
            .expect("first claim")
            .replacing_instance(
                second_instance,
                [service_claim(
                    "second.localhost",
                    second_instance,
                    "same:web",
                )],
            )
            .expect("same display key belongs to a distinct instance");

        assert!(matches!(
            index.resolve("first.localhost"),
            Some(DomainTarget::Service {
                project_instance_id,
                service_name: Some(service_name),
            }) if *project_instance_id == first_instance && service_name == "same:web"
        ));
        assert!(matches!(
            index.resolve("second.localhost"),
            Some(DomainTarget::Service {
                project_instance_id,
                service_name: Some(service_name),
            }) if *project_instance_id == second_instance && service_name == "same:web"
        ));
    }

    #[test]
    fn duplicate_runtime_service_target_within_one_instance_is_rejected() {
        let instance_id = instance(90);
        let error = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_claim("first.localhost", instance_id, "same:web"),
                    service_claim("second.localhost", instance_id, "same:web"),
                ],
            )
            .expect_err("one service cannot target two exact domains in milestone A");

        assert!(matches!(error, DomainError::RuntimeServiceConflict { .. }));
        assert!(error.to_string().contains("first.localhost"));
        assert!(error.to_string().contains("second.localhost"));
    }

    #[test]
    fn shared_handle_swaps_complete_snapshots() {
        let instance_id = instance(60);
        let handle = SharedDomainIndex::default();
        let old = handle.snapshot();
        let replacement = old
            .replacing_instance(
                instance_id,
                [service_claim("app.localhost", instance_id, "app:web")],
            )
            .expect("replacement");

        handle.store(replacement);

        assert!(old.resolve("app.localhost").is_none());
        assert!(handle.snapshot().resolve("app.localhost").is_some());
    }
}
