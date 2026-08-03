//! Shared wire and authority contract for the macOS privileged helper.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Current helper wire-protocol version.
pub const PROTOCOL_VERSION: u32 = 2;
/// Current helper authority-file schema version.
pub const AUTHORITY_SCHEMA_VERSION: u32 = 1;
/// Maximum accepted authority-file size.
pub const AUTHORITY_MAX_BYTES: u64 = 64 * 1024;
/// Installed helper authority path.
pub const AUTHORITY_PATH: &str = "/Library/Application Support/locald/helper-authority.json";
/// Installed helper executable path.
pub const HELPER_PATH: &str = "/Library/PrivilegedHelperTools/com.locald.helper";
/// Installed helper `LaunchDaemon` path.
pub const HELPER_PLIST_PATH: &str = "/Library/LaunchDaemons/com.locald.helper.plist";
/// Helper `LaunchDaemon` label and Mach service name.
pub const MACH_SERVICE: &str = "com.locald.helper";
/// Fixed hosts file managed by the privileged helper.
pub const HOSTS_PATH: &str = "/etc/hosts";
/// Maximum number of exact hostnames accepted in one complete-set request.
pub const HOST_SET_MAX_DOMAINS: usize = 4096;
/// Maximum aggregate hostname bytes accepted in one complete-set request.
pub const HOST_SET_MAX_BYTES: usize = 256 * 1024;

/// Helper request command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperCommand {
    /// Verify helper reachability and caller authorization without mutation.
    Probe,
    /// Bind one of locald's two privileged listener ports.
    Bind,
    /// Atomically replace locald's complete managed hosts-file section.
    SyncHosts,
}

impl HelperCommand {
    /// Stable command name used on the XPC wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Bind => "bind",
            Self::SyncHosts => "sync_hosts",
        }
    }

    /// Parse a stable command name.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "probe" => Some(Self::Probe),
            "bind" => Some(Self::Bind),
            "sync_hosts" => Some(Self::SyncHosts),
            _ => None,
        }
    }
}

/// Stable response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperStatus {
    /// The request completed successfully.
    Success,
    /// The request was rejected or failed.
    Error,
}

impl HelperStatus {
    /// Stable status string used on the XPC wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

/// Stable helper failure codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperErrorCode {
    /// The request was missing a required field or contained an invalid value.
    InvalidRequest,
    /// The client and helper protocol versions differ.
    ProtocolMismatch,
    /// The command is not part of the supported protocol.
    UnknownCommand,
    /// The requested port is outside locald's privileged networking contract.
    UnsupportedPort,
    /// The authority file could not be loaded safely.
    AuthorityUnavailable,
    /// The authority file was present but invalid.
    AuthorityInvalid,
    /// The caller did not satisfy the installed code requirement.
    AuthenticationFailed,
    /// The caller is not the configured setup owner.
    CallerUserMismatch,
    /// The configured user does not currently own the console.
    ConsoleUserMismatch,
    /// Root attempted a privileged bind.
    RootBindDenied,
    /// Binding the privileged socket failed.
    BindFailed,
    /// The complete host-set payload is malformed, noncanonical, or exceeds its bound.
    InvalidHostSet,
    /// The helper could not safely read the hosts file.
    HostsReadFailed,
    /// The helper could not atomically replace the managed hosts-file section.
    HostsWriteFailed,
    /// Root attempted to mutate the managed hosts-file section.
    RootHostsMutationDenied,
    /// An unexpected helper failure occurred.
    InternalError,
}

