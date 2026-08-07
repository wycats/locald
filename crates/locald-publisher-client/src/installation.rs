use std::ffi::OsString;
use std::fs::File;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use locald_publisher_protocol::{
    INSTALLATION_RECORD_MAX_BYTES, INSTALLATION_RECORD_NAME, InstallationRecord,
    PUBLISHER_SOCKET_RELATIVE_PATH, PublishedEndpointProtocolInfo, STANDARD_COMMAND_SOCKET,
};
use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::sys::stat::{Mode, SFlag, fstat};
use nix::unistd::Uid;
use thiserror::Error;

use crate::backend::{AuthenticatedDaemonDiscovery, BackendError, UnixCommandSocketDiscovery};

/// A verified compatible locald installation and active publisher transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPublisher {
    /// Safely read setup-owned installation record.
    record: InstallationRecord,
    /// Kernel-authenticated UID of the active daemon.
    daemon_uid: u32,
    /// Exact active publisher protocol information.
    protocol_info: PublishedEndpointProtocolInfo,
}

impl InstalledPublisher {
    /// Safely read setup-owned installation record.
    #[must_use]
    pub const fn record(&self) -> &InstallationRecord {
        &self.record
    }

    /// Kernel-authenticated UID of the active daemon.
    #[must_use]
    pub const fn daemon_uid(&self) -> u32 {
        self.daemon_uid
    }

    /// Exact active publisher protocol information.
    #[must_use]
    pub const fn protocol_info(&self) -> &PublishedEndpointProtocolInfo {
        &self.protocol_info
    }

    pub(crate) const fn from_verified(
        record: InstallationRecord,
        daemon_uid: u32,
        protocol_info: PublishedEndpointProtocolInfo,
    ) -> Self {
        Self {
            record,
            daemon_uid,
            protocol_info,
        }
    }
}

/// Failure to prove positive absence or a usable compatible installation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstallationError {
    /// Ambient storage could not name one absolute standard installation root.
    #[error(
        "cannot determine an absolute standard locald data directory; select an explicit authenticated sandbox context or run `sudo locald admin setup`"
    )]
    StandardDataDirUnavailable,
    /// Standard macOS evidence cannot be enumerated without an absolute home.
    #[error(
        "cannot determine an absolute home directory for standard locald evidence; set HOME correctly or run `sudo locald admin setup`"
    )]
    StandardHomeUnavailable,
    /// The setup-owned record exists but is unsafe or incompatible.
    #[error("publisher installation record is invalid: {0}; run `sudo locald admin setup`")]
    InvalidRecord(String),
    /// Another installation surface exists without the required record.
    #[error(
        "locald installation evidence exists without a compatible publisher record; run `sudo locald admin setup`"
    )]
    MissingRecord {
        /// Concrete installation surfaces that prevented positive absence.
        evidence: Vec<InstallationEvidence>,
    },
    /// The daemon was unreachable, unauthenticated, incompatible, or inactive.
    #[error("installed locald publisher discovery failed: {0}; run `sudo locald admin setup`")]
    Discovery(BackendError),
    /// The record owner and authenticated daemon user differ.
    #[error(
        "publisher record belongs to UID {record_uid}, but daemon belongs to UID {daemon_uid}; run `sudo locald admin setup`"
    )]
    DaemonUidMismatch {
        /// Owner of the securely opened setup record.
        record_uid: u32,
        /// UID authenticated from the active daemon connection.
        daemon_uid: u32,
    },
    /// Active discovery named a publisher socket outside setup's exact data root.
    #[error("publisher socket is `{actual}`, expected `{expected}`; run `sudo locald admin setup`")]
    PublisherSocketMismatch {
        /// Only publisher socket valid beneath the selected data root.
        expected: PathBuf,
        /// Socket advertised by authenticated ordinary discovery.
        actual: PathBuf,
    },
    /// Installation evidence could not be inspected conclusively.
    #[error(
        "cannot inspect locald installation evidence at `{path}`: {message}; run `sudo locald admin setup`"
    )]
    EvidenceInspection {
        /// Evidence path whose state could not be classified.
        path: PathBuf,
        /// Redaction-safe inspection failure.
        message: String,
    },
}

