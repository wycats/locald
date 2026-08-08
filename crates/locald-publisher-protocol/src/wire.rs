use std::fmt;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::ser::SerializeStruct as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::{
    AbsolutePath, AcquisitionAttemptHandle, BindingRevision, DaemonEpoch, LeaseHandle,
    ProjectInstanceId, ProtocolError, RebindAttemptHandle, ScalarError, SemanticOrigin,
    ServiceName,
};

/// Version implemented by this crate.
pub const PROTOCOL_VERSION: u32 = 1;
/// Maximum JSON body accepted by either framed direction.
pub const MAX_FRAME_JSON_BYTES: usize = 65_536;
/// First-use preparation deadline.
pub const PREPARATION_TIMEOUT_MS: u64 = 60_000;
/// Acquisition and rebind attempt lifetime.
pub const ATTEMPT_TTL_MS: u64 = 15_000;
/// Publisher lease lifetime.
pub const LEASE_TTL_MS: u64 = 30_000;
/// Normal client renewal target.
pub const RENEW_TARGET_MS: u64 = 10_000;
/// Maximum duration of one readiness wait.
pub const WAIT_TIMEOUT_MS: u64 = 30_000;
/// Maximum time to deliver a request frame.
pub const FRAME_TIMEOUT_MS: u64 = 5_000;
/// Setup-owned installation-record filename under locald's standard data directory.
pub const INSTALLATION_RECORD_NAME: &str = "publisher-installation-v1.json";
/// Maximum setup-owned installation-record size.
pub const INSTALLATION_RECORD_MAX_BYTES: usize = 4_096;
/// Exact ordinary daemon socket advertised by a version-1 standard installation.
pub const STANDARD_COMMAND_SOCKET: &str = "/tmp/locald.sock";
/// Publisher socket path relative to the selected locald data directory.
pub const PUBLISHER_SOCKET_RELATIVE_PATH: &str = "run/publisher-v1.sock";

/// Authenticated ordinary-IPC discovery result for the publisher socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct PublishedEndpointProtocolInfo {
    /// Wire version accepted on the dedicated socket.
    protocol_version: u32,
    /// Random daemon-lifetime epoch.
    daemon_epoch: DaemonEpoch,
    /// Exact daemon-selected publisher socket path.
    publisher_socket: AbsolutePath,
    /// First-use preparation bound.
    preparation_timeout_ms: u64,
    /// Acquisition and rebind attempt lifetime.
    attempt_ttl_ms: u64,
    /// Publisher lease lifetime.
    lease_ttl_ms: u64,
    /// Normal publisher renewal target.
    renew_target_ms: u64,
    /// Maximum duration of one readiness wait.
    wait_timeout_ms: u64,
    /// Maximum frame-delivery time.
    frame_timeout_ms: u64,
}

impl PublishedEndpointProtocolInfo {
    /// Construct the only valid version-1 policy around daemon-selected authority.
    #[must_use]
    pub const fn v1(daemon_epoch: DaemonEpoch, publisher_socket: AbsolutePath) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            daemon_epoch,
            publisher_socket,
            preparation_timeout_ms: PREPARATION_TIMEOUT_MS,
            attempt_ttl_ms: ATTEMPT_TTL_MS,
            lease_ttl_ms: LEASE_TTL_MS,
            renew_target_ms: RENEW_TARGET_MS,
            wait_timeout_ms: WAIT_TIMEOUT_MS,
            frame_timeout_ms: FRAME_TIMEOUT_MS,
        }
    }

    /// Exact protocol version.
    #[must_use]
    pub const fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    /// Current random daemon epoch.
    #[must_use]
    pub const fn daemon_epoch(&self) -> &DaemonEpoch {
        &self.daemon_epoch
    }

    /// Exact dedicated publisher socket selected by the daemon.
    #[must_use]
    pub const fn publisher_socket(&self) -> &AbsolutePath {
        &self.publisher_socket
    }

    /// First-use preparation timeout in milliseconds.
    #[must_use]
    pub const fn preparation_timeout_ms(&self) -> u64 {
        self.preparation_timeout_ms
    }

    /// Attempt TTL in milliseconds.
    #[must_use]
    pub const fn attempt_ttl_ms(&self) -> u64 {
        self.attempt_ttl_ms
    }

    /// Lease TTL in milliseconds.
    #[must_use]
    pub const fn lease_ttl_ms(&self) -> u64 {
        self.lease_ttl_ms
    }

    /// Normal renewal target in milliseconds.
    #[must_use]
    pub const fn renew_target_ms(&self) -> u64 {
        self.renew_target_ms
    }

    /// Readiness wait timeout in milliseconds.
    #[must_use]
    pub const fn wait_timeout_ms(&self) -> u64 {
        self.wait_timeout_ms
    }

    /// Request-frame timeout in milliseconds.
    #[must_use]
    pub const fn frame_timeout_ms(&self) -> u64 {
        self.frame_timeout_ms
    }

    /// Validate the complete fixed version-1 policy.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError`] when any advertised value differs from the
    /// fixed version-1 protocol policy.
    pub fn validate(&self) -> Result<(), ScalarError> {
        check_policy(
            "protocol_version",
            u64::from(PROTOCOL_VERSION),
            u64::from(self.protocol_version),
        )?;
        check_policy(
            "preparation_timeout_ms",
            PREPARATION_TIMEOUT_MS,
            self.preparation_timeout_ms,
        )?;
        check_policy("attempt_ttl_ms", ATTEMPT_TTL_MS, self.attempt_ttl_ms)?;
        check_policy("lease_ttl_ms", LEASE_TTL_MS, self.lease_ttl_ms)?;
        check_policy("renew_target_ms", RENEW_TARGET_MS, self.renew_target_ms)?;
        check_policy("wait_timeout_ms", WAIT_TIMEOUT_MS, self.wait_timeout_ms)?;
        check_policy("frame_timeout_ms", FRAME_TIMEOUT_MS, self.frame_timeout_ms)?;
        Ok(())
    }
}

const fn check_policy(field: &'static str, expected: u64, actual: u64) -> Result<(), ScalarError> {
    if expected == actual {
        Ok(())
    } else {
        Err(ScalarError::InvalidProtocolPolicy {
            field,
            expected,
            actual,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedEndpointProtocolInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            protocol_version: u32,
            daemon_epoch: DaemonEpoch,
            publisher_socket: AbsolutePath,
            preparation_timeout_ms: u64,
            attempt_ttl_ms: u64,
            lease_ttl_ms: u64,
            renew_target_ms: u64,
            wait_timeout_ms: u64,
            frame_timeout_ms: u64,
        }

        let raw = Raw::deserialize(deserializer)?;
        let info = Self {
            protocol_version: raw.protocol_version,
            daemon_epoch: raw.daemon_epoch,
            publisher_socket: raw.publisher_socket,
            preparation_timeout_ms: raw.preparation_timeout_ms,
            attempt_ttl_ms: raw.attempt_ttl_ms,
            lease_ttl_ms: raw.lease_ttl_ms,
            renew_target_ms: raw.renew_target_ms,
            wait_timeout_ms: raw.wait_timeout_ms,
            frame_timeout_ms: raw.frame_timeout_ms,
        };
        info.validate().map_err(serde::de::Error::custom)?;
        Ok(info)
    }
}

/// Setup-owned installation discovery record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallationRecord {
    /// Record schema version. Version 1 is the only supported value.
    schema_version: u32,
    /// Publisher protocol version advertised by setup.
    publisher_protocol_version: u32,
    /// Ordinary authenticated daemon command socket.
    command_socket: AbsolutePath,
}

