//! Strict version-1 wire types for externally published locald services.
//!
//! This crate deliberately contains no daemon or socket implementation. It is
//! the shared, dependency-light contract used by the daemon, the supported
//! publisher client, and authenticated discovery adapters.

mod error;
mod frame;
mod scalar;
mod wire;

pub use error::{ProtocolError, ProtocolErrorValidation, RetryClass, StableErrorCode};
pub use frame::{
    DescriptorPrelude, EncodedRequestFrame, FrameError, decode_request_frame,
    decode_response_frame, encode_request_frame, encode_response_frame,
};
pub use scalar::{
    AbsolutePath, AcquisitionAttemptHandle, BindingRevision, DaemonEpoch, LeaseHandle,
    ProjectInstanceId, RebindAttemptHandle, ScalarError, SemanticOrigin, ServiceName,
};
pub use wire::{
    ATTEMPT_TTL_MS, AcquireArguments, AcquireResult, AttemptState, BeginAcquisitionArguments,
    BeginAcquisitionResult, BeginRebindArguments, BeginRebindResult, FRAME_TIMEOUT_MS,
    INSTALLATION_RECORD_MAX_BYTES, INSTALLATION_RECORD_NAME, InstallationRecord, LEASE_TTL_MS,
    MACOS_PUBLISHER_AUDIT_TOKEN_PROOF_BYTES, MAX_FRAME_JSON_BYTES, PREPARATION_TIMEOUT_MS,
    PROTOCOL_VERSION, PUBLISHER_SOCKET_RELATIVE_PATH, PublicationState,
    PublishedEndpointProtocolInfo, PublisherRequest, RENEW_TARGET_MS, ReadyState, RebindArguments,
    RebindResult, ReleaseArguments, ReleaseResult, RenewArguments, RenewResult, RequestEnvelope,
    ResponseEnvelope, ResponsePayload, ResultValidationError, STANDARD_COMMAND_SOCKET,
    WAIT_TIMEOUT_MS, WaitReadyArguments, WaitReadyResult,
};
