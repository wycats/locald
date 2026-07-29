//! Domain ownership shared by routing, status, hosts, and TLS.

use crate::identity::ProjectInstanceId;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
/// ownership composes this exact type as a suffix through [`DomainPattern`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
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
                reason: "wildcard syntax is not valid in an exact hostname".to_owned(),
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

/// An exact hostname or a leftmost, one-label wildcard claim.
///
/// Wildcards are stored by their exact suffix and serialize in the familiar
/// `*.example.localhost` form. A wildcard matches exactly one additional DNS
/// label; it never absorbs multiple labels.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(with = "String")]
pub enum DomainPattern {
    Exact(DomainName),
    Wildcard(DomainName),
}

impl DomainPattern {
    /// Build an exact claim.
    #[must_use]
    pub const fn exact(domain: DomainName) -> Self {
        Self::Exact(domain)
    }

    /// Build a one-label wildcard claim from its exact suffix.
    #[must_use]
    pub const fn wildcard(suffix: DomainName) -> Self {
        Self::Wildcard(suffix)
    }

    /// Return the underlying exact name or wildcard suffix.
    #[must_use]
    pub const fn domain(&self) -> &DomainName {
        match self {
            Self::Exact(domain) | Self::Wildcard(domain) => domain,
        }
    }

    /// Return whether this is an exact claim.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Exact(_))
    }

    /// Return whether this pattern owns a concrete hostname.
    #[must_use]
    pub fn matches(&self, concrete: &DomainName) -> bool {
        match self {
            Self::Exact(domain) => domain == concrete,
            Self::Wildcard(suffix) => {
                let Some(prefix) = concrete
                    .as_str()
                    .strip_suffix(suffix.as_str())
                    .and_then(|prefix| prefix.strip_suffix('.'))
                else {
                    return false;
                };
                !prefix.is_empty() && !prefix.contains('.')
            }
        }
    }
}

impl fmt::Display for DomainPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(domain) => domain.fmt(formatter),
            Self::Wildcard(suffix) => write!(formatter, "*.{suffix}"),
        }
    }
}

impl From<DomainName> for DomainPattern {
    fn from(domain: DomainName) -> Self {
        Self::Exact(domain)
    }
}

impl FromStr for DomainPattern {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(suffix) = value.strip_prefix("*.") {
            if suffix.contains('*') {
                return Err(DomainError::InvalidName {
                    value: value.to_owned(),
                    reason: "only one leftmost `*.` wildcard label is supported".to_owned(),
                });
            }
            return suffix.parse::<DomainName>().map(Self::Wildcard);
        }
        if value.contains('*') {
            return Err(DomainError::InvalidName {
                value: value.to_owned(),
                reason: "only a leftmost `*.` wildcard label is supported".to_owned(),
            });
        }
        value.parse::<DomainName>().map(Self::Exact)
    }
}

impl Serialize for DomainPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DomainPattern {
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

/// The durable owner and routing target for one domain claim.
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
        /// The exact claim used as the service's canonical semantic origin.
        ///
        /// Catalogs written before multi-domain services implicitly contain
        /// exactly one claim per service, so a missing field remains primary.
        #[serde(default = "default_primary_domain")]
        primary: bool,
    },
}

const fn default_primary_domain() -> bool {
    true
}

impl fmt::Display for DomainTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platform { surface } => write!(formatter, "locald platform {surface:?}"),
            Self::Service {
                project_instance_id,
                service_name,
                ..
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

/// One desired exact or one-label wildcard hostname claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainClaim {
    pub domain: DomainPattern,
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
        Self::service_pattern(
            DomainPattern::Exact(domain),
            project_instance_id,
            service_name,
            true,
        )
    }

    /// Build a service claim with explicit pattern and canonical-origin status.
    #[must_use]
    pub const fn service_pattern(
        domain: DomainPattern,
        project_instance_id: ProjectInstanceId,
        service_name: String,
        primary: bool,
    ) -> Self {
        Self {
            domain,
            target: DomainTarget::Service {
                project_instance_id,
                service_name: Some(service_name),
                primary,
            },
        }
    }

    /// Build an ownership-only claim imported from the original flat catalog field.
    #[must_use]
    pub const fn legacy(domain: DomainName, project_instance_id: ProjectInstanceId) -> Self {
        Self {
            domain: DomainPattern::Exact(domain),
            target: DomainTarget::Service {
                project_instance_id,
                service_name: None,
                primary: true,
            },
        }
    }
}