impl InstallationRecord {
    /// Construct the exact standard version-1 record.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError`] if the compiled standard command-socket path
    /// does not satisfy the protocol's absolute-path invariant.
    pub fn v1() -> Result<Self, ScalarError> {
        Ok(Self {
            schema_version: 1,
            publisher_protocol_version: PROTOCOL_VERSION,
            command_socket: AbsolutePath::parse(STANDARD_COMMAND_SOCKET)?,
        })
    }

    /// Record schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Advertised publisher protocol version.
    #[must_use]
    pub const fn publisher_protocol_version(&self) -> u32 {
        self.publisher_protocol_version
    }

    /// Exact standard command socket.
    #[must_use]
    pub const fn command_socket(&self) -> &AbsolutePath {
        &self.command_socket
    }

    /// Verify the exact version-1 record contract.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError`] when the record selects another schema or
    /// protocol version, or names a nonstandard command socket.
    pub fn validate(&self) -> Result<(), ScalarError> {
        check_policy("schema_version", 1, u64::from(self.schema_version))?;
        check_policy(
            "publisher_protocol_version",
            u64::from(PROTOCOL_VERSION),
            u64::from(self.publisher_protocol_version),
        )?;
        if self.command_socket.as_str() != STANDARD_COMMAND_SOCKET {
            return Err(ScalarError::InvalidInstallationCommandSocket);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for InstallationRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            schema_version: u32,
            publisher_protocol_version: u32,
            command_socket: AbsolutePath,
        }

        let raw = Raw::deserialize(deserializer)?;
        let record = Self {
            schema_version: raw.schema_version,
            publisher_protocol_version: raw.publisher_protocol_version,
            command_socket: raw.command_socket,
        };
        record.validate().map_err(serde::de::Error::custom)?;
        Ok(record)
    }
}

/// State of one server-issued acquisition or rebind attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    /// The attempt is available for its first capability-bearing request.
    Pending,
    /// An exact request is currently executing.
    InFlight,
    /// The attempt has one bounded terminal result available for exact replay.
    Terminal,
}

/// Privacy-safe publication state vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    /// The declaration exists without a publisher lease.
    WaitingForPublisher,
    /// A current binding is being checked.
    CheckingEndpoint,
    /// The current binding failed its HTTP policy.
    EndpointUnhealthy,
    /// The exact current binding has route authorization.
    Ready,
    /// Project pause suppresses the route.
    RoutePaused,
    /// The physical project instance is missing.
    InstanceMissing,
}

/// A successful result that violates the version-1 authority or timing contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ResultValidationError {
    /// Initial acquisition must install the first binding generation.
    #[error("initial acquisition binding_revision must be 1, got {actual}")]
    InitialBindingRevision {
        /// Rejected initial binding revision.
        actual: u64,
    },
    /// A successful lease operation returned a state that has no live lease.
    #[error("successful lease result cannot have publication_state {state}")]
    InactivePublicationState {
        /// Rejected unavailable publication state.
        state: PublicationState,
    },
    /// A server-issued attempt was already expired or exceeded its fixed TTL.
    #[error("attempt_expires_in_ms must be between 1 and {maximum} inclusive, got {actual}")]
    AttemptExpiresInOutOfRange {
        /// Rejected remaining lifetime.
        actual: u64,
        /// Fixed version-1 maximum.
        maximum: u64,
    },
    /// A lease was already expired or exceeded its fixed TTL.
    #[error("expires_in_ms must be between 1 and {maximum} inclusive, got {actual}")]
    LeaseExpiresInOutOfRange {
        /// Rejected remaining lifetime.
        actual: u64,
        /// Fixed version-1 maximum.
        maximum: u64,
    },
    /// A response asked the publisher to wait beyond the fixed renewal target.
    #[error("renew_after_ms must be at most {maximum}, got {actual}")]
    RenewAfterExceedsTarget {
        /// Rejected renewal delay.
        actual: u64,
        /// Fixed version-1 maximum.
        maximum: u64,
    },
    /// A response scheduled renewal after the advertised lease expiry.
    #[error("renew_after_ms ({renew_after_ms}) must not exceed expires_in_ms ({expires_in_ms})")]
    RenewAfterExceedsExpiry {
        /// Rejected renewal delay.
        renew_after_ms: u64,
        /// Advertised remaining lease lifetime.
        expires_in_ms: u64,
    },
}

fn validate_attempt_timing(attempt_expires_in_ms: u64) -> Result<(), ResultValidationError> {
    if (1..=ATTEMPT_TTL_MS).contains(&attempt_expires_in_ms) {
        Ok(())
    } else {
        Err(ResultValidationError::AttemptExpiresInOutOfRange {
            actual: attempt_expires_in_ms,
            maximum: ATTEMPT_TTL_MS,
        })
    }
}

fn validate_lease_timing(
    renew_after_ms: u64,
    expires_in_ms: u64,
) -> Result<(), ResultValidationError> {
    if !(1..=LEASE_TTL_MS).contains(&expires_in_ms) {
        return Err(ResultValidationError::LeaseExpiresInOutOfRange {
            actual: expires_in_ms,
            maximum: LEASE_TTL_MS,
        });
    }
    if renew_after_ms > RENEW_TARGET_MS {
        return Err(ResultValidationError::RenewAfterExceedsTarget {
            actual: renew_after_ms,
            maximum: RENEW_TARGET_MS,
        });
    }
    if renew_after_ms > expires_in_ms {
        return Err(ResultValidationError::RenewAfterExceedsExpiry {
            renew_after_ms,
            expires_in_ms,
        });
    }
    Ok(())
}

const fn validate_live_publication_state(
    publication_state: PublicationState,
) -> Result<(), ResultValidationError> {
    match publication_state {
        PublicationState::WaitingForPublisher | PublicationState::InstanceMissing => {
            Err(ResultValidationError::InactivePublicationState {
                state: publication_state,
            })
        }
        PublicationState::CheckingEndpoint
        | PublicationState::EndpointUnhealthy
        | PublicationState::Ready
        | PublicationState::RoutePaused => Ok(()),
    }
}

fn deserialize_omitted_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn deserialize_raw_arguments<T, E>(value: &serde_json::value::RawValue) -> Result<T, E>
where
    T: DeserializeOwned,
    E: serde::de::Error,
{
    serde_json::from_str(value.get()).map_err(E::custom)
}

