use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// Stable version-1 publisher error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StableErrorCode {
    /// The request was semantically invalid.
    InvalidRequest,
    /// The client and daemon do not share a protocol version.
    ProtocolMismatch,
    /// The request names a prior daemon lifetime.
    DaemonEpochChanged,
    /// First-use preparation exceeded its bounded deadline.
    PreparationTimedOut,
    /// The kernel-backed peer process identity could not be established.
    PeerIdentityUnavailable,
    /// The peer UID differs from the daemon UID.
    PeerUidMismatch,
    /// The project locator did not resolve.
    ProjectNotFound,
    /// The independently resolved project instance differed from the identity fence.
    ProjectInstanceMismatch,
    /// The declared service does not exist.
    ServiceNotFound,
    /// The service is not a published declaration.
    ServiceNotPublished,
    /// Project pause currently suppresses publication.
    ProjectPaused,
    /// A domain claim conflicts with another project instance.
    DomainConflict,
    /// The complete hosts set could not be synchronized.
    HostsSyncFailed,
    /// Another principal owns the acquisition preparation or attempt.
    AcquisitionInProgress,
    /// Another rebind attempt is current for the lease.
    RebindInProgress,
    /// The service already has a live lease.
    AlreadyPublished,
    /// The server-issued attempt is no longer current.
    AttemptStale,
    /// The server-issued attempt deadline elapsed.
    AttemptExpired,
    /// The request does not exactly match the current attempt.
    AttemptMismatch,
    /// The lease is no longer current.
    LeaseLost,
    /// A later binding superseded the named revision.
    BindingReplaced,
    /// The acknowledged origin differs from daemon authority.
    OriginMismatch,
    /// An operation requiring a listener descriptor received none.
    ListenerMissing,
    /// The transferred descriptor is not an eligible listener.
    ListenerInvalid,
    /// The listener is not bound exactly to IPv4 loopback.
    ListenerNotIpv4Loopback,
    /// Another socket can share the listener binding.
    ListenerShareable,
    /// The endpoint collides with one of locald's front-door listeners.
    ListenerFrontDoorConflict,
    /// The listener and daemon use different network namespaces.
    NetworkNamespaceMismatch,
    /// Namespace equality could not be proven.
    NetworkNamespaceUnverifiable,
    /// A rebind candidate failed its bounded health check.
    EndpointUnhealthy,
    /// An observational readiness wait reached its deadline.
    WaitTimedOut,
    /// The daemon is applying its serialized wake barrier.
    WakeBarrierPending,
    /// The operation was canceled.
    OperationCanceled,
    /// This host cannot implement the version-1 authority contract.
    PublicationUnsupported,
    /// An unexpected daemon failure occurred.
    Internal,
}

impl StableErrorCode {
    /// Return the normative retry class for this version-1 error.
    #[must_use]
    pub const fn retry_class(self) -> RetryClass {
        match self {
            Self::WaitTimedOut => RetryClass::SameHandle,
            Self::DaemonEpochChanged
            | Self::PreparationTimedOut
            | Self::AttemptStale
            | Self::AttemptExpired
            | Self::AttemptMismatch
            | Self::LeaseLost
            | Self::BindingReplaced
            | Self::EndpointUnhealthy => RetryClass::NewAttempt,
            Self::ProjectNotFound
            | Self::ProjectInstanceMismatch
            | Self::ServiceNotFound
            | Self::ServiceNotPublished
            | Self::ProjectPaused
            | Self::DomainConflict
            | Self::HostsSyncFailed
            | Self::AcquisitionInProgress
            | Self::RebindInProgress
            | Self::AlreadyPublished
            | Self::OriginMismatch
            | Self::ListenerMissing
            | Self::ListenerInvalid
            | Self::ListenerNotIpv4Loopback
            | Self::ListenerShareable
            | Self::ListenerFrontDoorConflict
            | Self::NetworkNamespaceMismatch
            | Self::NetworkNamespaceUnverifiable
            | Self::WakeBarrierPending
            | Self::Internal => RetryClass::AfterExternalChange,
            Self::InvalidRequest
            | Self::ProtocolMismatch
            | Self::PeerIdentityUnavailable
            | Self::PeerUidMismatch
            | Self::OperationCanceled
            | Self::PublicationUnsupported => RetryClass::Never,
        }
    }
}