/// A complete immutable snapshot of all domains owned by locald.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainIndex {
    claims: BTreeMap<DomainName, DomainTarget>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    wildcard_claims: BTreeMap<DomainName, DomainTarget>,
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
        Self {
            claims,
            wildcard_claims: BTreeMap::new(),
        }
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
    /// Return the owner for a normalized or normalizable concrete hostname.
    ///
    /// Exact ownership wins over a wildcard. Wildcards match exactly one
    /// leading label.
    #[must_use]
    pub fn resolve(&self, domain: &str) -> Option<&DomainTarget> {
        let domain = domain.parse::<DomainName>().ok()?;
        self.claims.get(&domain).or_else(|| {
            self.wildcard_claims
                .iter()
                .filter(|(suffix, _)| DomainPattern::Wildcard((*suffix).clone()).matches(&domain))
                .max_by_key(|(suffix, _)| suffix.as_str().len())
                .map(|(_, target)| target)
        })
    }

    /// Return the finite certificate identity that authorizes one concrete SNI.
    ///
    /// Exact claims use their concrete hostname. A concrete hostname owned by
    /// a wildcard uses the reusable wildcard identity, preventing arbitrary
    /// matching labels from growing the certificate cache.
    #[must_use]
    pub fn certificate_name_for(&self, domain: &str) -> Option<String> {
        let domain = domain.parse::<DomainName>().ok()?;
        if self.claims.contains_key(&domain) {
            return Some(domain.to_string());
        }
        self.wildcard_claims
            .keys()
            .filter(|suffix| DomainPattern::Wildcard((*suffix).clone()).matches(&domain))
            .max_by_key(|suffix| suffix.as_str().len())
            .map(|suffix| format!("*.{suffix}"))
    }

    /// Return the complete persisted exact-claim map.
    #[must_use]
    pub const fn claims(&self) -> &BTreeMap<DomainName, DomainTarget> {
        &self.claims
    }

    /// Return the complete persisted wildcard-suffix map.
    #[must_use]
    pub const fn wildcard_claims(&self) -> &BTreeMap<DomainName, DomainTarget> {
        &self.wildcard_claims
    }

    /// Return an instance service's canonical exact domain.
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
                    primary: true,
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
        self.hosts_domain_names()
            .into_iter()
            .map(|domain| domain.to_string())
            .collect()
    }

    /// Return validated exact names that require explicit hosts-file mappings.
    #[must_use]
    pub fn hosts_domain_names(&self) -> Vec<DomainName> {
        self.claims
            .iter()
            .filter(|(domain, target)| {
                matches!(target, DomainTarget::Service { .. })
                    || (matches!(target, DomainTarget::Platform { .. })
                        && domain
                            .as_str()
                            .rsplit_once('.')
                            .is_some_and(|(_, suffix)| suffix == "local"))
            })
            .map(|(domain, _)| domain.clone())
            .collect()
    }

    /// Return exact service-owned names that macOS must map explicitly.
    ///
    /// `.localhost` is resolved natively. Historical `.local` platform
    /// fallbacks are intentionally omitted, while an identically named
    /// project-service claim remains an active custom domain.
    #[must_use]
    pub fn macos_hosts_domain_names(&self) -> Vec<DomainName> {
        self.claims
            .iter()
            .filter(|(domain, target)| {
                matches!(target, DomainTarget::Service { .. })
                    && domain.as_str() != "localhost"
                    && !domain.as_str().ends_with(".localhost")
            })
            .map(|(domain, _)| domain.clone())
            .collect()
    }

    /// Return every exact and wildcard claim owned by one project instance.
    #[must_use]
    pub fn domains_for_instance(&self, instance_id: ProjectInstanceId) -> BTreeSet<String> {
        let exact = self
            .claims
            .iter()
            .filter_map(|(domain, target)| match target {
                DomainTarget::Service {
                    project_instance_id,
                    ..
                } if *project_instance_id == instance_id => Some(domain.to_string()),
                DomainTarget::Platform { .. } | DomainTarget::Service { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        exact
            .into_iter()
            .chain(
                self.wildcard_claims
                    .iter()
                    .filter_map(|(suffix, target)| match target {
                        DomainTarget::Service {
                            project_instance_id,
                            ..
                        } if *project_instance_id == instance_id => Some(format!("*.{suffix}")),
                        DomainTarget::Platform { .. } | DomainTarget::Service { .. } => None,
                    }),
            )
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
        replacement.wildcard_claims.retain(|_, target| {
            !matches!(
                target,
                DomainTarget::Service {
                    project_instance_id,
                    ..
                } if *project_instance_id == instance_id
            )
        });
        replacement.restore_platform_fallbacks();

        let mut incoming_exact = BTreeMap::<DomainName, DomainTarget>::new();
        let mut incoming_wildcards = BTreeMap::<DomainName, DomainTarget>::new();
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

            let incoming = match &claim.domain {
                DomainPattern::Exact(_) => &mut incoming_exact,
                DomainPattern::Wildcard(_) => &mut incoming_wildcards,
            };
            if let Some(existing) =
                incoming.insert(claim.domain.domain().clone(), claim.target.clone())
            {
                return Err(DomainError::Conflict {
                    domain: claim.domain,
                    existing: Box::new(existing),
                    requested: Box::new(claim.target),
                });
            }
        }

        for (domain, target) in incoming_exact {
            match replacement.claims.get(&domain) {
                Some(DomainTarget::Platform { .. }) if platform_surface(&domain).is_some() => {
                    replacement.claims.insert(domain, target);
                }
                Some(existing) => {
                    return Err(DomainError::Conflict {
                        domain: DomainPattern::Exact(domain),
                        existing: Box::new(existing.clone()),
                        requested: Box::new(target),
                    });
                }
                None => {
                    replacement.claims.insert(domain, target);
                }
            }
        }
        for (suffix, target) in incoming_wildcards {
            match replacement.wildcard_claims.get(&suffix) {
                Some(existing) => {
                    return Err(DomainError::Conflict {
                        domain: DomainPattern::Wildcard(suffix),
                        existing: Box::new(existing.clone()),
                        requested: Box::new(target),
                    });
                }
                None => {
                    replacement.wildcard_claims.insert(suffix, target);
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

    /// Check platform fallbacks, owners, and canonical service origins.
    pub fn validate(&self) -> Result<(), DomainError> {
        for (name, surface) in PLATFORM_DOMAINS {
            let domain = DomainName((*name).to_owned());
            let expected = DomainTarget::Platform { surface: *surface };
            match self.claims.get(&domain) {
                Some(actual)
                    if actual == &expected || matches!(actual, DomainTarget::Service { .. }) => {}
                Some(actual) => {
                    return Err(DomainError::Conflict {
                        domain: DomainPattern::Exact(domain),
                        existing: Box::new(expected),
                        requested: Box::new(actual.clone()),
                    });
                }
                None => return Err(DomainError::MissingPlatformDomain { domain }),
            }
        }

        let mut claimed_services = BTreeSet::<(ProjectInstanceId, &str)>::new();
        let mut primary_domains = BTreeMap::<(ProjectInstanceId, &str), &DomainName>::new();
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
                    primary,
                } => {
                    claimed_services.insert((*project_instance_id, service_name));
                    if *primary
                        && let Some(existing_domain) =
                            primary_domains.insert((*project_instance_id, service_name), domain)
                    {
                        return Err(DomainError::PrimaryDomainConflict {
                            service_name: service_name.clone(),
                            existing_domain: existing_domain.clone(),
                            instance_id: *project_instance_id,
                            requested_domain: domain.clone(),
                        });
                    }
                }
                DomainTarget::Service {
                    service_name: None, ..
                } => {}
            }
        }
        for (suffix, target) in &self.wildcard_claims {
            match target {
                DomainTarget::Platform { .. } => {
                    return Err(DomainError::WildcardPlatformDomain {
                        domain: DomainPattern::Wildcard(suffix.clone()),
                    });
                }
                DomainTarget::Service {
                    service_name: None, ..
                } => {
                    return Err(DomainError::WildcardLegacyDomain {
                        domain: DomainPattern::Wildcard(suffix.clone()),
                    });
                }
                DomainTarget::Service { primary: true, .. } => {
                    return Err(DomainError::WildcardPrimaryDomain {
                        domain: DomainPattern::Wildcard(suffix.clone()),
                    });
                }
                DomainTarget::Service {
                    project_instance_id,
                    service_name: Some(service_name),
                    primary: false,
                } => {
                    claimed_services.insert((*project_instance_id, service_name));
                }
            }
        }
        for (instance_id, service_name) in claimed_services {
            if !primary_domains.contains_key(&(instance_id, service_name)) {
                return Err(DomainError::MissingPrimaryDomain {
                    service_name: service_name.to_owned(),
                    instance_id,
                });
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

/// An invalid domain claim or ownership conflict.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("invalid domain `{value}`: {reason}")]
    InvalidName { value: String, reason: String },

    #[error(
        "domain `{domain}` is already owned by {existing}; it cannot also be claimed by {requested}"
    )]
    Conflict {
        domain: DomainPattern,
        existing: Box<DomainTarget>,
        requested: Box<DomainTarget>,
    },

    #[error("domain `{domain}` belongs to instance {actual}, not replacement instance {expected}")]
    WrongInstance {
        expected: ProjectInstanceId,
        actual: ProjectInstanceId,
        domain: DomainPattern,
    },

    #[error("project instance {instance_id} cannot install platform ownership for `{domain}`")]
    PlatformReplacement {
        instance_id: ProjectInstanceId,
        domain: DomainPattern,
    },

    #[error("domain index is missing platform ownership for `{domain}`")]
    MissingPlatformDomain { domain: DomainName },

    #[error("platform surface {surface:?} is assigned to unexpected domain `{domain}`")]
    UnexpectedPlatformDomain {
        domain: DomainName,
        surface: PlatformDomain,
    },

    #[error(
        "service `{service_name}` in instance {instance_id} already uses `{existing_domain}` as its canonical origin; it cannot also use `{requested_domain}`"
    )]
    PrimaryDomainConflict {
        service_name: String,
        existing_domain: DomainName,
        instance_id: ProjectInstanceId,
        requested_domain: DomainName,
    },

    #[error(
        "service `{service_name}` in instance {instance_id} owns exact domains but has no canonical origin"
    )]
    MissingPrimaryDomain {
        service_name: String,
        instance_id: ProjectInstanceId,
    },

    #[error("platform ownership cannot use wildcard claim `{domain}`")]
    WildcardPlatformDomain { domain: DomainPattern },

    #[error("legacy ownership-only records cannot use wildcard claim `{domain}`")]
    WildcardLegacyDomain { domain: DomainPattern },

    #[error("wildcard claim `{domain}` cannot be a service's canonical origin")]
    WildcardPrimaryDomain { domain: DomainPattern },
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

    fn service_pattern_claim(
        domain: &str,
        instance_id: ProjectInstanceId,
        service: &str,
        primary: bool,
    ) -> DomainClaim {
        DomainClaim::service_pattern(
            domain.parse().expect("valid test domain pattern"),
            instance_id,
            service.to_owned(),
            primary,
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
    fn wildcard_patterns_accept_only_one_leftmost_label() {
        let pattern: DomainPattern = "*.FRAME.App.Localhost."
            .parse()
            .expect("valid wildcard pattern");
        assert_eq!(pattern.to_string(), "*.frame.app.localhost");

        for invalid in [
            "*",
            "*frame.app.localhost",
            "frame.*.app.localhost",
            "*.*.app.localhost",
            "*.api..localhost",
        ] {
            assert!(
                invalid.parse::<DomainPattern>().is_err(),
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
    fn macos_hosts_projection_uses_ownership_not_legacy_spelling() {
        let instance_id = instance(5);
        let index = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_claim("app.localhost", instance_id, "app:web"),
                    service_claim("docs.local", instance_id, "app:docs"),
                    service_claim("custom.example.test", instance_id, "app:custom"),
                ],
            )
            .expect("service claims");

        assert_eq!(
            index.macos_hosts_domain_names(),
            [
                "custom.example.test"
                    .parse::<DomainName>()
                    .expect("custom domain"),
                "docs.local"
                    .parse::<DomainName>()
                    .expect("explicit legacy-spelling service domain"),
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
    fn exact_claims_override_one_label_wildcards() {
        let wildcard_instance = instance(11);
        let exact_instance = instance(12);
        let index = DomainIndex::default()
            .replacing_instance(
                wildcard_instance,
                [
                    service_pattern_claim(
                        "frame.app.localhost",
                        wildcard_instance,
                        "app:frame",
                        true,
                    ),
                    service_pattern_claim(
                        "*.frame.app.localhost",
                        wildcard_instance,
                        "app:frame",
                        false,
                    ),
                ],
            )
            .expect("wildcard claim")
            .replacing_instance(
                exact_instance,
                [service_claim(
                    "special.frame.app.localhost",
                    exact_instance,
                    "special:web",
                )],
            )
            .expect("more-specific exact claim");

        assert!(matches!(
            index.resolve("other.frame.app.localhost"),
            Some(DomainTarget::Service {
                project_instance_id,
                ..
            }) if *project_instance_id == wildcard_instance
        ));
        assert!(matches!(
            index.resolve("special.frame.app.localhost"),
            Some(DomainTarget::Service {
                project_instance_id,
                ..
            }) if *project_instance_id == exact_instance
        ));
        assert!(index.resolve("deep.other.frame.app.localhost").is_none());
        assert_eq!(
            index.certificate_name_for("other.frame.app.localhost"),
            Some("*.frame.app.localhost".to_owned())
        );
        assert_eq!(
            index.certificate_name_for("special.frame.app.localhost"),
            Some("special.frame.app.localhost".to_owned())
        );
        assert!(
            index
                .certificate_name_for("deep.other.frame.app.localhost")
                .is_none()
        );
    }

    #[test]
    fn wildcard_claims_are_not_hosts_entries_and_leave_with_their_instance() {
        let instance_id = instance(13);
        let index = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_pattern_claim("frame.app.localhost", instance_id, "app:frame", true),
                    service_pattern_claim("*.frame.app.localhost", instance_id, "app:frame", false),
                ],
            )
            .expect("frame claims");

        assert_eq!(
            index
                .domain_for_service(instance_id, "app:frame")
                .map(DomainName::as_str),
            Some("frame.app.localhost")
        );
        assert!(
            index
                .hosts_domains()
                .iter()
                .all(|domain| !domain.starts_with("*."))
        );
        assert!(
            index
                .domains_for_instance(instance_id)
                .contains("*.frame.app.localhost")
        );

        let removed = index
            .replacing_instance(instance_id, std::iter::empty())
            .expect("remove claims");
        assert!(removed.resolve("preview.frame.app.localhost").is_none());
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
    fn wildcard_conflict_preserves_the_old_snapshot() {
        let first_instance = instance(41);
        let second_instance = instance(42);
        let current = DomainIndex::default()
            .replacing_instance(
                first_instance,
                [
                    service_pattern_claim(
                        "frame-first.app.localhost",
                        first_instance,
                        "first:frame",
                        true,
                    ),
                    service_pattern_claim(
                        "*.frame.app.localhost",
                        first_instance,
                        "first:frame",
                        false,
                    ),
                ],
            )
            .expect("first wildcard claim");

        let error = current
            .replacing_instance(
                second_instance,
                [
                    service_pattern_claim(
                        "frame-second.app.localhost",
                        second_instance,
                        "second:frame",
                        true,
                    ),
                    service_pattern_claim(
                        "*.frame.app.localhost",
                        second_instance,
                        "second:frame",
                        false,
                    ),
                ],
            )
            .expect_err("conflicting wildcard must fail");

        assert!(matches!(error, DomainError::Conflict { .. }));
        assert!(matches!(
            current.resolve("preview.frame.app.localhost"),
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
                ..
            }) if *project_instance_id == first_instance && service_name == "same:web"
        ));
        assert!(matches!(
            index.resolve("second.localhost"),
            Some(DomainTarget::Service {
                project_instance_id,
                service_name: Some(service_name),
                ..
            }) if *project_instance_id == second_instance && service_name == "same:web"
        ));
    }

    #[test]
    fn duplicate_primary_service_domains_within_one_instance_are_rejected() {
        let instance_id = instance(90);
        let error = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_claim("first.localhost", instance_id, "same:web"),
                    service_claim("second.localhost", instance_id, "same:web"),
                ],
            )
            .expect_err("one service cannot have two canonical origins");

        assert!(matches!(error, DomainError::PrimaryDomainConflict { .. }));
        assert!(error.to_string().contains("first.localhost"));
        assert!(error.to_string().contains("second.localhost"));
    }

    #[test]
    fn one_service_can_own_exact_aliases_with_one_primary() {
        let instance_id = instance(91);
        let index = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    service_pattern_claim("first.localhost", instance_id, "same:web", true),
                    service_pattern_claim("alias.localhost", instance_id, "same:web", false),
                ],
            )
            .expect("one primary and one alias");

        assert_eq!(
            index
                .domain_for_service(instance_id, "same:web")
                .map(DomainName::as_str),
            Some("first.localhost")
        );
        assert!(index.resolve("alias.localhost").is_some());
    }

    #[test]
    fn exact_service_domains_require_one_primary_origin() {
        let instance_id = instance(92);
        let error = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [service_pattern_claim(
                    "alias.localhost",
                    instance_id,
                    "same:web",
                    false,
                )],
            )
            .expect_err("an exact alias needs one canonical origin");

        assert!(matches!(error, DomainError::MissingPrimaryDomain { .. }));
    }

    #[test]
    fn wildcard_service_domains_require_an_exact_primary_origin() {
        let instance_id = instance(93);
        let error = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [service_pattern_claim(
                    "*.frame.app.localhost",
                    instance_id,
                    "app:frame",
                    false,
                )],
            )
            .expect_err("a wildcard service needs one exact canonical origin");

        assert!(matches!(error, DomainError::MissingPrimaryDomain { .. }));
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