/// Arguments for `begin_acquisition`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginAcquisitionArguments {
    /// Daemon-observed ambient instance carried as an identity fence.
    pub expected_project_instance_id: ProjectInstanceId,
    /// Absolute routing hint independently resolved by the daemon.
    pub project_locator: AbsolutePath,
    /// Exact declared published service name.
    pub service_name: ServiceName,
    /// Explicit compare-and-swap replacement of a terminal attempt.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_omitted_option"
    )]
    pub replace_terminal_attempt_handle: Option<AcquisitionAttemptHandle>,
}

/// Result of `begin_acquisition`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BeginAcquisitionResult {
    /// Current server-issued attempt.
    acquisition_attempt_handle: AcquisitionAttemptHandle,
    /// Independently resolved project-instance identity.
    expected_project_instance_id: ProjectInstanceId,
    /// Exact primary semantic origin the caller must install.
    origin: SemanticOrigin,
    /// Remaining attempt lifetime.
    attempt_expires_in_ms: u64,
    /// Current attempt execution state.
    attempt_state: AttemptState,
}

impl BeginAcquisitionResult {
    /// Construct a result with a live version-1 acquisition attempt lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`ResultValidationError`] when the remaining attempt lifetime is
    /// outside the version-1 bound.
    pub fn new(
        acquisition_attempt_handle: AcquisitionAttemptHandle,
        expected_project_instance_id: ProjectInstanceId,
        origin: SemanticOrigin,
        attempt_expires_in_ms: u64,
        attempt_state: AttemptState,
    ) -> Result<Self, ResultValidationError> {
        validate_attempt_timing(attempt_expires_in_ms)?;
        Ok(Self {
            acquisition_attempt_handle,
            expected_project_instance_id,
            origin,
            attempt_expires_in_ms,
            attempt_state,
        })
    }

    /// Current server-issued attempt.
    #[must_use]
    pub const fn acquisition_attempt_handle(&self) -> &AcquisitionAttemptHandle {
        &self.acquisition_attempt_handle
    }

    /// Independently resolved project-instance identity.
    #[must_use]
    pub const fn expected_project_instance_id(&self) -> ProjectInstanceId {
        self.expected_project_instance_id
    }

    /// Exact primary semantic origin the caller must install.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }

    /// Remaining attempt lifetime.
    #[must_use]
    pub const fn attempt_expires_in_ms(&self) -> u64 {
        self.attempt_expires_in_ms
    }

    /// Current attempt execution state.
    #[must_use]
    pub const fn attempt_state(&self) -> AttemptState {
        self.attempt_state
    }
}

impl<'de> Deserialize<'de> for BeginAcquisitionResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            acquisition_attempt_handle: AcquisitionAttemptHandle,
            expected_project_instance_id: ProjectInstanceId,
            origin: SemanticOrigin,
            attempt_expires_in_ms: u64,
            attempt_state: AttemptState,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(
            raw.acquisition_attempt_handle,
            raw.expected_project_instance_id,
            raw.origin,
            raw.attempt_expires_in_ms,
            raw.attempt_state,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Arguments for `acquire`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquireArguments {
    /// Current server-issued acquisition attempt.
    pub acquisition_attempt_handle: AcquisitionAttemptHandle,
    /// Caller assertion that the exact origin was installed.
    pub acknowledged_origin: SemanticOrigin,
}

/// Result of `acquire`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcquireResult {
    /// Opaque authority for the installed lease.
    lease_handle: LeaseHandle,
    /// Publisher-private current binding revision.
    binding_revision: BindingRevision,
    /// Exact current semantic origin.
    origin: SemanticOrigin,
    /// Relative target for conservative renewal scheduling.
    renew_after_ms: u64,
    /// True remaining lease lifetime at serialization.
    expires_in_ms: u64,
    /// Privacy-safe current publication state.
    publication_state: PublicationState,
}

impl AcquireResult {
    /// Construct a successful lease acquisition with valid relative timing.
    ///
    /// # Errors
    ///
    /// Returns [`ResultValidationError`] unless this is binding revision `1`
    /// with a live publication state and valid version-1 lease timing.
    pub fn new(
        lease_handle: LeaseHandle,
        binding_revision: BindingRevision,
        origin: SemanticOrigin,
        renew_after_ms: u64,
        expires_in_ms: u64,
        publication_state: PublicationState,
    ) -> Result<Self, ResultValidationError> {
        if binding_revision.get() != 1 {
            return Err(ResultValidationError::InitialBindingRevision {
                actual: binding_revision.get(),
            });
        }
        validate_live_publication_state(publication_state)?;
        validate_lease_timing(renew_after_ms, expires_in_ms)?;
        Ok(Self {
            lease_handle,
            binding_revision,
            origin,
            renew_after_ms,
            expires_in_ms,
            publication_state,
        })
    }

    /// Opaque authority for the installed lease.
    #[must_use]
    pub const fn lease_handle(&self) -> &LeaseHandle {
        &self.lease_handle
    }

    /// Publisher-private current binding revision.
    #[must_use]
    pub const fn binding_revision(&self) -> BindingRevision {
        self.binding_revision
    }

    /// Exact current semantic origin.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }

    /// Relative target for conservative renewal scheduling.
    #[must_use]
    pub const fn renew_after_ms(&self) -> u64 {
        self.renew_after_ms
    }

    /// True remaining lease lifetime at serialization.
    #[must_use]
    pub const fn expires_in_ms(&self) -> u64 {
        self.expires_in_ms
    }

    /// Privacy-safe current publication state.
    #[must_use]
    pub const fn publication_state(&self) -> PublicationState {
        self.publication_state
    }
}

impl<'de> Deserialize<'de> for AcquireResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            lease_handle: LeaseHandle,
            binding_revision: BindingRevision,
            origin: SemanticOrigin,
            renew_after_ms: u64,
            expires_in_ms: u64,
            publication_state: PublicationState,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(
            raw.lease_handle,
            raw.binding_revision,
            raw.origin,
            raw.renew_after_ms,
            raw.expires_in_ms,
            raw.publication_state,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Arguments for `renew`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenewArguments {
    /// Exact current lease authority.
    pub lease_handle: LeaseHandle,
}

/// Result of `renew`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RenewResult {
    /// Current binding revision, unchanged by renewal.
    binding_revision: BindingRevision,
    /// Relative target for conservative renewal scheduling.
    renew_after_ms: u64,
    /// True remaining lease lifetime at serialization.
    expires_in_ms: u64,
    /// Privacy-safe current publication state.
    publication_state: PublicationState,
}

impl RenewResult {
    /// Construct a successful lease renewal with valid relative timing.
    ///
    /// # Errors
    ///
    /// Returns [`ResultValidationError`] unless the result has a live
    /// publication state and valid version-1 lease timing.
    pub fn new(
        binding_revision: BindingRevision,
        renew_after_ms: u64,
        expires_in_ms: u64,
        publication_state: PublicationState,
    ) -> Result<Self, ResultValidationError> {
        validate_live_publication_state(publication_state)?;
        validate_lease_timing(renew_after_ms, expires_in_ms)?;
        Ok(Self {
            binding_revision,
            renew_after_ms,
            expires_in_ms,
            publication_state,
        })
    }

