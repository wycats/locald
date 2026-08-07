use std::fmt;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

/// A malformed scalar in the version-1 publisher protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ScalarError {
    /// An opaque authority value was not canonical unpadded base64url.
    #[error("{kind} must be canonical unpadded base64url")]
    InvalidOpaqueEncoding {
        /// Human-readable scalar kind.
        kind: &'static str,
    },
    /// An opaque authority value decoded to the wrong amount of entropy.
    #[error("{kind} must contain {requirement}")]
    InvalidOpaqueLength {
        /// Human-readable scalar kind.
        kind: &'static str,
        /// Canonical entropy requirement.
        requirement: &'static str,
    },
    /// A project-instance ID was not a canonical lowercase hyphenated UUID.
    #[error("project instance ID must be a canonical lowercase hyphenated UUID")]
    InvalidProjectInstanceId,
    /// A protocol path was not an absolute UTF-8 filesystem path.
    #[error("{kind} must be an absolute UTF-8 filesystem path")]
    InvalidAbsolutePath {
        /// Human-readable path kind.
        kind: &'static str,
    },
    /// A service name did not follow locald's admitted service-name grammar.
    #[error("service name does not follow locald's admitted service-name grammar")]
    InvalidServiceName,
    /// A semantic origin was not a canonical serialized HTTPS origin.
    #[error(
        "semantic origin must be a canonical absolute HTTPS origin without a path, query, or fragment"
    )]
    InvalidSemanticOrigin,
    /// A binding revision was zero.
    #[error("binding revision must be at least 1")]
    InvalidBindingRevision,
    /// The setup-owned record did not name the standard command socket.
    #[error("version-1 installation record command socket must be /tmp/locald.sock")]
    InvalidInstallationCommandSocket,
    /// A version-1 timing policy did not match the canonical constants.
    #[error("version-1 protocol policy field `{field}` must be {expected}, got {actual}")]
    InvalidProtocolPolicy {
        /// Wire field containing the fixed policy value.
        field: &'static str,
        /// Canonical version-1 value.
        expected: u64,
        /// Value supplied by the input.
        actual: u64,
    },
}

fn validate_opaque(
    kind: &'static str,
    value: &str,
    length: OpaqueLength,
) -> Result<(), ScalarError> {
    if value.is_empty() || value.contains('=') {
        return Err(ScalarError::InvalidOpaqueEncoding { kind });
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ScalarError::InvalidOpaqueEncoding { kind })?;
    if URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(ScalarError::InvalidOpaqueEncoding { kind });
    }
    let valid = match length {
        OpaqueLength::Exactly(bytes) => decoded.len() == bytes,
        OpaqueLength::AtLeast(bytes) => decoded.len() >= bytes,
    };
    if !valid {
        return Err(ScalarError::InvalidOpaqueLength {
            kind,
            requirement: match length {
                OpaqueLength::Exactly(16) => "exactly 128 bits",
                OpaqueLength::AtLeast(32) => "at least 256 bits",
                _ => "the required entropy",
            },
        });
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum OpaqueLength {
    Exactly(usize),
    AtLeast(usize),
}

macro_rules! opaque_scalar {
    ($name:ident, $kind:literal, $length:expr) => {
        #[doc = concat!("A validated opaque ", $kind, " value.")]
        #[derive(Clone, PartialEq, Eq, Hash, JsonSchema)]
        #[schemars(with = "String")]
        pub struct $name(String);

        impl $name {
            /// Parse and validate the canonical wire representation.
            ///
            /// # Errors
            ///
            /// Returns [`ScalarError`] when the value is not canonical
            /// URL-safe base64 or does not carry the required entropy.
            pub fn parse(value: impl Into<String>) -> Result<Self, ScalarError> {
                let value = value.into();
                validate_opaque($kind, &value, $length)?;
                Ok(Self(value))
            }

            /// Return the exact wire representation.
            ///
            /// Callers should avoid logging this value. Authority-bearing
            /// types deliberately redact their [`Debug`](fmt::Debug) output.
            #[must_use]
            pub fn expose_secret(&self) -> &str {
                &self.0
            }

            #[cfg(test)]
            #[allow(dead_code)]
            pub(crate) fn from_repeated_byte(byte: u8, bytes: usize) -> Self {
                Self(URL_SAFE_NO_PAD.encode(vec![byte; bytes]))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }

        impl FromStr for $name {
            type Err = ScalarError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }
    };
}

opaque_scalar!(DaemonEpoch, "daemon epoch", OpaqueLength::Exactly(16));
opaque_scalar!(
    AcquisitionAttemptHandle,
    "acquisition attempt handle",
    OpaqueLength::AtLeast(32)
);
opaque_scalar!(
    RebindAttemptHandle,
    "rebind attempt handle",
    OpaqueLength::AtLeast(32)
);
opaque_scalar!(LeaseHandle, "lease handle", OpaqueLength::AtLeast(32));

impl DaemonEpoch {
    /// Construct an epoch from the exact 128 bits selected by the daemon.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }
}

/// A canonical lowercase hyphenated project-instance UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct ProjectInstanceId(Uuid);

impl ProjectInstanceId {
    /// Parse the exact wire representation.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError::InvalidProjectInstanceId`] unless `value` is a
    /// canonical lowercase hyphenated UUID.
    pub fn parse(value: &str) -> Result<Self, ScalarError> {
        let uuid = Uuid::parse_str(value).map_err(|_| ScalarError::InvalidProjectInstanceId)?;
        if uuid.hyphenated().to_string() != value {
            return Err(ScalarError::InvalidProjectInstanceId);
        }
        Ok(Self(uuid))
    }

    /// Return the UUID value.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for ProjectInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(formatter)
    }
}

impl FromStr for ProjectInstanceId {
    type Err = ScalarError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ProjectInstanceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ProjectInstanceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// An absolute UTF-8 filesystem path carried on the protocol wire.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(with = "String")]
pub struct AbsolutePath(String);

impl AbsolutePath {
    /// Validate an absolute UTF-8 path string.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError::InvalidAbsolutePath`] when the path is empty,
    /// contains a NUL byte, or is not absolute.
    pub fn parse(value: impl Into<String>) -> Result<Self, ScalarError> {
        let value = value.into();
        if value.is_empty() || value.contains('\0') || !Path::new(&value).is_absolute() {
            return Err(ScalarError::InvalidAbsolutePath { kind: "path" });
        }
        Ok(Self(value))
    }

    /// Return this path as a [`Path`].
    #[must_use]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// Return the exact UTF-8 wire value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Clone this path into an owned platform path.
    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.0)
    }
}