/// One concrete surface that prevents a positive-absence conclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallationEvidence {
    /// A known installation path exists, including a symlink or inaccessible occupant.
    Path(PathBuf),
    /// An executable named `locald` was found through the caller's trusted PATH.
    TrustedPathExecutable(PathBuf),
}

#[derive(Debug, Clone)]
struct InstallationProbeContext {
    data_dir: PathBuf,
    trusted_path: Vec<PathBuf>,
    home: Option<PathBuf>,
    command_socket_evidence: PathBuf,
    additional_evidence: Vec<PathBuf>,
    include_system_evidence: bool,
}

impl InstallationProbeContext {
    fn explicit_standard_record(
        data_dir: PathBuf,
        trusted_path: Vec<PathBuf>,
        home: Option<PathBuf>,
    ) -> Self {
        Self {
            data_dir,
            trusted_path,
            home,
            command_socket_evidence: PathBuf::from(STANDARD_COMMAND_SOCKET),
            additional_evidence: Vec::new(),
            include_system_evidence: false,
        }
    }

    fn standard() -> Result<Self, InstallationError> {
        let trusted_path = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::standard_from(
            locald_core::storage::standard_data_dir(),
            trusted_path,
            home,
        )
    }

    fn standard_from(
        data_dir: Option<PathBuf>,
        trusted_path: Vec<PathBuf>,
        home: Option<PathBuf>,
    ) -> Result<Self, InstallationError> {
        let data_dir = data_dir.ok_or(InstallationError::StandardDataDirUnavailable)?;
        let home = select_standard_home(home, cfg!(target_os = "macos"))?;
        let mut context = Self::explicit_standard_record(data_dir, trusted_path, home);
        context.include_system_evidence = true;
        Ok(context)
    }

    #[cfg(test)]
    fn hermetic(data_dir: PathBuf, trusted_path: Vec<PathBuf>, home: Option<PathBuf>) -> Self {
        Self {
            command_socket_evidence: data_dir.join("command.sock"),
            data_dir,
            trusted_path,
            home,
            additional_evidence: Vec::new(),
            include_system_evidence: false,
        }
    }
}

fn select_standard_home(
    home: Option<PathBuf>,
    required: bool,
) -> Result<Option<PathBuf>, InstallationError> {
    let home = home.filter(|path| path.is_absolute());
    if required && home.is_none() {
        Err(InstallationError::StandardHomeUnavailable)
    } else {
        Ok(home)
    }
}

/// Probe the standard installation and its active authenticated publisher API.
///
/// `Ok(None)` is the sole direct-fallback authority and is returned only when
/// every applicable installation surface is absent. Any evidence, malformed
/// state, unreachable daemon, or inactive/incompatible protocol is an error.
///
/// # Errors
///
/// Returns [`InstallationError`] unless the standard installation is either
/// positively absent or fully verified through authenticated discovery.
pub fn probe_installation() -> Result<Option<InstalledPublisher>, InstallationError> {
    let context = InstallationProbeContext::standard()?;
    probe_installation_with(&context, &UnixCommandSocketDiscovery)
}

fn probe_installation_with(
    context: &InstallationProbeContext,
    discovery: &dyn AuthenticatedDaemonDiscovery,
) -> Result<Option<InstalledPublisher>, InstallationError> {
    probe_installation_stably(context, discovery, || {})
}

fn probe_installation_stably<F>(
    context: &InstallationProbeContext,
    discovery: &dyn AuthenticatedDaemonDiscovery,
    after_first_absence: F,
) -> Result<Option<InstalledPublisher>, InstallationError>
where
    F: FnOnce(),
{
    if let Some(record) = read_record(&context.data_dir)? {
        return verify_installed(context, discovery, record).map(Some);
    }
    after_first_absence();
    let evidence = collect_evidence(context)?;
    if let Some(record) = read_record(&context.data_dir)? {
        return verify_installed(context, discovery, record).map(Some);
    }
    if evidence.is_empty() {
        // This final absent-record observation is the probe's linearization
        // point. Activation must publish the valid record before creating any
        // listed evidence; teardown must remove it only after all listed
        // evidence is gone. PR4 activates discovery only with that setup-owned
        // ordering in place and covered by writer/teardown tests.
        Ok(None)
    } else {
        Err(InstallationError::MissingRecord { evidence })
    }
}