/// Normative client retry disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    /// Repeat the observational operation with the same live handle.
    SameHandle,
    /// Obtain fresh server-issued attempt authority.
    NewAttempt,
    /// Wait until external state changes before retrying.
    AfterExternalChange,
    /// Do not retry this request.
    Never,
}

/// A structured publisher-protocol error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Error)]
#[error("{code:?}: {message}")]
pub struct ProtocolError {
    /// Stable machine-readable error code.
    code: StableErrorCode,
    /// Human-readable explanation.
    message: String,
    /// Normative retry disposition for `code`.
    retry: RetryClass,
    /// Optional actionable next step.
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
}

impl ProtocolError {
    /// Construct an error with the canonical retry classification.
    pub fn new(code: StableErrorCode, message: impl Into<String>, action: Option<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry: code.retry_class(),
            action,
        }
    }

    /// Verify that stable code and retry fields agree.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolErrorValidation`] when the serialized retry class is
    /// not the canonical class for this stable error code.
    pub fn validate(&self) -> Result<(), ProtocolErrorValidation> {
        let expected = self.code.retry_class();
        if self.retry != expected {
            return Err(ProtocolErrorValidation {
                code: self.code,
                expected,
                actual: self.retry,
            });
        }
        Ok(())
    }

    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> StableErrorCode {
        self.code
    }

    /// Normative retry class.
    #[must_use]
    pub const fn retry(&self) -> RetryClass {
        self.retry
    }

    /// Human-readable explanation.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Optional actionable next step.
    #[must_use]
    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }
}

/// A stable protocol error was paired with a noncanonical retry class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("error code {code:?} requires retry {expected:?}, got {actual:?}")]
pub struct ProtocolErrorValidation {
    code: StableErrorCode,
    expected: RetryClass,
    actual: RetryClass,
}

impl<'de> Deserialize<'de> for ProtocolError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            code: StableErrorCode,
            message: String,
            retry: RetryClass,
            action: Option<String>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let error = Self {
            code: raw.code,
            message: raw.message,
            retry: raw.retry,
            action: raw.action,
        };
        error.validate().map_err(serde::de::Error::custom)?;
        Ok(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_code_has_the_normative_retry_class() {
        assert_eq!(
            StableErrorCode::WaitTimedOut.retry_class(),
            RetryClass::SameHandle
        );
        assert_eq!(
            StableErrorCode::LeaseLost.retry_class(),
            RetryClass::NewAttempt
        );
        assert_eq!(
            StableErrorCode::ProjectPaused.retry_class(),
            RetryClass::AfterExternalChange
        );
        assert_eq!(
            StableErrorCode::WakeBarrierPending.retry_class(),
            RetryClass::AfterExternalChange
        );
        assert_eq!(
            StableErrorCode::ProtocolMismatch.retry_class(),
            RetryClass::Never
        );
    }

    #[test]
    fn mismatched_retry_class_is_rejected() {
        let wire = r#"{
            "code":"wait_timed_out",
            "message":"still waiting",
            "retry":"never"
        }"#;
        assert!(serde_json::from_str::<ProtocolError>(wire).is_err());
    }

    #[test]
    fn wake_barrier_pending_round_trips_with_canonical_retry() {
        let error = ProtocolError::new(
            StableErrorCode::WakeBarrierPending,
            "wake barrier pending",
            None,
        );
        let wire = serde_json::to_string(&error).expect("serialize wake barrier error");
        assert!(wire.contains(r#""code":"wake_barrier_pending""#));
        let decoded: ProtocolError =
            serde_json::from_str(&wire).expect("deserialize wake barrier error");
        assert_eq!(decoded.code(), StableErrorCode::WakeBarrierPending);
        assert_eq!(decoded.retry(), RetryClass::AfterExternalChange);
    }

    #[test]
    fn error_objects_reject_unknown_fields() {
        let wire = r#"{
            "code":"wait_timed_out",
            "message":"still waiting",
            "retry":"same_handle",
            "secret":"no"
        }"#;
        assert!(serde_json::from_str::<ProtocolError>(wire).is_err());
    }
}