impl TryFrom<PathBuf> for AbsolutePath {
    type Error = ScalarError;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let value = path
            .into_os_string()
            .into_string()
            .map_err(|_| ScalarError::InvalidAbsolutePath { kind: "path" })?;
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A service name admitted by the current locald configuration grammar.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(with = "String")]
pub struct ServiceName(String);

impl ServiceName {
    /// Parse a service name using locald's interpolation-safe grammar.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError::InvalidServiceName`] when the name can escape
    /// the interpolation grammar.
    pub fn parse(value: impl Into<String>) -> Result<Self, ScalarError> {
        let value = value.into();
        if value.contains('}') {
            return Err(ScalarError::InvalidServiceName);
        }
        Ok(Self(value))
    }

    /// Return the exact configured service name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ServiceName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A canonical absolute HTTPS origin without path, query, or fragment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(with = "String")]
pub struct SemanticOrigin(String);

impl SemanticOrigin {
    /// Parse the exact canonical wire serialization.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError::InvalidSemanticOrigin`] unless the value is a
    /// canonical absolute HTTPS origin with no path, query, or fragment.
    pub fn parse(value: impl Into<String>) -> Result<Self, ScalarError> {
        let value = value.into();
        let authority = value
            .strip_prefix("https://")
            .ok_or(ScalarError::InvalidSemanticOrigin)?;
        if authority.is_empty() || authority.contains(['/', '?', '#', '@', '\\', '\r', '\n']) {
            return Err(ScalarError::InvalidSemanticOrigin);
        }
        let (host, port) = authority.rsplit_once(':').map_or_else(
            || Ok::<_, ScalarError>((authority, 443_u16)),
            |(host, port)| {
                if host.is_empty()
                    || port.is_empty()
                    || !port.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(ScalarError::InvalidSemanticOrigin);
                }
                let port = port
                    .parse::<u16>()
                    .map_err(|_| ScalarError::InvalidSemanticOrigin)?;
                if port == 0 {
                    return Err(ScalarError::InvalidSemanticOrigin);
                }
                Ok((host, port))
            },
        )?;
        validate_canonical_domain(host)?;
        let canonical = if port == 443 {
            format!("https://{host}")
        } else {
            format!("https://{host}:{port}")
        };
        if canonical != value {
            return Err(ScalarError::InvalidSemanticOrigin);
        }
        Ok(Self(value))
    }

    /// Return the exact serialized origin.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_canonical_domain(host: &str) -> Result<(), ScalarError> {
    if host.trim() != host
        || !host.is_ascii()
        || host != host.to_ascii_lowercase()
        || host.ends_with('.')
        || host.is_empty()
        || host.len() > 253
    {
        return Err(ScalarError::InvalidSemanticOrigin);
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(ScalarError::InvalidSemanticOrigin);
        }
        let bytes = label.as_bytes();
        if !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return Err(ScalarError::InvalidSemanticOrigin);
        }
    }
    Ok(())
}

impl fmt::Display for SemanticOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for SemanticOrigin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A nonzero publisher-private binding revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(with = "u64")]
pub struct BindingRevision(NonZeroU64);

impl BindingRevision {
    /// Construct a nonzero revision.
    ///
    /// # Errors
    ///
    /// Returns [`ScalarError::InvalidBindingRevision`] when `value` is zero.
    pub fn new(value: u64) -> Result<Self, ScalarError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ScalarError::InvalidBindingRevision)
    }

    /// Return the integer wire value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl<'de> Deserialize<'de> for BindingRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_values_are_canonical_and_redacted() {
        let epoch = DaemonEpoch::from_bytes([7; 16]);
        assert_eq!(format!("{epoch:?}"), "DaemonEpoch(<redacted>)");
        assert!(DaemonEpoch::parse(format!("{}=", epoch.expose_secret())).is_err());
        assert!(LeaseHandle::parse(URL_SAFE_NO_PAD.encode([3; 31])).is_err());
        assert!(LeaseHandle::parse(URL_SAFE_NO_PAD.encode([3; 32])).is_ok());
    }

    #[test]
    fn scalar_wire_forms_reject_noncanonical_values() {
        assert!(ProjectInstanceId::parse("550E8400-E29B-41D4-A716-446655440000").is_err());
        assert!(AbsolutePath::parse("relative/project").is_err());
        assert!(ServiceName::parse("bad}name").is_err());
        assert!(SemanticOrigin::parse("https://example.localhost/path").is_err());
        assert!(SemanticOrigin::parse("https://example.localhost:443").is_err());
        assert!(SemanticOrigin::parse("https://example.localhost").is_ok());
        assert!(BindingRevision::new(0).is_err());
    }
}