    /// Current binding revision, unchanged by renewal.
    #[must_use]
    pub const fn binding_revision(&self) -> BindingRevision {
        self.binding_revision
    }

    /// Relative target for conservative renewal scheduling.
    #[must_use]
    pub const fn renew_after_ms(&self) -> u64 {
        self.renew_after_ms
    }

    /// True remaining lease lifetime at serialization.
    #[must_use]
    pub const fn expires_in_ms(&self) -> u64 {
        self.expires_in_ms
    }

    /// Privacy-safe current publication state.
    #[must_use]
    pub const fn publication_state(&self) -> PublicationState {
        self.publication_state
    }
}

impl<'de> Deserialize<'de> for RenewResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            binding_revision: BindingRevision,
            renew_after_ms: u64,
            expires_in_ms: u64,
            publication_state: PublicationState,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(
            raw.binding_revision,
            raw.renew_after_ms,
            raw.expires_in_ms,
            raw.publication_state,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Arguments for `begin_rebind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginRebindArguments {
    /// Exact current lease authority.
    pub lease_handle: LeaseHandle,
    /// Compare-and-swap binding revision fence.
    pub expected_binding_revision: BindingRevision,
    /// Explicit compare-and-swap replacement of a terminal attempt.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_omitted_option"
    )]
    pub replace_terminal_attempt_handle: Option<RebindAttemptHandle>,
}

/// Result of `begin_rebind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BeginRebindResult {
    /// Current server-issued rebind attempt.
    rebind_attempt_handle: RebindAttemptHandle,
    /// Exact current semantic origin the candidate must install.
    origin: SemanticOrigin,
    /// Remaining attempt lifetime.
    attempt_expires_in_ms: u64,
    /// Current attempt execution state.
    attempt_state: AttemptState,
}

impl BeginRebindResult {
    /// Construct a result with a live version-1 rebind attempt lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`ResultValidationError`] when the remaining attempt lifetime is
    /// outside the version-1 bound.
    pub fn new(
        rebind_attempt_handle: RebindAttemptHandle,
        origin: SemanticOrigin,
        attempt_expires_in_ms: u64,
        attempt_state: AttemptState,
    ) -> Result<Self, ResultValidationError> {
        validate_attempt_timing(attempt_expires_in_ms)?;
        Ok(Self {
            rebind_attempt_handle,
            origin,
            attempt_expires_in_ms,
            attempt_state,
        })
    }

    /// Current server-issued rebind attempt.
    #[must_use]
    pub const fn rebind_attempt_handle(&self) -> &RebindAttemptHandle {
        &self.rebind_attempt_handle
    }

    /// Exact current semantic origin the candidate must install.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }

    /// Remaining attempt lifetime.
    #[must_use]
    pub const fn attempt_expires_in_ms(&self) -> u64 {
        self.attempt_expires_in_ms
    }

    /// Current attempt execution state.
    #[must_use]
    pub const fn attempt_state(&self) -> AttemptState {
        self.attempt_state
    }
}

impl<'de> Deserialize<'de> for BeginRebindResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            rebind_attempt_handle: RebindAttemptHandle,
            origin: SemanticOrigin,
            attempt_expires_in_ms: u64,
            attempt_state: AttemptState,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(
            raw.rebind_attempt_handle,
            raw.origin,
            raw.attempt_expires_in_ms,
            raw.attempt_state,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Arguments for `rebind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebindArguments {
    /// Current server-issued rebind attempt.
    pub rebind_attempt_handle: RebindAttemptHandle,
    /// Caller assertion that the exact origin was installed.
    pub acknowledged_origin: SemanticOrigin,
}

/// Result of `rebind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebindResult {
    /// Opaque authority for the unchanged lease.
    lease_handle: LeaseHandle,
    /// Newly installed binding revision.
    binding_revision: BindingRevision,
    /// Exact current semantic origin.
    origin: SemanticOrigin,
    /// Relative target for conservative renewal scheduling.
    renew_after_ms: u64,
    /// True remaining lease lifetime at serialization.
    expires_in_ms: u64,
    /// Privacy-safe current publication state.
    publication_state: PublicationState,
}

impl RebindResult {
    /// Construct a successful lease rebind with valid relative timing.
    ///
    /// # Errors
    ///
    /// Returns [`ResultValidationError`] unless the result has a live
    /// publication state and valid version-1 lease timing.
    pub fn new(
        lease_handle: LeaseHandle,
        binding_revision: BindingRevision,
        origin: SemanticOrigin,
        renew_after_ms: u64,
        expires_in_ms: u64,
        publication_state: PublicationState,
    ) -> Result<Self, ResultValidationError> {
        validate_live_publication_state(publication_state)?;
        validate_lease_timing(renew_after_ms, expires_in_ms)?;
        Ok(Self {
            lease_handle,
            binding_revision,
            origin,
            renew_after_ms,
            expires_in_ms,
            publication_state,
        })
    }

    /// Opaque authority for the unchanged lease.
    #[must_use]
    pub const fn lease_handle(&self) -> &LeaseHandle {
        &self.lease_handle
    }

    /// Newly installed binding revision.
    #[must_use]
    pub const fn binding_revision(&self) -> BindingRevision {
        self.binding_revision
    }

    /// Exact current semantic origin.
    #[must_use]
    pub const fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }

    /// Relative target for conservative renewal scheduling.
    #[must_use]
    pub const fn renew_after_ms(&self) -> u64 {
        self.renew_after_ms
    }

    /// True remaining lease lifetime at serialization.
    #[must_use]
    pub const fn expires_in_ms(&self) -> u64 {
        self.expires_in_ms
    }

    /// Privacy-safe current publication state.
    #[must_use]
    pub const fn publication_state(&self) -> PublicationState {
        self.publication_state
    }
}

impl<'de> Deserialize<'de> for RebindResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            lease_handle: LeaseHandle,
            binding_revision: BindingRevision,
            origin: SemanticOrigin,
            renew_after_ms: u64,
            expires_in_ms: u64,
            publication_state: PublicationState,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(
            raw.lease_handle,
            raw.binding_revision,
            raw.origin,
            raw.renew_after_ms,
            raw.expires_in_ms,
            raw.publication_state,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Arguments for `wait_ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitReadyArguments {
    /// Exact current lease authority.
    pub lease_handle: LeaseHandle,
    /// Exact binding revision whose route authorization is awaited.
    pub expected_binding_revision: BindingRevision,
}

/// Successful result of `wait_ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitReadyResult {
    /// Exact ready binding revision.
    pub binding_revision: BindingRevision,
    /// Exact current semantic origin.
    pub origin: SemanticOrigin,
    /// A successful wait can only serialize `ready`.
    pub publication_state: ReadyState,
}

/// The only publication state accepted in a successful readiness result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadyState {
    /// The exact binding has current route authorization.
    Ready,
}

/// Arguments for `release`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseArguments {
    /// Exact current lease authority.
    pub lease_handle: LeaseHandle,
}

