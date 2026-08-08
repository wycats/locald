use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::{MAX_FRAME_JSON_BYTES, PublisherRequest, RequestEnvelope, ResponseEnvelope};

const REQUEST_HEADER_BYTES: usize = 5;
const RESPONSE_HEADER_BYTES: usize = 4;

/// Descriptor prelude carried by the first request byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DescriptorPrelude {
    /// No descriptor may accompany any frame byte.
    None = 0,
    /// Exactly one descriptor must accompany the first frame byte.
    Listener = 1,
}

impl DescriptorPrelude {
    /// Parse the exact first-byte descriptor contract for a streaming receiver.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::InvalidDescriptorPrelude`] for values outside
    /// the version-1 descriptor contract.
    pub const fn parse(value: u8) -> Result<Self, FrameError> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Listener),
            other => Err(FrameError::InvalidDescriptorPrelude(other)),
        }
    }

    const fn for_request(request: &PublisherRequest) -> Self {
        if request.requires_listener_descriptor() {
            Self::Listener
        } else {
            Self::None
        }
    }
}

/// An encoded request plus its exact ancillary-descriptor contract.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedRequestFrame {
    bytes: Vec<u8>,
    descriptor: DescriptorPrelude,
}

impl EncodedRequestFrame {
    /// Return the complete request bytes, including the descriptor prelude.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume the frame into its bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Return the exact ancillary-descriptor requirement.
    #[must_use]
    pub const fn descriptor(&self) -> DescriptorPrelude {
        self.descriptor
    }
}

impl fmt::Debug for EncodedRequestFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncodedRequestFrame")
            .field(
                "bytes",
                &format_args!("<redacted; {} bytes>", self.bytes.len()),
            )
            .field("descriptor", &self.descriptor)
            .finish()
    }
}