impl HelperErrorCode {
    /// Stable error-code string used on the XPC wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::UnknownCommand => "unknown_command",
            Self::UnsupportedPort => "unsupported_port",
            Self::AuthorityUnavailable => "authority_unavailable",
            Self::AuthorityInvalid => "authority_invalid",
            Self::AuthenticationFailed => "authentication_failed",
            Self::CallerUserMismatch => "caller_user_mismatch",
            Self::ConsoleUserMismatch => "console_user_mismatch",
            Self::RootBindDenied => "root_bind_denied",
            Self::BindFailed => "bind_failed",
            Self::InvalidHostSet => "invalid_host_set",
            Self::HostsReadFailed => "hosts_read_failed",
            Self::HostsWriteFailed => "hosts_write_failed",
            Self::RootHostsMutationDenied => "root_hosts_mutation_denied",
            Self::InternalError => "internal_error",
        }
    }

    /// Parse a stable error-code string.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "invalid_request" => Some(Self::InvalidRequest),
            "protocol_mismatch" => Some(Self::ProtocolMismatch),
            "unknown_command" => Some(Self::UnknownCommand),
            "unsupported_port" => Some(Self::UnsupportedPort),
            "authority_unavailable" => Some(Self::AuthorityUnavailable),
            "authority_invalid" => Some(Self::AuthorityInvalid),
            "authentication_failed" => Some(Self::AuthenticationFailed),
            "caller_user_mismatch" => Some(Self::CallerUserMismatch),
            "console_user_mismatch" => Some(Self::ConsoleUserMismatch),
            "root_bind_denied" => Some(Self::RootBindDenied),
            "bind_failed" => Some(Self::BindFailed),
            "invalid_host_set" => Some(Self::InvalidHostSet),
            "hosts_read_failed" => Some(Self::HostsReadFailed),
            "hosts_write_failed" => Some(Self::HostsWriteFailed),
            "root_hosts_mutation_denied" => Some(Self::RootHostsMutationDenied),
            "internal_error" => Some(Self::InternalError),
            _ => None,
        }
    }
}

/// Persistent authorization state installed by explicit administrator setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperAuthority {
    /// Authority-file schema version.
    pub schema_version: u32,
    /// Required helper wire-protocol version.
    pub protocol_version: u32,
    /// Non-root console user configured by setup.
    pub console_user_uid: u32,
    /// Canonical macOS designated requirement for the installed locald executable.
    pub designated_requirement: String,
    /// Informational canonical executable path.
    pub executable_path: PathBuf,
    /// Informational locald build version.
    pub executable_version: String,
}

impl HelperAuthority {
    /// Construct and validate current authority state.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError`] when any authority invariant is invalid.
    pub fn new(
        console_user_uid: u32,
        designated_requirement: String,
        executable_path: PathBuf,
        executable_version: String,
    ) -> Result<Self, AuthorityError> {
        let authority = Self {
            schema_version: AUTHORITY_SCHEMA_VERSION,
            protocol_version: PROTOCOL_VERSION,
            console_user_uid,
            designated_requirement,
            executable_path,
            executable_version,
        };
        authority.validate()?;
        Ok(authority)
    }

    /// Validate all authority invariants.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError`] describing the first invalid field.
    pub fn validate(&self) -> Result<(), AuthorityError> {
        if self.schema_version != AUTHORITY_SCHEMA_VERSION {
            return Err(AuthorityError::SchemaVersion(self.schema_version));
        }
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(AuthorityError::ProtocolVersion(self.protocol_version));
        }
        if self.console_user_uid == 0 {
            return Err(AuthorityError::RootConsoleUser);
        }
        if self.designated_requirement.trim().is_empty()
            || self.designated_requirement.len() > 32 * 1024
        {
            return Err(AuthorityError::InvalidRequirement);
        }
        if !self.executable_path.is_absolute() {
            return Err(AuthorityError::InvalidExecutablePath);
        }
        if self.executable_version.trim().is_empty() || self.executable_version.len() > 1024 {
            return Err(AuthorityError::InvalidExecutableVersion);
        }
        Ok(())
    }
}