/// Successful result of `release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReleaseResult {
    /// Always true for a successful release.
    released: bool,
}

impl ReleaseResult {
    /// Construct the only valid success value.
    #[must_use]
    pub const fn released() -> Self {
        Self { released: true }
    }

    /// Return the fixed successful value.
    #[must_use]
    pub const fn is_released(self) -> bool {
        self.released
    }
}

impl<'de> Deserialize<'de> for ReleaseResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            released: bool,
        }
        let raw = Raw::deserialize(deserializer)?;
        if !raw.released {
            return Err(serde::de::Error::custom(
                "a successful release result must set released to true",
            ));
        }
        Ok(Self::released())
    }
}

/// One of the seven exact version-1 requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherRequest {
    /// Prepare a published-service acquisition.
    BeginAcquisition(BeginAcquisitionArguments),
    /// Install a listener capability and acquire a lease.
    Acquire(AcquireArguments),
    /// Renew a live lease.
    Renew(RenewArguments),
    /// Prepare a compare-and-swap rebind.
    BeginRebind(BeginRebindArguments),
    /// Validate and install a candidate listener capability.
    Rebind(RebindArguments),
    /// Wait for exact route authorization.
    WaitReady(WaitReadyArguments),
    /// Release a live lease.
    Release(ReleaseArguments),
}

impl PublisherRequest {
    /// Return the stable operation string.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        match self {
            Self::BeginAcquisition(_) => "begin_acquisition",
            Self::Acquire(_) => "acquire",
            Self::Renew(_) => "renew",
            Self::BeginRebind(_) => "begin_rebind",
            Self::Rebind(_) => "rebind",
            Self::WaitReady(_) => "wait_ready",
            Self::Release(_) => "release",
        }
    }

    /// Whether the first frame byte must carry exactly one listener descriptor.
    #[must_use]
    pub const fn requires_listener_descriptor(&self) -> bool {
        matches!(self, Self::Acquire(_) | Self::Rebind(_))
    }
}

/// Strict versioned request envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestEnvelope {
    /// Protocol version echoed by the caller.
    protocol_version: u32,
    /// Daemon lifetime observed through authenticated discovery.
    daemon_epoch: DaemonEpoch,
    /// Exact operation and arguments.
    request: PublisherRequest,
}

impl RequestEnvelope {
    /// Construct a version-1 request.
    #[must_use]
    pub const fn v1(daemon_epoch: DaemonEpoch, request: PublisherRequest) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            daemon_epoch,
            request,
        }
    }

    /// Exact wire version.
    #[must_use]
    pub const fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    /// Daemon lifetime named by this request.
    #[must_use]
    pub const fn daemon_epoch(&self) -> &DaemonEpoch {
        &self.daemon_epoch
    }

    /// Exact typed operation.
    #[must_use]
    pub const fn request(&self) -> &PublisherRequest {
        &self.request
    }

    /// Consume the envelope into its exact operation.
    #[must_use]
    pub fn into_request(self) -> PublisherRequest {
        self.request
    }
}

impl Serialize for RequestEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RequestEnvelope", 4)?;
        state.serialize_field("protocol_version", &self.protocol_version)?;
        state.serialize_field("daemon_epoch", &self.daemon_epoch)?;
        state.serialize_field("operation", self.request.operation())?;
        match &self.request {
            PublisherRequest::BeginAcquisition(arguments) => {
                state.serialize_field("arguments", arguments)?;
            }
            PublisherRequest::Acquire(arguments) => {
                state.serialize_field("arguments", arguments)?;
            }
            PublisherRequest::Renew(arguments) => state.serialize_field("arguments", arguments)?,
            PublisherRequest::BeginRebind(arguments) => {
                state.serialize_field("arguments", arguments)?;
            }
            PublisherRequest::Rebind(arguments) => state.serialize_field("arguments", arguments)?,
            PublisherRequest::WaitReady(arguments) => {
                state.serialize_field("arguments", arguments)?;
            }
            PublisherRequest::Release(arguments) => {
                state.serialize_field("arguments", arguments)?;
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for RequestEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            protocol_version: u32,
            daemon_epoch: DaemonEpoch,
            operation: String,
            arguments: Box<serde_json::value::RawValue>,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.protocol_version != PROTOCOL_VERSION {
            return Err(serde::de::Error::custom(format!(
                "unsupported publisher protocol version {}",
                raw.protocol_version
            )));
        }
        let request = match raw.operation.as_str() {
            "begin_acquisition" => {
                PublisherRequest::BeginAcquisition(deserialize_raw_arguments::<
                    BeginAcquisitionArguments,
                    D::Error,
                >(&raw.arguments)?)
            }
            "acquire" => PublisherRequest::Acquire(deserialize_raw_arguments::<
                AcquireArguments,
                D::Error,
            >(&raw.arguments)?),
            "renew" => PublisherRequest::Renew(deserialize_raw_arguments::<
                RenewArguments,
                D::Error,
            >(&raw.arguments)?),
            "begin_rebind" => PublisherRequest::BeginRebind(deserialize_raw_arguments::<
                BeginRebindArguments,
                D::Error,
            >(&raw.arguments)?),
            "rebind" => PublisherRequest::Rebind(deserialize_raw_arguments::<
                RebindArguments,
                D::Error,
            >(&raw.arguments)?),
            "wait_ready" => PublisherRequest::WaitReady(deserialize_raw_arguments::<
                WaitReadyArguments,
                D::Error,
            >(&raw.arguments)?),
            "release" => PublisherRequest::Release(deserialize_raw_arguments::<
                ReleaseArguments,
                D::Error,
            >(&raw.arguments)?),
            other => {
                return Err(serde::de::Error::custom(format!(
                    "unknown publisher operation `{other}`"
                )));
            }
        };
        Ok(Self {
            protocol_version: raw.protocol_version,
            daemon_epoch: raw.daemon_epoch,
            request,
        })
    }
}

/// Exact success or error payload in a publisher response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponsePayload<R> {
    /// Operation-specific success result.
    Ok(R),
    /// Structured stable error.
    Error(ProtocolError),
}

/// Strict versioned response envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseEnvelope<R> {
    /// Protocol version selected by the daemon.
    protocol_version: u32,
    /// Current daemon lifetime.
    daemon_epoch: DaemonEpoch,
    /// Success or error payload.
    payload: ResponsePayload<R>,
}