/// Structural or JSON failure while encoding or decoding one bounded frame.
#[derive(Debug, Error)]
pub enum FrameError {
    /// The JSON body exceeds the version-1 bound.
    #[error("publisher JSON body exceeds {MAX_FRAME_JSON_BYTES} bytes")]
    BodyTooLarge,
    /// A request did not include its complete fixed header.
    #[error("publisher request frame is truncated before its header")]
    TruncatedRequestHeader,
    /// A response did not include its complete fixed header.
    #[error("publisher response frame is truncated before its header")]
    TruncatedResponseHeader,
    /// The frame length did not exactly match its declared body.
    #[error("publisher frame declared {declared} JSON bytes but carried {actual}")]
    LengthMismatch {
        /// JSON byte count encoded in the frame header.
        declared: usize,
        /// JSON bytes actually carried by the frame.
        actual: usize,
    },
    /// The descriptor prelude is not part of version 1.
    #[error("invalid descriptor prelude {0}")]
    InvalidDescriptorPrelude(u8),
    /// The operation and prelude disagree about descriptor transfer.
    #[error("descriptor prelude does not match operation `{operation}`")]
    DescriptorOperationMismatch {
        /// Operation whose descriptor contract disagreed with the prelude.
        operation: &'static str,
    },
    /// JSON serialization failed.
    #[error("failed to serialize publisher frame: {0}")]
    Serialize(#[source] serde_json::Error),
    /// JSON parsing or strict schema validation failed.
    #[error("failed to deserialize publisher frame: {0}")]
    Deserialize(#[source] serde_json::Error),
    /// The framed message selected a version this v1 codec cannot interpret.
    #[error("publisher frame selected unsupported protocol version {actual}")]
    ProtocolVersionMismatch {
        /// Unsupported version selected by the frame.
        actual: u32,
    },
}

/// Encode a strict request frame and its descriptor contract.
///
/// # Errors
///
/// Returns [`FrameError`] when JSON serialization fails or the encoded body
/// exceeds the version-1 frame bound.
pub fn encode_request_frame(request: &RequestEnvelope) -> Result<EncodedRequestFrame, FrameError> {
    let body = serde_json::to_vec(request).map_err(FrameError::Serialize)?;
    ensure_body_bound(body.len())?;
    let descriptor = DescriptorPrelude::for_request(request.request());
    let mut bytes = Vec::with_capacity(REQUEST_HEADER_BYTES + body.len());
    bytes.push(descriptor as u8);
    bytes.extend_from_slice(&body_length(body.len())?);
    bytes.extend_from_slice(&body);
    Ok(EncodedRequestFrame { bytes, descriptor })
}

/// Decode one complete request frame and enforce operation/prelude parity.
///
/// # Errors
///
/// Returns [`FrameError`] when the frame is malformed, exceeds its bound,
/// selects another protocol version, or violates the descriptor contract.
pub fn decode_request_frame(frame: &[u8]) -> Result<RequestEnvelope, FrameError> {
    if frame.len() < REQUEST_HEADER_BYTES {
        return Err(FrameError::TruncatedRequestHeader);
    }
    let descriptor = DescriptorPrelude::parse(frame[0])?;
    let body = exact_body(&frame[1..], RESPONSE_HEADER_BYTES)?;
    ensure_v1(body)?;
    let request =
        serde_json::from_slice::<RequestEnvelope>(body).map_err(FrameError::Deserialize)?;
    if descriptor != DescriptorPrelude::for_request(request.request()) {
        return Err(FrameError::DescriptorOperationMismatch {
            operation: request.request().operation(),
        });
    }
    Ok(request)
}

/// Encode one complete length-prefixed response frame.
///
/// # Errors
///
/// Returns [`FrameError`] when JSON serialization fails or the encoded body
/// exceeds the version-1 frame bound.
pub fn encode_response_frame<R: Serialize>(
    response: &ResponseEnvelope<R>,
) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(response).map_err(FrameError::Serialize)?;
    ensure_body_bound(body.len())?;
    let mut bytes = Vec::with_capacity(RESPONSE_HEADER_BYTES + body.len());
    bytes.extend_from_slice(&body_length(body.len())?);
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// Decode one complete length-prefixed response frame.
///
/// # Errors
///
/// Returns [`FrameError`] when the frame is malformed, exceeds its bound, or
/// selects another protocol version.
pub fn decode_response_frame<R: DeserializeOwned>(
    frame: &[u8],
) -> Result<ResponseEnvelope<R>, FrameError> {
    if frame.len() < RESPONSE_HEADER_BYTES {
        return Err(FrameError::TruncatedResponseHeader);
    }
    let body = exact_body(frame, RESPONSE_HEADER_BYTES)?;
    ensure_v1(body)?;
    serde_json::from_slice(body).map_err(FrameError::Deserialize)
}

fn ensure_v1(body: &[u8]) -> Result<(), FrameError> {
    #[derive(serde::Deserialize)]
    struct VersionProbe {
        protocol_version: u32,
    }
    let probe = serde_json::from_slice::<VersionProbe>(body).map_err(FrameError::Deserialize)?;
    if probe.protocol_version == crate::PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(FrameError::ProtocolVersionMismatch {
            actual: probe.protocol_version,
        })
    }
}

fn exact_body(frame: &[u8], header_bytes: usize) -> Result<&[u8], FrameError> {
    let header = frame
        .get(..header_bytes)
        .ok_or(FrameError::TruncatedResponseHeader)?;
    let length = u32::from_be_bytes(
        header
            .try_into()
            .map_err(|_| FrameError::TruncatedResponseHeader)?,
    ) as usize;
    ensure_body_bound(length)?;
    let body = &frame[header_bytes..];
    if body.len() != length {
        return Err(FrameError::LengthMismatch {
            declared: length,
            actual: body.len(),
        });
    }
    Ok(body)
}

const fn ensure_body_bound(length: usize) -> Result<(), FrameError> {
    if length > MAX_FRAME_JSON_BYTES {
        Err(FrameError::BodyTooLarge)
    } else {
        Ok(())
    }
}

fn body_length(length: usize) -> Result<[u8; 4], FrameError> {
    ensure_body_bound(length)?;
    let length = u32::try_from(length).map_err(|_| FrameError::BodyTooLarge)?;
    Ok(length.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::*;
    use crate::{
        AcquireArguments, AcquisitionAttemptHandle, DaemonEpoch, ProtocolError, ReleaseArguments,
        ReleaseResult, StableErrorCode,
    };

    fn epoch() -> DaemonEpoch {
        DaemonEpoch::from_bytes([1; 16])
    }

    #[test]
    fn request_frames_encode_descriptor_contract_and_round_trip()
    -> Result<(), Box<dyn std::error::Error>> {
        let acquire = RequestEnvelope::v1(
            epoch(),
            PublisherRequest::Acquire(AcquireArguments {
                acquisition_attempt_handle: AcquisitionAttemptHandle::parse(
                    URL_SAFE_NO_PAD.encode([2; 32]),
                )?,
                acknowledged_origin: crate::SemanticOrigin::parse(
                    "https://workbench.exo.localhost",
                )?,
            }),
        );
        let encoded = encode_request_frame(&acquire)?;
        assert_eq!(encoded.descriptor(), DescriptorPrelude::Listener);
        assert_eq!(decode_request_frame(encoded.as_bytes())?, acquire);

        let release = RequestEnvelope::v1(
            epoch(),
            PublisherRequest::Release(ReleaseArguments {
                lease_handle: crate::LeaseHandle::parse(URL_SAFE_NO_PAD.encode([3; 32]))?,
            }),
        );
        let encoded = encode_request_frame(&release)?;
        assert_eq!(encoded.descriptor(), DescriptorPrelude::None);
        assert_eq!(decode_request_frame(encoded.as_bytes())?, release);
        Ok(())
    }

    #[test]
    fn descriptor_operation_mismatch_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let release = RequestEnvelope::v1(
            epoch(),
            PublisherRequest::Release(ReleaseArguments {
                lease_handle: crate::LeaseHandle::parse(URL_SAFE_NO_PAD.encode([3; 32]))?,
            }),
        );
        let mut encoded = encode_request_frame(&release)?.into_bytes();
        encoded[0] = DescriptorPrelude::Listener as u8;
        assert!(matches!(
            decode_request_frame(&encoded),
            Err(FrameError::DescriptorOperationMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn response_frames_round_trip_and_reject_trailing_or_oversize_bodies()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = ResponseEnvelope::success(epoch(), ReleaseResult::released());
        let encoded = encode_response_frame(&response)?;
        assert_eq!(decode_response_frame::<ReleaseResult>(&encoded)?, response);
        let mut trailing = encoded;
        trailing.push(0);
        assert!(matches!(
            decode_response_frame::<ReleaseResult>(&trailing),
            Err(FrameError::LengthMismatch { .. })
        ));

        let mut oversized = vec![0_u8; RESPONSE_HEADER_BYTES];
        oversized.copy_from_slice(&((MAX_FRAME_JSON_BYTES as u32) + 1).to_be_bytes());
        assert!(matches!(
            decode_response_frame::<ReleaseResult>(&oversized),
            Err(FrameError::BodyTooLarge)
        ));
        Ok(())
    }

    #[test]
    fn structured_error_response_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let response: ResponseEnvelope<ReleaseResult> = ResponseEnvelope::error(
            epoch(),
            ProtocolError::new(
                StableErrorCode::LeaseLost,
                "lease no longer exists",
                Some("prepare a fresh acquisition".to_owned()),
            ),
        );
        let encoded = encode_response_frame(&response)?;
        assert_eq!(decode_response_frame::<ReleaseResult>(&encoded)?, response);
        Ok(())
    }

    #[test]
    fn future_versions_are_identified_before_v1_payload_decoding() {
        let request_body = br#"{"protocol_version":2,"operation":{"future":true}}"#;
        let mut request = vec![DescriptorPrelude::None as u8];
        request.extend_from_slice(&(request_body.len() as u32).to_be_bytes());
        request.extend_from_slice(request_body);
        assert!(matches!(
            decode_request_frame(&request),
            Err(FrameError::ProtocolVersionMismatch { actual: 2 })
        ));

        let response_body = br#"{"protocol_version":2,"future_payload":[1,2,3]}"#;
        let mut response = Vec::new();
        response.extend_from_slice(&(response_body.len() as u32).to_be_bytes());
        response.extend_from_slice(response_body);
        assert!(matches!(
            decode_response_frame::<ReleaseResult>(&response),
            Err(FrameError::ProtocolVersionMismatch { actual: 2 })
        ));
    }
}