/// Failures while loading or validating installed authority state.
#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    /// The authority path could not be opened or read.
    #[error("could not read helper authority: {0}")]
    Io(#[from] std::io::Error),
    /// The authority path is not a regular file.
    #[error("helper authority is not a regular file")]
    NotRegularFile,
    /// The authority file is not owned by root:wheel.
    #[error("helper authority must be owned by root:wheel (found {uid}:{gid})")]
    WrongOwner {
        /// Actual owner UID.
        uid: u32,
        /// Actual owner GID.
        gid: u32,
    },
    /// The authority file does not have mode 0600.
    #[error("helper authority must have mode 0600 (found {0:04o})")]
    WrongMode(u32),
    /// The authority file exceeds its fixed size bound.
    #[error("helper authority exceeds {AUTHORITY_MAX_BYTES} bytes")]
    TooLarge,
    /// The authority JSON is malformed.
    #[error("helper authority JSON is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
    /// The authority schema version is unsupported.
    #[error("unsupported helper authority schema version {0}")]
    SchemaVersion(u32),
    /// The authority protocol version is unsupported.
    #[error("unsupported helper protocol version {0}")]
    ProtocolVersion(u32),
    /// Root cannot be configured as the console user.
    #[error("helper authority console user must be non-root")]
    RootConsoleUser,
    /// The designated requirement is missing or unreasonably large.
    #[error("helper authority has an invalid designated requirement")]
    InvalidRequirement,
    /// The informational executable path is not absolute.
    #[error("helper authority executable path must be absolute")]
    InvalidExecutablePath,
    /// The informational executable version is missing or unreasonably large.
    #[error("helper authority has an invalid executable version")]
    InvalidExecutableVersion,
}

/// Validation failures for a complete managed host-set request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostSetValidationError {
    /// The request contains more hostnames than the protocol permits.
    #[error("host set contains {actual} domains; at most {HOST_SET_MAX_DOMAINS} are allowed")]
    TooManyDomains {
        /// Number of hostnames in the request.
        actual: usize,
    },
    /// The aggregate hostname payload exceeds its fixed protocol bound.
    #[error("host set exceeds {HOST_SET_MAX_BYTES} bytes")]
    PayloadTooLarge,
    /// A hostname is not an exact canonical DNS name.
    #[error("host set domain at index {index} is invalid: {reason}")]
    InvalidDomain {
        /// Zero-based position of the invalid hostname.
        index: usize,
        /// Stable human-readable validation detail.
        reason: &'static str,
    },
    /// Complete sets must arrive in deterministic strictly increasing order.
    #[error("host set domains must be sorted and unique (violation at index {index})")]
    NotSortedUnique {
        /// Zero-based position of the first out-of-order or duplicate hostname.
        index: usize,
    },
}

/// Return whether a port is part of locald's privileged networking contract.
pub const fn is_supported_bind_port(port: u16) -> bool {
    port == 80 || port == 443
}

/// Validate the bounded, canonical complete host set used by helper protocol v2.
///
/// The helper calls this independently even when the client already validated
/// the request. Empty sets are valid and remove locald's managed hosts section.
///
/// # Errors
///
/// Returns [`HostSetValidationError`] for oversized, noncanonical, duplicate,
/// or out-of-order hostnames.
pub fn validate_complete_host_set(domains: &[String]) -> Result<(), HostSetValidationError> {
    if domains.len() > HOST_SET_MAX_DOMAINS {
        return Err(HostSetValidationError::TooManyDomains {
            actual: domains.len(),
        });
    }

    let mut payload_bytes = 0_usize;
    let mut previous: Option<&str> = None;
    for (index, domain) in domains.iter().enumerate() {
        payload_bytes = payload_bytes
            .checked_add(domain.len())
            .ok_or(HostSetValidationError::PayloadTooLarge)?;
        if payload_bytes > HOST_SET_MAX_BYTES {
            return Err(HostSetValidationError::PayloadTooLarge);
        }

        validate_canonical_domain(domain)
            .map_err(|reason| HostSetValidationError::InvalidDomain { index, reason })?;
        if previous.is_some_and(|previous| previous >= domain.as_str()) {
            return Err(HostSetValidationError::NotSortedUnique { index });
        }
        previous = Some(domain);
    }

    Ok(())
}

