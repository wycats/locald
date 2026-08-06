//! Privacy-safe ambient agent context and status projections.
//!
//! Host adapters derive conversation ownership and workspace locators from
//! private request metadata. The model-facing tool surface never accepts
//! those values as arguments, and the daemon persists only an opaque digest.

use crate::availability::ProjectAvailabilityStatus;
use crate::ipc::{PublicationStatus, ServiceType};
use crate::state::{HealthStatus, ServiceState};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

/// The protocol version shared by the trusted locald MCP adapter and daemon.
pub const AGENT_ADAPTER_PROTOCOL_VERSION: u32 = 1;
const AGENT_CONVERSATION_DIGEST_DOMAIN: &[u8] = b"locald-agent-conversation-v1\0";

/// A failure to construct or validate private ambient agent context.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AgentContextError {
    #[error("agent conversation identity must not be empty")]
    EmptyConversationIdentity,
    #[error("agent conversation key is not a valid opaque SHA-256 digest")]
    InvalidConversationKey,
    #[error(
        "unsupported locald agent-adapter protocol version {found}; expected {expected}; update locald and restart the agent adapter"
    )]
    UnsupportedProtocol { found: u32, expected: u32 },
    #[error("agent workspace context contains no trusted workspace locator")]
    MissingWorkspaceLocator,
}

/// A stable digest of one private host-provided conversation identity.
///
/// Debug output is deliberately redacted. Serialization exposes only the
/// domain-separated digest used by daemon IPC and catalog persistence.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(transparent)]
pub struct AgentConversationKey(String);

impl AgentConversationKey {
    /// Digest one canonical private conversation identity before IPC or storage.
    pub fn digest(private_identity: &str) -> Result<Self, AgentContextError> {
        if private_identity.trim().is_empty() {
            return Err(AgentContextError::EmptyConversationIdentity);
        }
        let mut hasher = Sha256::new();
        hasher.update(AGENT_CONVERSATION_DIGEST_DOMAIN);
        hasher.update(private_identity.as_bytes());
        Ok(Self(format!("{:x}", hasher.finalize())))
    }

    /// Return the already-opaque value used to derive a demand owner.
    #[must_use]
    pub fn as_opaque_str(&self) -> &str {
        &self.0
    }

    /// Validate an opaque value loaded from IPC or durable state.
    pub fn validate(&self) -> Result<(), AgentContextError> {
        if self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(())
        } else {
            Err(AgentContextError::InvalidConversationKey)
        }
    }
}

impl fmt::Debug for AgentConversationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentConversationKey([redacted])")
    }
}

impl Serialize for AgentConversationKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AgentConversationKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let key = Self(value);
        key.validate().map_err(serde::de::Error::custom)?;
        Ok(key)
    }
}

/// Private host context supplied by the authenticated locald MCP adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkspaceContext {
    pub protocol_version: u32,
    pub conversation: AgentConversationKey,
    /// Workspace roots returned by the MCP client.
    #[serde(default)]
    pub workspace_roots: Vec<PathBuf>,
    /// Trusted Codex sandbox workspace metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_cwd: Option<PathBuf>,
    /// Adapter process working directory, used only as an unambiguous fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_cwd: Option<PathBuf>,
}

impl AgentWorkspaceContext {
    /// Validate protocol and required private context before resolution.
    pub fn validate(&self) -> Result<(), AgentContextError> {
        if self.protocol_version != AGENT_ADAPTER_PROTOCOL_VERSION {
            return Err(AgentContextError::UnsupportedProtocol {
                found: self.protocol_version,
                expected: AGENT_ADAPTER_PROTOCOL_VERSION,
            });
        }
        self.conversation.validate()?;
        if self.workspace_roots.is_empty()
            && self.sandbox_cwd.is_none()
            && self.process_cwd.is_none()
        {
            return Err(AgentContextError::MissingWorkspaceLocator);
        }
        Ok(())
    }
}

/// Whether an ambiently discovered project already has daemon-owned identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentProjectRegistration {
    Registered,
    Unregistered,
}

/// Privacy-safe worktree metadata useful to an ambient coding agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentWorktreeStatus {
    pub linked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// Privacy-safe service state for normal agent tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentServiceStatus {
    pub name: String,
    #[serde(default)]
    pub service_type: ServiceType,
    pub status: ServiceState,
    #[serde(default)]
    pub health_status: HealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<PublicationStatus>,
}

/// Ambient project state returned by agent-facing daemon APIs.
///
/// This projection intentionally excludes ports, PIDs, project UUIDs, demand
/// owner IDs, activity generations, and raw conversation provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentProjectStatus {
    pub registration: AgentProjectRegistration,
    pub project_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<AgentWorktreeStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<ProjectAvailabilityStatus>,
    #[serde(default)]
    pub services: Vec<AgentServiceStatus>,
    #[serde(default)]
    pub urls: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_ADAPTER_PROTOCOL_VERSION, AgentContextError, AgentConversationKey,
        AgentWorkspaceContext,
    };
    use std::path::PathBuf;

    #[test]
    fn conversation_key_is_stable_redacted_and_contains_no_private_identity() {
        let private = "issuer=codex;thread=private-thread-123";
        let first = AgentConversationKey::digest(private).expect("digest identity");
        let second = AgentConversationKey::digest(private).expect("digest identity again");

        assert_eq!(first, second);
        assert_eq!(first.as_opaque_str().len(), 64);
        assert!(!first.as_opaque_str().contains("private-thread"));
        assert_eq!(format!("{first:?}"), "AgentConversationKey([redacted])");
        let encoded = serde_json::to_string(&first).expect("serialize opaque key");
        assert!(!encoded.contains("private-thread"));
    }

    #[test]
    fn deserialization_rejects_non_digest_conversation_values() {
        let error = serde_json::from_str::<AgentConversationKey>("\"private-thread-123\"")
            .expect_err("raw identity must not deserialize");
        assert!(error.to_string().contains("opaque SHA-256 digest"));
    }

    #[test]
    fn workspace_context_requires_supported_protocol_and_locator() {
        let conversation = AgentConversationKey::digest("conversation").expect("digest identity");
        let empty = AgentWorkspaceContext {
            protocol_version: AGENT_ADAPTER_PROTOCOL_VERSION,
            conversation: conversation.clone(),
            workspace_roots: Vec::new(),
            sandbox_cwd: None,
            process_cwd: None,
        };
        assert_eq!(
            empty.validate(),
            Err(AgentContextError::MissingWorkspaceLocator)
        );

        let wrong_version = AgentWorkspaceContext {
            protocol_version: AGENT_ADAPTER_PROTOCOL_VERSION + 1,
            conversation,
            workspace_roots: vec![PathBuf::from("/tmp/project")],
            sandbox_cwd: None,
            process_cwd: None,
        };
        assert!(matches!(
            wrong_version.validate(),
            Err(AgentContextError::UnsupportedProtocol { .. })
        ));
    }
}