impl<R> ResponseEnvelope<R> {
    /// Construct a version-1 success response.
    pub const fn success(daemon_epoch: DaemonEpoch, result: R) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            daemon_epoch,
            payload: ResponsePayload::Ok(result),
        }
    }

    /// Construct a version-1 error response.
    pub const fn error(daemon_epoch: DaemonEpoch, error: ProtocolError) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            daemon_epoch,
            payload: ResponsePayload::Error(error),
        }
    }

    /// Convert the payload into an ordinary result.
    ///
    /// # Errors
    ///
    /// Returns the structured [`ProtocolError`] carried by an error response.
    pub fn into_result(self) -> Result<R, ProtocolError> {
        match self.payload {
            ResponsePayload::Ok(result) => Ok(result),
            ResponsePayload::Error(error) => Err(error),
        }
    }

    /// Exact wire version.
    #[must_use]
    pub const fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    /// Daemon lifetime that produced this response.
    #[must_use]
    pub const fn daemon_epoch(&self) -> &DaemonEpoch {
        &self.daemon_epoch
    }

    /// Borrow the exact success or error payload.
    #[must_use]
    pub const fn payload(&self) -> &ResponsePayload<R> {
        &self.payload
    }
}

impl<R: Serialize> Serialize for ResponseEnvelope<R> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ResponseEnvelope", 4)?;
        state.serialize_field("protocol_version", &self.protocol_version)?;
        state.serialize_field("daemon_epoch", &self.daemon_epoch)?;
        match &self.payload {
            ResponsePayload::Ok(result) => {
                state.serialize_field("status", "ok")?;
                state.serialize_field("result", result)?;
            }
            ResponsePayload::Error(error) => {
                state.serialize_field("status", "error")?;
                state.serialize_field("error", error)?;
            }
        }
        state.end()
    }
}

impl<'de, R> Deserialize<'de> for ResponseEnvelope<R>
where
    R: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            protocol_version: u32,
            daemon_epoch: DaemonEpoch,
            status: String,
            #[serde(default)]
            result: Presence<Box<serde_json::value::RawValue>>,
            #[serde(default)]
            error: Presence<ProtocolError>,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.protocol_version != PROTOCOL_VERSION {
            return Err(serde::de::Error::custom(format!(
                "unsupported publisher protocol version {}",
                raw.protocol_version
            )));
        }
        let payload = match (raw.status.as_str(), raw.result, raw.error) {
            ("ok", Presence::Present(result), Presence::Missing) => {
                if result.get().trim() == "null" {
                    return Err(serde::de::Error::custom(
                        "response result must be an object, not null",
                    ));
                }
                ResponsePayload::Ok(
                    serde_json::from_str(result.get()).map_err(serde::de::Error::custom)?,
                )
            }
            ("error", Presence::Missing, Presence::Present(error)) => ResponsePayload::Error(error),
            _ => {
                return Err(serde::de::Error::custom(
                    "response status must select exactly one result or error object",
                ));
            }
        };
        Ok(Self {
            protocol_version: raw.protocol_version,
            daemon_epoch: raw.daemon_epoch,
            payload,
        })
    }
}

#[derive(Default)]
enum Presence<T> {
    #[default]
    Missing,
    Present(T),
}

impl<'de, T> Deserialize<'de> for Presence<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