fn validate_canonical_domain(domain: &str) -> Result<(), &'static str> {
    if domain.is_empty() {
        return Err("hostname is empty");
    }
    if domain.len() > 253 {
        return Err("hostname exceeds 253 bytes");
    }
    if !domain.is_ascii() {
        return Err("hostname must contain only ASCII characters");
    }
    if domain.parse::<std::net::IpAddr>().is_ok() {
        return Err("IP literals are not exact hostnames");
    }
    if domain.bytes().any(|byte| byte.is_ascii_uppercase()) || domain.ends_with('.') {
        return Err("hostname must use canonical lowercase form without a trailing dot");
    }

    for label in domain.split('.') {
        if label.is_empty() {
            return Err("hostname contains an empty DNS label");
        }
        if label.len() > 63 {
            return Err("hostname contains a DNS label longer than 63 bytes");
        }
        let bytes = label.as_bytes();
        if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
            return Err("DNS labels must start and end with a letter or digit");
        }
        if !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return Err("hostname contains an unsupported DNS character");
        }
    }

    Ok(())
}

/// Parse and validate authority JSON.
///
/// # Errors
///
/// Returns [`AuthorityError`] for oversized, malformed, or invalid authority data.
pub fn parse_authority(bytes: &[u8]) -> Result<HelperAuthority, AuthorityError> {
    if bytes.len() as u64 > AUTHORITY_MAX_BYTES {
        return Err(AuthorityError::TooLarge);
    }
    let authority: HelperAuthority = serde_json::from_slice(bytes)?;
    authority.validate()?;
    Ok(authority)
}

/// Load installed authority through a no-follow file descriptor and enforce root:wheel mode 0600.
///
/// # Errors
///
/// Returns [`AuthorityError`] when the file cannot be read safely or its contents are invalid.
#[cfg(unix)]
pub fn load_authority(path: &Path) -> Result<HelperAuthority, AuthorityError> {
    load_authority_for_owner(path, 0, 0)
}