fn verify_installed(
    context: &InstallationProbeContext,
    discovery: &dyn AuthenticatedDaemonDiscovery,
    (record, record_uid): (InstallationRecord, u32),
) -> Result<InstalledPublisher, InstallationError> {
    let discovered = discovery
        .protocol_info(record.command_socket())
        .map_err(InstallationError::Discovery)?;
    if discovered.peer_uid != record_uid {
        return Err(InstallationError::DaemonUidMismatch {
            record_uid,
            daemon_uid: discovered.peer_uid,
        });
    }
    let expected_publisher_socket = context.data_dir.join(PUBLISHER_SOCKET_RELATIVE_PATH);
    if discovered.value.publisher_socket().as_path() != expected_publisher_socket {
        return Err(InstallationError::PublisherSocketMismatch {
            expected: expected_publisher_socket,
            actual: discovered.value.publisher_socket().to_path_buf(),
        });
    }
    Ok(InstalledPublisher::from_verified(
        record,
        discovered.peer_uid,
        discovered.value,
    ))
}

fn read_record(data_dir: &Path) -> Result<Option<(InstallationRecord, u32)>, InstallationError> {
    if !data_dir.is_absolute() {
        return Err(InstallationError::InvalidRecord(
            "locald data directory is not absolute".to_owned(),
        ));
    }
    let directory = match nix::fcntl::open(
        data_dir,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => {
            return Err(InstallationError::InvalidRecord(format!(
                "cannot safely open record directory: {error}"
            )));
        }
    };
    let directory_stat = fstat(&directory).map_err(|error| {
        InstallationError::InvalidRecord(format!("cannot inspect record directory: {error}"))
    })?;
    let directory_kind = SFlag::from_bits_truncate(directory_stat.st_mode);
    if !directory_kind.contains(SFlag::S_IFDIR) {
        return Err(InstallationError::InvalidRecord(
            "record parent is not a real directory".to_owned(),
        ));
    }

    // Check record presence relative to the no-follow directory descriptor
    // before requiring setup's final 0700 parent mode. A pre-existing 0755
    // locald data directory with no installation record remains genuine
    // absence unless another installation surface exists.
    let descriptor = match nix::fcntl::openat(
        &directory,
        INSTALLATION_RECORD_NAME,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => {
            return Err(InstallationError::InvalidRecord(format!(
                "cannot safely open installation record: {error}"
            )));
        }
    };

    validate_owner_mode(
        directory_stat.st_uid,
        directory_stat.st_mode,
        0o700,
        "record directory",
    )?;
    let stat = fstat(&descriptor).map_err(|error| {
        InstallationError::InvalidRecord(format!("cannot inspect installation record: {error}"))
    })?;
    let kind = SFlag::from_bits_truncate(stat.st_mode);
    if !kind.contains(SFlag::S_IFREG) {
        return Err(InstallationError::InvalidRecord(
            "installation record is not a regular non-symlink file".to_owned(),
        ));
    }
    validate_owner_mode(stat.st_uid, stat.st_mode, 0o600, "installation record")?;
    if stat.st_size < 0 || stat.st_size as usize > INSTALLATION_RECORD_MAX_BYTES {
        return Err(InstallationError::InvalidRecord(format!(
            "installation record exceeds {INSTALLATION_RECORD_MAX_BYTES} bytes"
        )));
    }

    let mut file = File::from(descriptor);
    let mut bytes = Vec::new();
    file.by_ref()
        .take((INSTALLATION_RECORD_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            InstallationError::InvalidRecord(format!("cannot read installation record: {error}"))
        })?;
    if bytes.len() > INSTALLATION_RECORD_MAX_BYTES {
        return Err(InstallationError::InvalidRecord(format!(
            "installation record exceeds {INSTALLATION_RECORD_MAX_BYTES} bytes"
        )));
    }
    let record = serde_json::from_slice::<InstallationRecord>(&bytes).map_err(|error| {
        InstallationError::InvalidRecord(format!("malformed installation record: {error}"))
    })?;
    record.validate().map_err(|error| {
        InstallationError::InvalidRecord(format!("incompatible installation record: {error}"))
    })?;
    Ok(Some((record, stat.st_uid)))
}