impl fmt::Display for PublicationState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::WaitingForPublisher => "waiting_for_publisher",
            Self::CheckingEndpoint => "checking_endpoint",
            Self::EndpointUnhealthy => "endpoint_unhealthy",
            Self::Ready => "ready",
            Self::RoutePaused => "route_paused",
            Self::InstanceMissing => "instance_missing",
        };
        formatter.write_str(value)
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    use super::*;

    fn epoch() -> DaemonEpoch {
        DaemonEpoch::from_bytes([1; 16])
    }

    fn acquisition_handle() -> AcquisitionAttemptHandle {
        AcquisitionAttemptHandle::from_repeated_byte(2, 32)
    }

    fn lease_handle() -> LeaseHandle {
        LeaseHandle::from_repeated_byte(3, 32)
    }

    fn rebind_handle() -> RebindAttemptHandle {
        RebindAttemptHandle::from_repeated_byte(4, 32)
    }

    fn project_instance_id() -> Result<ProjectInstanceId, ScalarError> {
        ProjectInstanceId::parse("550e8400-e29b-41d4-a716-446655440000")
    }

    fn origin() -> Result<SemanticOrigin, ScalarError> {
        SemanticOrigin::parse("https://workbench.exo.localhost")
    }

    #[test]
    fn begin_acquisition_has_the_exact_golden_shape() -> Result<(), Box<dyn std::error::Error>> {
        let request = RequestEnvelope::v1(
            epoch(),
            PublisherRequest::BeginAcquisition(BeginAcquisitionArguments {
                expected_project_instance_id: ProjectInstanceId::parse(
                    "550e8400-e29b-41d4-a716-446655440000",
                )?,
                project_locator: AbsolutePath::parse("/work/project")?,
                service_name: ServiceName::parse("workbench")?,
                replace_terminal_attempt_handle: None,
            }),
        );
        assert_eq!(
            serde_json::to_value(request)?,
            json!({
                "protocol_version": 1,
                "daemon_epoch": URL_SAFE_NO_PAD.encode([1; 16]),
                "operation": "begin_acquisition",
                "arguments": {
                    "expected_project_instance_id": "550e8400-e29b-41d4-a716-446655440000",
                    "project_locator": "/work/project",
                    "service_name": "workbench"
                }
            })
        );
        Ok(())
    }

    #[test]
    fn request_round_trip_preserves_exact_operation() -> Result<(), Box<dyn std::error::Error>> {
        let request = RequestEnvelope::v1(
            epoch(),
            PublisherRequest::Acquire(AcquireArguments {
                acquisition_attempt_handle: acquisition_handle(),
                acknowledged_origin: SemanticOrigin::parse("https://workbench.exo.localhost")?,
            }),
        );
        let bytes = serde_json::to_vec(&request)?;
        assert_eq!(serde_json::from_slice::<RequestEnvelope>(&bytes)?, request);
        Ok(())
    }

    #[test]
    fn request_and_argument_unknown_fields_are_rejected() {
        let handle = acquisition_handle();
        let epoch = epoch();
        let envelope_extra = json!({
            "protocol_version": 1,
            "daemon_epoch": epoch.expose_secret(),
            "operation": "acquire",
            "arguments": {
                "acquisition_attempt_handle": handle.expose_secret(),
                "acknowledged_origin": "https://workbench.exo.localhost"
            },
            "extra": true
        });
        assert!(serde_json::from_value::<RequestEnvelope>(envelope_extra).is_err());

        let argument_extra = json!({
            "protocol_version": 1,
            "daemon_epoch": epoch.expose_secret(),
            "operation": "acquire",
            "arguments": {
                "acquisition_attempt_handle": handle.expose_secret(),
                "acknowledged_origin": "https://workbench.exo.localhost",
                "port": 4000
            }
        });
        assert!(serde_json::from_value::<RequestEnvelope>(argument_extra).is_err());
    }

    #[test]
    fn duplicate_argument_fields_and_explicit_null_options_are_rejected() {
        let epoch = epoch();
        let handle = acquisition_handle();
        let duplicate = format!(
            r#"{{
                "protocol_version":1,
                "daemon_epoch":"{}",
                "operation":"acquire",
                "arguments":{{
                    "acquisition_attempt_handle":"{}",
                    "acknowledged_origin":"https://workbench.exo.localhost",
                    "acknowledged_origin":"https://other.exo.localhost"
                }}
            }}"#,
            epoch.expose_secret(),
            handle.expose_secret(),
        );
        assert!(serde_json::from_str::<RequestEnvelope>(&duplicate).is_err());

        let explicit_null = format!(
            r#"{{
                "protocol_version":1,
                "daemon_epoch":"{}",
                "operation":"begin_acquisition",
                "arguments":{{
                    "expected_project_instance_id":"550e8400-e29b-41d4-a716-446655440000",
                    "project_locator":"/work/project",
                    "service_name":"workbench",
                    "replace_terminal_attempt_handle":null
                }}
            }}"#,
            epoch.expose_secret(),
        );
        assert!(serde_json::from_str::<RequestEnvelope>(&explicit_null).is_err());
    }

    #[test]
    fn success_and_error_response_shapes_are_exclusive() -> Result<(), Box<dyn std::error::Error>> {
        let success = ResponseEnvelope::success(
            epoch(),
            RenewResult::new(
                BindingRevision::new(1)?,
                10_000,
                30_000,
                PublicationState::CheckingEndpoint,
            )?,
        );
        let bytes = serde_json::to_vec(&success)?;
        assert_eq!(
            serde_json::from_slice::<ResponseEnvelope<RenewResult>>(&bytes)?,
            success
        );

        let invalid = json!({
            "protocol_version": 1,
            "daemon_epoch": epoch().expose_secret(),
            "status": "ok",
            "result": {
                "binding_revision": 1,
                "renew_after_ms": 10000,
                "expires_in_ms": 30000,
                "publication_state": "checking_endpoint"
            },
            "error": {
                "code": "internal",
                "message": "no",
                "retry": "after_external_change"
            }
        });
        assert!(serde_json::from_value::<ResponseEnvelope<RenewResult>>(invalid).is_err());
        Ok(())
    }

    #[test]
    fn response_result_duplicates_and_explicit_null_members_are_rejected() {
        let epoch = epoch();
        let duplicate = format!(
            r#"{{
                "protocol_version":1,
                "daemon_epoch":"{}",
                "status":"ok",
                "result":{{
                    "binding_revision":1,
                    "renew_after_ms":10000,
                    "renew_after_ms":9000,
                    "expires_in_ms":30000,
                    "publication_state":"checking_endpoint"
                }}
            }}"#,
            epoch.expose_secret(),
        );
        assert!(serde_json::from_str::<ResponseEnvelope<RenewResult>>(&duplicate).is_err());

        for wire in [
            format!(
                r#"{{"protocol_version":1,"daemon_epoch":"{}","status":"ok","result":null}}"#,
                epoch.expose_secret(),
            ),
            format!(
                r#"{{"protocol_version":1,"daemon_epoch":"{}","status":"ok","result":{{"released":true}},"error":null}}"#,
                epoch.expose_secret(),
            ),
            format!(
                r#"{{"protocol_version":1,"daemon_epoch":"{}","status":"error","result":null,"error":{{"code":"internal","message":"failed","retry":"after_external_change"}}}}"#,
                epoch.expose_secret(),
            ),
        ] {
            assert!(serde_json::from_str::<ResponseEnvelope<ReleaseResult>>(&wire).is_err());
        }
    }

    #[test]
    fn result_objects_reject_unknown_fields_and_invalid_constants() {
        let extra = json!({
            "binding_revision": 1,
            "renew_after_ms": 10000,
            "expires_in_ms": 30000,
            "publication_state": "checking_endpoint",
            "pid": 42
        });
        assert!(serde_json::from_value::<RenewResult>(extra).is_err());
        assert!(serde_json::from_value::<ReleaseResult>(json!({"released": false})).is_err());

        let info = json!({
            "protocol_version": 1,
            "daemon_epoch": epoch().expose_secret(),
            "publisher_socket": "/tmp/locald-publisher.sock",
            "preparation_timeout_ms": 60000,
            "attempt_ttl_ms": 15000,
            "lease_ttl_ms": 30000,
            "renew_target_ms": 9999,
            "wait_timeout_ms": 30000,
            "frame_timeout_ms": 5000
        });
        assert!(serde_json::from_value::<PublishedEndpointProtocolInfo>(info).is_err());
    }

    #[test]
    fn attempt_result_constructors_accept_exact_timing_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        for attempt_expires_in_ms in [1, ATTEMPT_TTL_MS] {
            let acquisition = BeginAcquisitionResult::new(
                acquisition_handle(),
                project_instance_id()?,
                origin()?,
                attempt_expires_in_ms,
                AttemptState::Pending,
            )?;
            assert_eq!(acquisition.attempt_expires_in_ms(), attempt_expires_in_ms);
            assert_eq!(
                acquisition.expected_project_instance_id(),
                project_instance_id()?
            );

            let rebind = BeginRebindResult::new(
                rebind_handle(),
                origin()?,
                attempt_expires_in_ms,
                AttemptState::Terminal,
            )?;
            assert_eq!(rebind.attempt_expires_in_ms(), attempt_expires_in_ms);
        }
        Ok(())
    }

    #[test]
    fn attempt_results_reject_expired_and_overlong_values() -> Result<(), Box<dyn std::error::Error>>
    {
        for invalid in [0, ATTEMPT_TTL_MS + 1] {
            assert_eq!(
                BeginAcquisitionResult::new(
                    acquisition_handle(),
                    project_instance_id()?,
                    origin()?,
                    invalid,
                    AttemptState::Pending,
                ),
                Err(ResultValidationError::AttemptExpiresInOutOfRange {
                    actual: invalid,
                    maximum: ATTEMPT_TTL_MS,
                })
            );
            assert!(
                serde_json::from_value::<BeginAcquisitionResult>(json!({
                    "acquisition_attempt_handle": acquisition_handle().expose_secret(),
                    "expected_project_instance_id": project_instance_id()?.to_string(),
                    "origin": origin()?.as_str(),
                    "attempt_expires_in_ms": invalid,
                    "attempt_state": "pending"
                }))
                .is_err()
            );
            assert!(
                serde_json::from_value::<BeginRebindResult>(json!({
                    "rebind_attempt_handle": rebind_handle().expose_secret(),
                    "origin": origin()?.as_str(),
                    "attempt_expires_in_ms": invalid,
                    "attempt_state": "pending"
                }))
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn lease_result_constructors_accept_exact_timing_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let acquire = AcquireResult::new(
            lease_handle(),
            BindingRevision::new(1)?,
            origin()?,
            0,
            1,
            PublicationState::CheckingEndpoint,
        )?;
        assert_eq!(acquire.renew_after_ms(), 0);
        assert_eq!(acquire.expires_in_ms(), 1);

        let renew = RenewResult::new(
            BindingRevision::new(1)?,
            RENEW_TARGET_MS,
            LEASE_TTL_MS,
            PublicationState::Ready,
        )?;
        assert_eq!(renew.renew_after_ms(), RENEW_TARGET_MS);
        assert_eq!(renew.expires_in_ms(), LEASE_TTL_MS);

        let rebind = RebindResult::new(
            lease_handle(),
            BindingRevision::new(2)?,
            origin()?,
            1,
            1,
            PublicationState::EndpointUnhealthy,
        )?;
        assert_eq!(rebind.renew_after_ms(), rebind.expires_in_ms());
        Ok(())
    }

    #[test]
    fn lease_result_constructors_report_each_timing_violation()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            RenewResult::new(BindingRevision::new(1)?, 0, 0, PublicationState::Ready,),
            Err(ResultValidationError::LeaseExpiresInOutOfRange {
                actual: 0,
                maximum: LEASE_TTL_MS,
            })
        );
        assert_eq!(
            RenewResult::new(
                BindingRevision::new(1)?,
                0,
                LEASE_TTL_MS + 1,
                PublicationState::Ready,
            ),
            Err(ResultValidationError::LeaseExpiresInOutOfRange {
                actual: LEASE_TTL_MS + 1,
                maximum: LEASE_TTL_MS,
            })
        );
        assert_eq!(
            RenewResult::new(
                BindingRevision::new(1)?,
                RENEW_TARGET_MS + 1,
                LEASE_TTL_MS,
                PublicationState::Ready,
            ),
            Err(ResultValidationError::RenewAfterExceedsTarget {
                actual: RENEW_TARGET_MS + 1,
                maximum: RENEW_TARGET_MS,
            })
        );
        assert_eq!(
            RenewResult::new(BindingRevision::new(1)?, 2, 1, PublicationState::Ready,),
            Err(ResultValidationError::RenewAfterExceedsExpiry {
                renew_after_ms: 2,
                expires_in_ms: 1,
            })
        );
        Ok(())
    }

    #[test]
    fn lease_result_constructors_reject_impossible_live_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            AcquireResult::new(
                lease_handle(),
                BindingRevision::new(2)?,
                origin()?,
                RENEW_TARGET_MS,
                LEASE_TTL_MS,
                PublicationState::Ready,
            ),
            Err(ResultValidationError::InitialBindingRevision { actual: 2 })
        );

        for publication_state in [
            PublicationState::WaitingForPublisher,
            PublicationState::InstanceMissing,
        ] {
            assert_eq!(
                AcquireResult::new(
                    lease_handle(),
                    BindingRevision::new(1)?,
                    origin()?,
                    RENEW_TARGET_MS,
                    LEASE_TTL_MS,
                    publication_state,
                ),
                Err(ResultValidationError::InactivePublicationState {
                    state: publication_state,
                })
            );
            assert_eq!(
                RenewResult::new(
                    BindingRevision::new(1)?,
                    RENEW_TARGET_MS,
                    LEASE_TTL_MS,
                    publication_state,
                ),
                Err(ResultValidationError::InactivePublicationState {
                    state: publication_state,
                })
            );
            assert_eq!(
                RebindResult::new(
                    lease_handle(),
                    BindingRevision::new(2)?,
                    origin()?,
                    RENEW_TARGET_MS,
                    LEASE_TTL_MS,
                    publication_state,
                ),
                Err(ResultValidationError::InactivePublicationState {
                    state: publication_state,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn every_lease_result_rejects_invalid_wire_timing() -> Result<(), ScalarError> {
        let acquire = json!({
            "lease_handle": lease_handle().expose_secret(),
            "binding_revision": 1,
            "origin": origin()?.as_str(),
            "renew_after_ms": 2,
            "expires_in_ms": 1,
            "publication_state": "checking_endpoint"
        });
        assert!(serde_json::from_value::<AcquireResult>(acquire).is_err());

        let renew = json!({
            "binding_revision": 1,
            "renew_after_ms": RENEW_TARGET_MS + 1,
            "expires_in_ms": LEASE_TTL_MS,
            "publication_state": "ready"
        });
        assert!(serde_json::from_value::<RenewResult>(renew).is_err());

        let rebind = json!({
            "lease_handle": lease_handle().expose_secret(),
            "binding_revision": 2,
            "origin": origin()?.as_str(),
            "renew_after_ms": 0,
            "expires_in_ms": 0,
            "publication_state": "endpoint_unhealthy"
        });
        assert!(serde_json::from_value::<RebindResult>(rebind).is_err());
        Ok(())
    }

    #[test]
    fn acquire_wire_rejects_noninitial_revision_and_unleased_state() -> Result<(), ScalarError> {
        let base = json!({
            "lease_handle": lease_handle().expose_secret(),
            "binding_revision": 2,
            "origin": origin()?.as_str(),
            "renew_after_ms": RENEW_TARGET_MS,
            "expires_in_ms": LEASE_TTL_MS,
            "publication_state": "ready"
        });
        assert!(serde_json::from_value::<AcquireResult>(base).is_err());

        let unleased = json!({
            "lease_handle": lease_handle().expose_secret(),
            "binding_revision": 1,
            "origin": origin()?.as_str(),
            "renew_after_ms": RENEW_TARGET_MS,
            "expires_in_ms": LEASE_TTL_MS,
            "publication_state": "waiting_for_publisher"
        });
        assert!(serde_json::from_value::<AcquireResult>(unleased).is_err());
        Ok(())
    }

    #[test]
    fn validated_results_preserve_the_exact_json_field_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        let begin = BeginAcquisitionResult::new(
            acquisition_handle(),
            project_instance_id()?,
            origin()?,
            ATTEMPT_TTL_MS,
            AttemptState::Pending,
        )?;
        assert_eq!(
            serde_json::to_value(begin)?,
            json!({
                "acquisition_attempt_handle": acquisition_handle().expose_secret(),
                "expected_project_instance_id": project_instance_id()?.to_string(),
                "origin": origin()?.as_str(),
                "attempt_expires_in_ms": ATTEMPT_TTL_MS,
                "attempt_state": "pending"
            })
        );

        let acquire = AcquireResult::new(
            lease_handle(),
            BindingRevision::new(1)?,
            origin()?,
            RENEW_TARGET_MS,
            LEASE_TTL_MS,
            PublicationState::Ready,
        )?;
        assert_eq!(
            serde_json::to_value(acquire)?,
            json!({
                "lease_handle": lease_handle().expose_secret(),
                "binding_revision": 1,
                "origin": origin()?.as_str(),
                "renew_after_ms": RENEW_TARGET_MS,
                "expires_in_ms": LEASE_TTL_MS,
                "publication_state": "ready"
            })
        );
        Ok(())
    }

    #[test]
    fn handle_debug_output_never_contains_wire_authority() {
        let handle = lease_handle();
        assert!(!format!("{handle:?}").contains(handle.expose_secret()));
    }
}