#[cfg(unix)]
#[allow(clippy::similar_names)]
fn load_authority_for_owner(
    path: &Path,
    owner_uid: u32,
    owner_gid: u32,
) -> Result<HelperAuthority, AuthorityError> {
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(AuthorityError::NotRegularFile);
    }
    if metadata.uid() != owner_uid || metadata.gid() != owner_gid {
        return Err(AuthorityError::WrongOwner {
            uid: metadata.uid(),
            gid: metadata.gid(),
        });
    }
    let mode = metadata.mode() & 0o7777;
    if mode != 0o600 {
        return Err(AuthorityError::WrongMode(mode));
    }
    if metadata.len() > AUTHORITY_MAX_BYTES {
        return Err(AuthorityError::TooLarge);
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(AUTHORITY_MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    parse_authority(&bytes)
}

/// macOS Code Signing Services integration used by setup and the helper.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub mod code_signing {
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::CFURL;
    use security_framework::base::Error as SecurityError;
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement, SecStaticCode,
    };
    use security_framework_sys::code_signing::{SecCodeRef, SecRequirementRef};
    use std::mem::MaybeUninit;
    use std::path::Path;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecCodeCopyDesignatedRequirement(
            code: SecCodeRef,
            flags: u32,
            requirement: *mut SecRequirementRef,
        ) -> i32;
        fn SecRequirementCopyString(
            requirement: SecRequirementRef,
            flags: u32,
            text: *mut CFStringRef,
        ) -> i32;
    }

    /// Code-signing operation failure.
    #[derive(Debug, thiserror::Error)]
    pub enum CodeSigningError {
        /// The provided executable path cannot be represented as a file URL.
        #[error("invalid executable path: {0}")]
        InvalidPath(String),
        /// Security.framework rejected an operation.
        #[error("macOS code-signing validation failed: {0}")]
        Security(#[from] SecurityError),
    }

    /// Return the canonical designated requirement for a signed executable on disk.
    ///
    /// # Errors
    ///
    /// Returns [`CodeSigningError`] when the path or its signature cannot be inspected.
    pub fn designated_requirement_for_path(path: &Path) -> Result<String, CodeSigningError> {
        let url = CFURL::from_path(path, false)
            .ok_or_else(|| CodeSigningError::InvalidPath(path.display().to_string()))?;
        let code = SecStaticCode::from_path(&url, Flags::NONE)?;
        let mut requirement = MaybeUninit::uninit();
        let status = unsafe {
            SecCodeCopyDesignatedRequirement(
                code.as_concrete_TypeRef().cast(),
                Flags::NONE.bits(),
                requirement.as_mut_ptr(),
            )
        };
        check_status(status)?;
        let requirement =
            unsafe { SecRequirement::wrap_under_create_rule(requirement.assume_init()) };

        let mut text = MaybeUninit::uninit();
        let status = unsafe {
            SecRequirementCopyString(
                requirement.as_concrete_TypeRef(),
                Flags::NONE.bits(),
                text.as_mut_ptr(),
            )
        };
        check_status(status)?;
        let text = unsafe { CFString::wrap_under_create_rule(text.assume_init()) };
        Ok(text.to_string())
    }

    /// Verify that an executing XPC guest identified by audit token satisfies a requirement.
    ///
    /// # Errors
    ///
    /// Returns [`CodeSigningError`] when the guest cannot be resolved or fails the requirement.
    pub fn audit_token_satisfies_requirement(
        audit_token: &[u8; 32],
        requirement: &str,
    ) -> Result<(), CodeSigningError> {
        let token = CFData::from_buffer(audit_token);
        let mut attributes = GuestAttributes::new();
        attributes.set_audit_token(token.as_concrete_TypeRef());
        // `security-framework` 2.x publishes this method with the upstream
        // `attribues` spelling.
        let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)?;
        let requirement: SecRequirement = requirement.parse()?;
        code.check_validity(validation_flags(), &requirement)?;
        Ok(())
    }

    /// Parse a code requirement without binding it to an executable path.
    ///
    /// # Errors
    ///
    /// Returns [`CodeSigningError`] when Security.framework rejects the requirement text.
    pub fn validate_requirement(requirement: &str) -> Result<(), CodeSigningError> {
        let _: SecRequirement = requirement.parse()?;
        Ok(())
    }

    /// Verify that a signed executable on disk satisfies a requirement.
    ///
    /// # Errors
    ///
    /// Returns [`CodeSigningError`] when the path cannot be inspected or fails the requirement.
    pub fn path_satisfies_requirement(
        path: &Path,
        requirement: &str,
    ) -> Result<(), CodeSigningError> {
        let url = CFURL::from_path(path, false)
            .ok_or_else(|| CodeSigningError::InvalidPath(path.display().to_string()))?;
        let code = SecStaticCode::from_path(&url, Flags::NONE)?;
        let requirement: SecRequirement = requirement.parse()?;
        code.check_validity(
            validation_flags() | Flags::CHECK_ALL_ARCHITECTURES,
            &requirement,
        )?;
        Ok(())
    }

    fn validation_flags() -> Flags {
        Flags::STRICT_VALIDATE | Flags::NO_NETWORK_ACCESS
    }

    fn check_status(status: i32) -> Result<(), CodeSigningError> {
        if status == 0 {
            Ok(())
        } else {
            Err(SecurityError::from_code(status).into())
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::expect_used)]
mod tests {
    use super::*;

    fn authority() -> HelperAuthority {
        HelperAuthority::new(
            501,
            "identifier \"com.locald\"".to_string(),
            PathBuf::from("/usr/local/bin/locald"),
            "0.1.0".to_string(),
        )
        .expect("valid authority")
    }