fn validate_owner_mode(
    owner_uid: u32,
    raw_mode: libc::mode_t,
    expected_mode: libc::mode_t,
    label: &str,
) -> Result<(), InstallationError> {
    let current_uid = Uid::effective().as_raw();
    if owner_uid != current_uid {
        return Err(InstallationError::InvalidRecord(format!(
            "{label} is owned by UID {owner_uid}, expected effective UID {current_uid}"
        )));
    }
    let actual_mode = raw_mode & 0o777;
    if actual_mode != expected_mode {
        return Err(InstallationError::InvalidRecord(format!(
            "{label} has mode {actual_mode:#05o}, expected {expected_mode:#05o}"
        )));
    }
    Ok(())
}

fn collect_evidence(
    context: &InstallationProbeContext,
) -> Result<Vec<InstallationEvidence>, InstallationError> {
    let mut paths = vec![
        context.command_socket_evidence.clone(),
        context.data_dir.join("locald-agent"),
        context.data_dir.join(PUBLISHER_SOCKET_RELATIVE_PATH),
    ];
    paths.extend(context.additional_evidence.iter().cloned());
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = &context.home {
            paths.push(home.join("Library/Application Support/locald/locald-agent"));
            paths.push(home.join("Library/LaunchAgents/com.locald.agent.plist"));
        }
        if context.include_system_evidence {
            paths.extend([
                PathBuf::from("/Library/Application Support/locald/helper-authority.json"),
                PathBuf::from("/Library/PrivilegedHelperTools/com.locald.helper"),
                PathBuf::from("/Library/LaunchDaemons/com.locald.helper.plist"),
            ]);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = &context.home;

    let mut evidence = paths
        .into_iter()
        .filter(|path| path_is_evidence(path))
        .map(InstallationEvidence::Path)
        .collect::<Vec<_>>();
    if let Some(executable) = find_trusted_path_executable(&context.trusted_path)? {
        evidence.push(InstallationEvidence::TrustedPathExecutable(executable));
    }
    Ok(evidence)
}

fn path_is_evidence(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

fn find_trusted_path_executable(
    path_entries: &[PathBuf],
) -> Result<Option<PathBuf>, InstallationError> {
    for directory in path_entries {
        let candidate = directory.join(OsString::from("locald"));
        let metadata = match std::fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(InstallationError::EvidenceInspection {
                    path: candidate,
                    message: error.to_string(),
                });
            }
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "hermetic fixture setup uses expect so probe assertions remain legible"
)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write as _;
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use locald_publisher_protocol::{AbsolutePath, DaemonEpoch};
    use tempfile::TempDir;

    use super::*;
    use crate::backend::{AuthenticatedValue, BackendErrorKind};

    #[derive(Debug)]
    struct FakeDiscovery {
        result: Result<AuthenticatedValue<PublishedEndpointProtocolInfo>, BackendError>,
    }

    impl AuthenticatedDaemonDiscovery for FakeDiscovery {
        fn protocol_info(
            &self,
            _command_socket: &AbsolutePath,
        ) -> Result<AuthenticatedValue<PublishedEndpointProtocolInfo>, BackendError> {
            self.result.clone()
        }

        fn resolve_project(
            &self,
            _command_socket: &AbsolutePath,
            _project_locator: &AbsolutePath,
        ) -> Result<AuthenticatedValue<locald_publisher_protocol::ProjectInstanceId>, BackendError>
        {
            Err(BackendError::new(BackendErrorKind::Protocol, "unused"))
        }
    }

    fn context(root: &TempDir) -> InstallationProbeContext {
        InstallationProbeContext::hermetic(
            root.path().join("data"),
            vec![root.path().join("bin")],
            Some(root.path().join("home")),
        )
    }

    fn active_info(context: &InstallationProbeContext) -> PublishedEndpointProtocolInfo {
        PublishedEndpointProtocolInfo::v1(
            DaemonEpoch::from_bytes([2; 16]),
            AbsolutePath::try_from(context.data_dir.join(PUBLISHER_SOCKET_RELATIVE_PATH))
                .expect("publisher socket"),
        )
    }

    fn discovery(context: &InstallationProbeContext) -> FakeDiscovery {
        FakeDiscovery {
            result: Ok(AuthenticatedValue {
                peer_uid: Uid::effective().as_raw(),
                value: active_info(context),
            }),
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        File::create(path)?.write_all(bytes)
    }

    fn write_record(context: &InstallationProbeContext, record_json: &str) {
        fs::create_dir_all(&context.data_dir).expect("create data directory");
        fs::set_permissions(&context.data_dir, fs::Permissions::from_mode(0o700))
            .expect("secure directory");
        let path = context.data_dir.join(INSTALLATION_RECORD_NAME);
        write_file(&path, record_json.as_bytes()).expect("write record");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("secure record");
    }

    #[test]
    fn compatible_record_and_authenticated_protocol_are_installed() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        write_record(
            &context,
            r#"{"schema_version":1,"publisher_protocol_version":1,"command_socket":"/tmp/locald.sock"}"#,
        );
        let installed = probe_installation_with(&context, &discovery(&context))
            .expect("probe")
            .expect("installed");
        assert_eq!(installed.protocol_info, active_info(&context));
    }

    #[test]
    fn ambiguous_ambient_data_root_fails_closed() {
        assert!(matches!(
            InstallationProbeContext::standard_from(None, Vec::new(), None),
            Err(InstallationError::StandardDataDirUnavailable)
        ));
    }

    #[test]
    fn standard_home_selection_is_platform_requirement_aware() {
        assert_eq!(
            select_standard_home(None, true),
            Err(InstallationError::StandardHomeUnavailable)
        );
        assert_eq!(
            select_standard_home(Some(PathBuf::from("relative-home")), true),
            Err(InstallationError::StandardHomeUnavailable)
        );
        assert_eq!(select_standard_home(None, false), Ok(None));
        let absolute = PathBuf::from("/Users/example");
        assert_eq!(
            select_standard_home(Some(absolute.clone()), true),
            Ok(Some(absolute))
        );
    }

    #[test]
    fn explicit_context_is_hermetic_standard_record_not_sandbox_selection() {
        let root = TempDir::new().expect("tempdir");
        let context = InstallationProbeContext::explicit_standard_record(
            root.path().join("data"),
            vec![root.path().join("bin")],
            Some(root.path().join("home")),
        );
        assert_eq!(
            context.command_socket_evidence,
            PathBuf::from(STANDARD_COMMAND_SOCKET)
        );
        assert!(!context.include_system_evidence);
    }

    #[test]
    fn unsafe_incompatible_or_symlinked_record_fails_closed() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        write_record(
            &context,
            r#"{"schema_version":1,"publisher_protocol_version":1,"command_socket":"/tmp/other.sock"}"#,
        );
        assert!(matches!(
            probe_installation_with(&context, &discovery(&context)),
            Err(InstallationError::InvalidRecord(_))
        ));

        fs::remove_file(context.data_dir.join(INSTALLATION_RECORD_NAME)).expect("remove record");
        let target = root.path().join("record.json");
        write_file(
            &target,
            br#"{"schema_version":1,"publisher_protocol_version":1,"command_socket":"/tmp/locald.sock"}"#,
        )
        .expect("write target");
        symlink(&target, context.data_dir.join(INSTALLATION_RECORD_NAME)).expect("symlink");
        assert!(matches!(
            probe_installation_with(&context, &discovery(&context)),
            Err(InstallationError::InvalidRecord(_))
        ));
    }

    #[test]
    fn absent_record_is_checked_before_preexisting_parent_mode() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        fs::create_dir_all(&context.data_dir).expect("create data directory");
        fs::set_permissions(&context.data_dir, fs::Permissions::from_mode(0o755))
            .expect("ordinary pre-setup mode");
        assert_eq!(
            probe_installation_with(&context, &discovery(&context)).expect("probe"),
            None
        );
    }

    #[test]
    fn positive_absence_requires_every_evidence_surface_to_be_absent() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        assert_eq!(
            probe_installation_with(&context, &discovery(&context)).expect("probe"),
            None
        );

        fs::create_dir_all(context.trusted_path.first().expect("path entry")).expect("create bin");
        let executable = context.trusted_path[0].join("locald");
        write_file(&executable, b"binary").expect("write executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make executable");
        assert!(matches!(
            probe_installation_with(&context, &discovery(&context)),
            Err(InstallationError::MissingRecord { .. })
        ));
    }

    #[test]
    fn publisher_socket_without_record_is_installation_evidence() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        fs::create_dir_all(&context.data_dir).expect("create data directory");
        let publisher_socket = context.data_dir.join(PUBLISHER_SOCKET_RELATIVE_PATH);
        fs::create_dir_all(publisher_socket.parent().expect("publisher socket parent"))
            .expect("create publisher socket parent");
        write_file(&publisher_socket, b"stale socket occupant").expect("create socket evidence");

        let error = probe_installation_with(&context, &discovery(&context))
            .expect_err("publisher socket must fail closed");
        assert!(matches!(
            error,
            InstallationError::MissingRecord { evidence }
                if evidence.contains(&InstallationEvidence::Path(publisher_socket))
        ));
    }

    #[test]
    fn installed_but_unreachable_or_inactive_never_becomes_absent() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        write_record(
            &context,
            r#"{"schema_version":1,"publisher_protocol_version":1,"command_socket":"/tmp/locald.sock"}"#,
        );
        let unavailable = FakeDiscovery {
            result: Err(BackendError::new(
                BackendErrorKind::ProtocolUnavailable,
                "publisher transport inactive",
            )),
        };
        assert!(matches!(
            probe_installation_with(&context, &unavailable),
            Err(InstallationError::Discovery(BackendError {
                kind: BackendErrorKind::ProtocolUnavailable,
                ..
            }))
        ));
    }

    #[test]
    fn atomic_record_publication_during_absence_probe_is_not_misclassified() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        let discovered = discovery(&context);
        let result = probe_installation_stably(&context, &discovered, || {
            write_record(
                &context,
                r#"{"schema_version":1,"publisher_protocol_version":1,"command_socket":"/tmp/locald.sock"}"#,
            );
        })
        .expect("probe")
        .expect("published installation");
        assert_eq!(result.protocol_info, active_info(&context));
    }

    #[test]
    fn publisher_socket_must_be_inside_the_exact_installation_data_root() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        write_record(
            &context,
            r#"{"schema_version":1,"publisher_protocol_version":1,"command_socket":"/tmp/locald.sock"}"#,
        );
        let discovery = FakeDiscovery {
            result: Ok(AuthenticatedValue {
                peer_uid: Uid::effective().as_raw(),
                value: PublishedEndpointProtocolInfo::v1(
                    DaemonEpoch::from_bytes([2; 16]),
                    AbsolutePath::parse("/tmp/other/publisher-v1.sock").expect("socket"),
                ),
            }),
        };
        assert!(matches!(
            probe_installation_with(&context, &discovery),
            Err(InstallationError::PublisherSocketMismatch { .. })
        ));
    }

    #[test]
    fn trusted_path_metadata_failures_are_not_treated_as_absence() {
        let root = TempDir::new().expect("tempdir");
        let context = context(&root);
        let bin = context.trusted_path.first().expect("path entry");
        fs::create_dir_all(bin).expect("create bin");
        symlink("locald", bin.join("locald")).expect("create self-referential symlink");
        assert!(matches!(
            probe_installation_with(&context, &discovery(&context)),
            Err(InstallationError::EvidenceInspection { .. })
        ));
    }
}