    #[test]
    fn protocol_has_only_probe_bind_and_sync_hosts() {
        assert_eq!(HelperCommand::parse("probe"), Some(HelperCommand::Probe));
        assert_eq!(HelperCommand::parse("bind"), Some(HelperCommand::Bind));
        assert_eq!(
            HelperCommand::parse("sync_hosts"),
            Some(HelperCommand::SyncHosts)
        );
        assert_eq!(HelperCommand::parse("setup"), None);
        assert_eq!(HelperCommand::parse("trust"), None);
    }

    #[test]
    fn complete_host_sets_are_bounded_canonical_sorted_and_unique() {
        let valid = vec![
            "api.example.test".to_owned(),
            "workbench.example.test".to_owned(),
        ];
        assert!(validate_complete_host_set(&[]).is_ok());
        assert!(validate_complete_host_set(&valid).is_ok());

        for invalid in [
            vec!["API.example.test".to_owned()],
            vec!["*.example.test".to_owned()],
            vec!["127.0.0.1".to_owned()],
            vec!["example.test.".to_owned()],
            vec!["example.test\n127.0.0.1 injected.test".to_owned()],
        ] {
            assert!(matches!(
                validate_complete_host_set(&invalid),
                Err(HostSetValidationError::InvalidDomain { .. })
            ));
        }

        assert!(matches!(
            validate_complete_host_set(&["z.example.test".to_owned(), "a.example.test".to_owned()]),
            Err(HostSetValidationError::NotSortedUnique { index: 1 })
        ));
        assert!(matches!(
            validate_complete_host_set(&["a.example.test".to_owned(), "a.example.test".to_owned()]),
            Err(HostSetValidationError::NotSortedUnique { index: 1 })
        ));
        assert!(matches!(
            validate_complete_host_set(
                &(0..=HOST_SET_MAX_DOMAINS)
                    .map(|index| format!("host-{index:04}.example.test"))
                    .collect::<Vec<_>>()
            ),
            Err(HostSetValidationError::TooManyDomains { .. })
        ));
        assert!(matches!(
            validate_complete_host_set(
                &(0..1100)
                    .map(|index| {
                        format!(
                            "{index:04}.{}.{}.{}.{}",
                            "a".repeat(60),
                            "b".repeat(60),
                            "c".repeat(60),
                            "d".repeat(60)
                        )
                    })
                    .collect::<Vec<_>>()
            ),
            Err(HostSetValidationError::PayloadTooLarge)
        ));
    }

    #[test]
    fn host_sync_error_codes_round_trip_stably() {
        for code in [
            HelperErrorCode::RootBindDenied,
            HelperErrorCode::RootHostsMutationDenied,
            HelperErrorCode::InvalidHostSet,
            HelperErrorCode::HostsReadFailed,
            HelperErrorCode::HostsWriteFailed,
        ] {
            assert_eq!(HelperErrorCode::parse(code.as_str()), Some(code));
        }
    }

    #[test]
    fn only_ports_80_and_443_are_supported() {
        assert!(is_supported_bind_port(80));
        assert!(is_supported_bind_port(443));
        assert!(!is_supported_bind_port(0));
        assert!(!is_supported_bind_port(3000));
    }

    #[test]
    fn authority_round_trips_and_rejects_unknown_fields() {
        let bytes = serde_json::to_vec(&authority()).expect("serialize authority");
        assert_eq!(
            parse_authority(&bytes).expect("parse authority"),
            authority()
        );

        let with_unknown = br#"{
            "schema_version":1,
            "protocol_version":2,
            "console_user_uid":501,
            "designated_requirement":"identifier locald",
            "executable_path":"/usr/local/bin/locald",
            "executable_version":"0.1.0",
            "unexpected":true
        }"#;
        assert!(matches!(
            parse_authority(with_unknown),
            Err(AuthorityError::Malformed(_))
        ));
    }

    #[test]
    fn authority_rejects_wrong_versions_root_and_relative_paths() {
        let mut value = authority();
        value.protocol_version += 1;
        assert!(matches!(
            value.validate(),
            Err(AuthorityError::ProtocolVersion(_))
        ));

        let mut value = authority();
        value.console_user_uid = 0;
        assert!(matches!(
            value.validate(),
            Err(AuthorityError::RootConsoleUser)
        ));

        let mut value = authority();
        value.executable_path = PathBuf::from("locald");
        assert!(matches!(
            value.validate(),
            Err(AuthorityError::InvalidExecutablePath)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn secure_loader_checks_owner_mode_and_size() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let directory = tempfile::tempdir().expect("temporary authority directory");
        let path = directory.path().join("helper-authority.json");
        std::fs::write(&path, serde_json::to_vec(&authority()).expect("serialize"))
            .expect("write authority");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("set mode");
        let metadata = std::fs::metadata(&path).expect("authority metadata");

        assert_eq!(
            load_authority_for_owner(&path, metadata.uid(), metadata.gid()).expect("secure load"),
            authority()
        );
        assert!(matches!(
            load_authority_for_owner(&path, metadata.uid().wrapping_add(1), metadata.gid()),
            Err(AuthorityError::WrongOwner { .. })
        ));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("set loose mode");
        assert!(matches!(
            load_authority_for_owner(&path, metadata.uid(), metadata.gid()),
            Err(AuthorityError::WrongMode(0o644))
        ));

        std::fs::write(&path, vec![b'x'; (AUTHORITY_MAX_BYTES + 1) as usize])
            .expect("write oversized authority");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("restore mode");
        assert!(matches!(
            load_authority_for_owner(&path, metadata.uid(), metadata.gid()),
            Err(AuthorityError::TooLarge)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn secure_loader_rejects_symlinks_and_malformed_json() {
        use std::io::ErrorKind;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let directory = tempfile::tempdir().expect("temporary authority directory");
        let target = directory.path().join("authority-target.json");
        let link = directory.path().join("helper-authority.json");
        assert!(matches!(
            load_authority_for_owner(&link, 0, 0),
            Err(AuthorityError::Io(error)) if error.kind() == ErrorKind::NotFound
        ));

        std::fs::write(&target, b"not json").expect("write malformed authority");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .expect("set mode");
        let metadata = std::fs::metadata(&target).expect("target metadata");
        assert!(matches!(
            load_authority_for_owner(&target, metadata.uid(), metadata.gid()),
            Err(AuthorityError::Malformed(_))
        ));

        std::os::unix::fs::symlink(&target, &link).expect("create authority symlink");
        assert!(matches!(
            load_authority_for_owner(&link, metadata.uid(), metadata.gid()),
            Err(AuthorityError::Io(error)) if error.raw_os_error() == Some(libc::ELOOP)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn secure_loader_rejects_a_fifo_without_blocking() {
        use std::os::unix::fs::MetadataExt;

        let directory = tempfile::tempdir().expect("temporary authority directory");
        let path = directory.path().join("helper-authority.json");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600))
            .expect("create authority FIFO");
        let metadata = std::fs::metadata(&path).expect("FIFO metadata");

        assert!(matches!(
            load_authority_for_owner(&path, metadata.uid(), metadata.gid()),
            Err(AuthorityError::NotRegularFile)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn current_executable_matches_its_requirement_and_unrelated_code_does_not() {
        let current = std::env::current_exe().expect("current test executable");
        let requirement = code_signing::designated_requirement_for_path(&current)
            .expect("current designated requirement");
        code_signing::path_satisfies_requirement(&current, &requirement)
            .expect("current executable must match");
        assert!(
            code_signing::path_satisfies_requirement(Path::new("/bin/ls"), &requirement).is_err(),
            "unrelated executable must not satisfy the recorded requirement"
        );
        assert!(code_signing::validate_requirement("not a requirement (").is_err());
    }
}
