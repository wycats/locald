//! Strict rendering and durable publication for locald-managed hosts-file sections.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

const START_MARKER: &str = "# BEGIN locald";
const END_MARKER: &str = "# END locald";

/// The maximum hosts-file size accepted by the durable writer.
///
/// `/etc/hosts` is normally tiny. A bound keeps a privileged locald process
/// from allocating without limit when the file is unexpectedly large.
pub const MAX_HOSTS_FILE_BYTES: usize = 1024 * 1024;

/// A canonical, sorted set of exact host names owned by locald.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostSet(BTreeSet<String>);

impl HostSet {
    /// Validate and canonicalize exact host names into a sorted set.
    ///
    /// ASCII case is canonicalized to lowercase. Wildcards, IP literals,
    /// control characters, empty labels, and labels outside DNS size limits
    /// are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`HostSetError`] when any input is not an exact DNS host name.
    pub fn try_from_strings<I, S>(domains: I) -> Result<Self, HostSetError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut canonical = BTreeSet::new();
        for domain in domains {
            let domain = domain.as_ref();
            validate_domain(domain)?;
            canonical.insert(domain.to_ascii_lowercase());
        }
        Ok(Self(canonical))
    }

    /// Iterate over canonical host names in deterministic order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// Copy the canonical host names into a deterministic vector.
    #[must_use]
    pub fn as_strings(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }

    /// Return whether the set contains no host names.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Return the number of host names in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<'a> IntoIterator for &'a HostSet {
    type Item = &'a str;
    type IntoIter = HostSetIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        HostSetIter(self.0.iter())
    }
}

/// Iterator over a [`HostSet`].
#[derive(Clone, Debug)]
pub struct HostSetIter<'a>(std::collections::btree_set::Iter<'a, String>);

impl<'a> Iterator for HostSetIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(String::as_str)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for HostSetIter<'_> {}

/// Why an input could not become part of a [`HostSet`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostSetError {
    domain: String,
    reason: &'static str,
}

impl HostSetError {
    /// Return the rejected input.
    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Return the stable validation reason.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for HostSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid locald host name {:?}: {}",
            self.domain, self.reason
        )
    }
}

impl Error for HostSetError {}

fn validate_domain(domain: &str) -> Result<(), HostSetError> {
    let reject = |reason| {
        Err(HostSetError {
            domain: domain.to_owned(),
            reason,
        })
    };

    if domain.is_empty() {
        return reject("host name is empty");
    }
    if domain.len() > 253 {
        return reject("host name exceeds 253 bytes");
    }
    if !domain.is_ascii() {
        return reject("host name must be ASCII");
    }
    if domain.parse::<std::net::IpAddr>().is_ok() {
        return reject("IP literals are not host names");
    }
    if domain.starts_with('.') || domain.ends_with('.') {
        return reject("host name must not start or end with a dot");
    }
    if domain.contains('*') {
        return reject("wildcards are not exact host names");
    }
    if domain
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.'))
    {
        return reject("host name contains an unsupported character");
    }
    for label in domain.split('.') {
        if label.is_empty() {
            return reject("host name contains an empty label");
        }
        if label.len() > 63 {
            return reject("host name contains a label longer than 63 bytes");
        }
        if label.starts_with('-') || label.ends_with('-') {
            return reject("host-name labels must not start or end with a hyphen");
        }
    }
    Ok(())
}

/// The kind of malformed locald-managed section encountered while parsing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedSectionErrorKind {
    /// More than one start marker was present.
    DuplicateStart,
    /// More than one end marker was present.
    DuplicateEnd,
    /// A start marker had no matching end marker.
    MissingEnd,
    /// An end marker had no matching start marker.
    MissingStart,
    /// The end marker appeared before the start marker.
    EndBeforeStart,
    /// A managed line was not an IPv4 loopback mapping.
    InvalidMapping,
    /// A managed mapping contained an invalid host name.
    InvalidDomain,
}

/// A strict managed-section parsing error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedSectionError {
    kind: ManagedSectionErrorKind,
    line: Option<usize>,
    domain_error: Option<HostSetError>,
}

impl ManagedSectionError {
    /// Return the stable error kind.
    #[must_use]
    pub const fn kind(&self) -> ManagedSectionErrorKind {
        self.kind
    }

    /// Return the one-based line number, when the error belongs to one line.
    #[must_use]
    pub const fn line(&self) -> Option<usize> {
        self.line
    }

    /// Return the underlying host-name error, when applicable.
    #[must_use]
    pub const fn domain_error(&self) -> Option<&HostSetError> {
        self.domain_error.as_ref()
    }

    const fn structural(kind: ManagedSectionErrorKind, line: Option<usize>) -> Self {
        Self {
            kind,
            line,
            domain_error: None,
        }
    }
}

impl fmt::Display for ManagedSectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = self
            .line
            .map_or_else(String::new, |line| format!(" on line {line}"));
        match self.kind {
            ManagedSectionErrorKind::DuplicateStart => {
                write!(formatter, "duplicate {START_MARKER} marker{location}")
            }
            ManagedSectionErrorKind::DuplicateEnd => {
                write!(formatter, "duplicate {END_MARKER} marker{location}")
            }
            ManagedSectionErrorKind::MissingEnd => {
                write!(formatter, "{START_MARKER} has no matching end marker")
            }
            ManagedSectionErrorKind::MissingStart => {
                write!(formatter, "{END_MARKER} has no matching start marker")
            }
            ManagedSectionErrorKind::EndBeforeStart => {
                write!(formatter, "{END_MARKER} appears before {START_MARKER}")
            }
            ManagedSectionErrorKind::InvalidMapping => write!(
                formatter,
                "locald-managed section contains a non-loopback mapping{location}"
            ),
            ManagedSectionErrorKind::InvalidDomain => {
                if let Some(error) = &self.domain_error {
                    write!(formatter, "{error}{location}")
                } else {
                    write!(formatter, "invalid host name{location}")
                }
            }
        }
    }
}

impl Error for ManagedSectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.domain_error
            .as_ref()
            .map(|error| error as &(dyn Error + 'static))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManagedSection {
    range: Option<std::ops::Range<usize>>,
    hosts: HostSet,
}

fn parse_managed_section(content: &str) -> Result<ManagedSection, ManagedSectionError> {
    let mut start = None;
    let mut end = None;
    let mut offset = 0;
    let mut start_line = None;
    let mut end_line = None;

    for (line_index, segment) in content.split_inclusive('\n').enumerate() {
        let line_number = line_index + 1;
        let line = segment
            .strip_suffix('\n')
            .unwrap_or(segment)
            .strip_suffix('\r')
            .unwrap_or_else(|| segment.strip_suffix('\n').unwrap_or(segment));
        if line == START_MARKER {
            if start.is_some() {
                return Err(ManagedSectionError::structural(
                    ManagedSectionErrorKind::DuplicateStart,
                    Some(line_number),
                ));
            }
            start = Some(offset);
            start_line = Some(line_number);
        } else if line == END_MARKER {
            if end.is_some() {
                return Err(ManagedSectionError::structural(
                    ManagedSectionErrorKind::DuplicateEnd,
                    Some(line_number),
                ));
            }
            end = Some(offset + segment.len());
            end_line = Some(line_number);
        }
        offset += segment.len();
    }

    match (start, end) {
        (None, None) => Ok(ManagedSection {
            range: None,
            hosts: HostSet::default(),
        }),
        (Some(_), None) => Err(ManagedSectionError::structural(
            ManagedSectionErrorKind::MissingEnd,
            start_line,
        )),
        (None, Some(_)) => Err(ManagedSectionError::structural(
            ManagedSectionErrorKind::MissingStart,
            end_line,
        )),
        (Some(section_start), Some(section_end)) if section_end <= section_start => Err(
            ManagedSectionError::structural(ManagedSectionErrorKind::EndBeforeStart, end_line),
        ),
        (Some(section_start), Some(section_end)) => {
            let content_start = content[section_start..]
                .find('\n')
                .map_or(section_start + START_MARKER.len(), |newline| {
                    section_start + newline + 1
                });
            let end_marker_start = content[..section_end]
                .rfind(END_MARKER)
                .unwrap_or(section_end - END_MARKER.len());
            let body = &content[content_start..end_marker_start];
            let body_first_line = start_line.unwrap_or(1) + 1;
            let mut domains = Vec::new();
            for (line_index, raw_line) in body.lines().enumerate() {
                let line_number = body_first_line + line_index;
                let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
                if line.trim().is_empty() {
                    continue;
                }
                let mut fields = line.split_whitespace();
                if fields.next() != Some("127.0.0.1") {
                    return Err(ManagedSectionError::structural(
                        ManagedSectionErrorKind::InvalidMapping,
                        Some(line_number),
                    ));
                }
                let before = domains.len();
                domains.extend(fields.map(str::to_owned));
                if domains.len() == before {
                    return Err(ManagedSectionError::structural(
                        ManagedSectionErrorKind::InvalidMapping,
                        Some(line_number),
                    ));
                }
            }
            let hosts =
                HostSet::try_from_strings(domains).map_err(|domain_error| ManagedSectionError {
                    kind: ManagedSectionErrorKind::InvalidDomain,
                    line: None,
                    domain_error: Some(domain_error),
                })?;
            Ok(ManagedSection {
                range: Some(section_start..section_end),
                hosts,
            })
        }
    }
}

/// Parse the complete locald-managed host set from a hosts file.
///
/// An absent managed section produces an empty set. Existing managed mappings
/// may place several names after one `127.0.0.1` address for compatibility.
///
/// # Errors
///
/// Returns [`ManagedSectionError`] for duplicate or unbalanced markers,
/// non-loopback managed mappings, or invalid host names.
pub fn managed_host_set(content: &str) -> Result<HostSet, ManagedSectionError> {
    parse_managed_section(content).map(|section| section.hosts)
}

/// Render a complete locald-managed host set while preserving outside bytes.
///
/// # Errors
///
/// Returns [`ManagedSectionError`] when the existing managed section is not
/// structurally valid.
pub fn render_hosts_content(
    current_content: &str,
    hosts: &HostSet,
) -> Result<String, ManagedSectionError> {
    let section = parse_managed_section(current_content)?;
    let replacement = render_section(hosts);
    match section.range {
        Some(range) => {
            let mut output =
                String::with_capacity(current_content.len() - range.len() + replacement.len());
            output.push_str(&current_content[..range.start]);
            output.push_str(&replacement);
            output.push_str(&current_content[range.end..]);
            Ok(output)
        }
        None if hosts.is_empty() => Ok(current_content.to_owned()),
        None => {
            let separator =
                usize::from(!current_content.is_empty() && !current_content.ends_with('\n'));
            let mut output =
                String::with_capacity(current_content.len() + separator + replacement.len());
            output.push_str(current_content);
            if separator == 1 {
                output.push('\n');
            }
            output.push_str(&replacement);
            Ok(output)
        }
    }
}

fn render_section(hosts: &HostSet) -> String {
    if hosts.is_empty() {
        return String::new();
    }
    let mut section = String::from(START_MARKER);
    section.push('\n');
    for domain in hosts {
        section.push_str("127.0.0.1 ");
        section.push_str(domain);
        section.push('\n');
    }
    section.push_str(END_MARKER);
    section.push('\n');
    section
}

/// Validation or parsing failure from [`update_hosts_content`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostsContentError {
    /// One requested host name was invalid.
    InvalidHost(HostSetError),
    /// The existing managed section was malformed.
    MalformedSection(ManagedSectionError),
}

impl fmt::Display for HostsContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHost(error) => error.fmt(formatter),
            Self::MalformedSection(error) => error.fmt(formatter),
        }
    }
}

impl Error for HostsContentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidHost(error) => Some(error),
            Self::MalformedSection(error) => Some(error),
        }
    }
}

/// Validate and replace locald's managed hosts-file section.
///
/// This compatibility entry point validates its string inputs before calling
/// [`render_hosts_content`]. New code that already owns a [`HostSet`] should
/// call that function directly.
///
/// # Errors
///
/// Returns [`HostsContentError`] when an input host name or the existing
/// managed section is invalid.
pub fn update_hosts_content(
    current_content: &str,
    domains: &[String],
) -> Result<String, HostsContentError> {
    let hosts = HostSet::try_from_strings(domains).map_err(HostsContentError::InvalidHost)?;
    render_hosts_content(current_content, &hosts).map_err(HostsContentError::MalformedSection)
}

/// Durable replace stage used to classify host-file publication failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplaceStage {
    /// Opening, inspecting, reading, parsing, or revalidating the target.
    Read,
    /// Creating, writing, securing, or synchronizing the temporary file.
    Write,
    /// Atomically renaming the temporary file over the target.
    Rename,
    /// Synchronizing the parent directory after a successful rename.
    ParentSync,
}

/// Failure while durably replacing a hosts file.
#[derive(Debug)]
pub struct ReplaceHostsError {
    stage: ReplaceStage,
    kind: ReplaceHostsErrorKind,
}

#[derive(Debug)]
enum ReplaceHostsErrorKind {
    InvalidPath(&'static str),
    UnsafeTarget(&'static str),
    UnsupportedFileFlags { flags: u32 },
    SourceTooLarge { actual: usize, limit: usize },
    MalformedSection(ManagedSectionError),
    ConcurrentModification,
    Io(std::io::Error),
}

impl ReplaceHostsError {
    /// Return the stable operation stage at which publication failed.
    #[must_use]
    pub const fn stage(&self) -> ReplaceStage {
        self.stage
    }

    /// Return whether the requested complete host set may already be visible.
    ///
    /// This is true only after the atomic rename succeeded and parent-directory
    /// synchronization failed. Recovery should reapply the intended complete
    /// set rather than infer state from the error alone.
    #[must_use]
    pub const fn publication_may_be_visible(&self) -> bool {
        matches!(self.stage, ReplaceStage::ParentSync)
    }

    const fn new(stage: ReplaceStage, kind: ReplaceHostsErrorKind) -> Self {
        Self { stage, kind }
    }

    fn io(stage: ReplaceStage, error: impl Into<std::io::Error>) -> Self {
        Self::new(stage, ReplaceHostsErrorKind::Io(error.into()))
    }
}

impl fmt::Display for ReplaceHostsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "hosts replacement failed during {:?}: ",
            self.stage
        )?;
        match &self.kind {
            ReplaceHostsErrorKind::InvalidPath(reason)
            | ReplaceHostsErrorKind::UnsafeTarget(reason) => formatter.write_str(reason),
            ReplaceHostsErrorKind::UnsupportedFileFlags { flags } => write!(
                formatter,
                "hosts file has unsupported or rename-blocking inode flags 0x{flags:08x}"
            ),
            ReplaceHostsErrorKind::SourceTooLarge { actual, limit } => write!(
                formatter,
                "hosts file is {actual} bytes, exceeding the {limit}-byte safety limit"
            ),
            ReplaceHostsErrorKind::MalformedSection(error) => error.fmt(formatter),
            ReplaceHostsErrorKind::ConcurrentModification => {
                formatter.write_str("hosts file changed while its replacement was prepared")
            }
            ReplaceHostsErrorKind::Io(error) => error.fmt(formatter),
        }
    }
}

impl Error for ReplaceHostsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            ReplaceHostsErrorKind::MalformedSection(error) => Some(error),
            ReplaceHostsErrorKind::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Atomically replace the locald-managed section in an existing Unix hosts file.
///
/// The writer anchors all target operations to an opened parent directory,
/// rejects symlinked and non-regular targets, preserves the target's safe
/// permission bits and ownership, synchronizes a same-directory temporary,
/// atomically exchanges it with the target, verifies the exact displaced
/// entry, restores a concurrent writer without overwriting it, and synchronizes
/// the parent directory.
///
/// # Errors
///
/// Returns [`ReplaceHostsError`] when the path or existing content is unsafe,
/// a concurrent writer is detected, or a filesystem operation fails.
#[cfg(unix)]
pub fn replace_hosts_file(
    path: impl AsRef<std::path::Path>,
    hosts: &HostSet,
) -> Result<(), ReplaceHostsError> {
    replace_hosts_file_with_hook(path.as_ref(), None, hosts, &mut NoopWriteHook)
}

/// Atomically replace a Unix hosts file only if it still has expected content.
///
/// This variant is intended for read/filter/write operations such as setup
/// cleanup. It prevents a complete host set derived from an earlier read from
/// overwriting a concurrent daemon publication. The writer also performs its
/// normal second snapshot comparison and displaced-entry verification around
/// the atomic exchange.
///
/// # Errors
///
/// Returns [`ReplaceHostsError`] with [`ReplaceStage::Read`] when the anchored
/// file no longer matches `expected_content`, in addition to the errors from
/// [`replace_hosts_file`].
#[cfg(unix)]
pub fn replace_hosts_file_if_unchanged(
    path: impl AsRef<std::path::Path>,
    expected_content: &str,
    hosts: &HostSet,
) -> Result<(), ReplaceHostsError> {
    replace_hosts_file_with_hook(
        path.as_ref(),
        Some(expected_content.as_bytes()),
        hosts,
        &mut NoopWriteHook,
    )
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriteHookPoint {
    AfterRead,
    AfterTemporarySync,
    BeforeRename,
    AfterExchange,
    AfterRestoreExchange,
    AfterRename,
}

#[cfg(unix)]
trait WriteHook {
    fn check(&mut self, point: WriteHookPoint) -> std::io::Result<()>;
}

#[cfg(unix)]
#[derive(Debug)]
struct NoopWriteHook;

#[cfg(unix)]
impl WriteHook for NoopWriteHook {
    fn check(&mut self, _point: WriteHookPoint) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct TargetSnapshot {
    file: std::fs::File,
    device: u64,
    inode: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    extended_attributes: Vec<(std::ffi::OsString, Vec<u8>)>,
    access_control_list: Option<Vec<u8>>,
    file_flags: Option<u32>,
    bytes: Vec<u8>,
}

#[cfg(unix)]
impl TargetSnapshot {
    fn same_target_and_content(&self, other: &Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.mode == other.mode
            && self.uid == other.uid
            && self.gid == other.gid
            && self.extended_attributes == other.extended_attributes
            && self.access_control_list == other.access_control_list
            && self.file_flags == other.file_flags
            && self.bytes == other.bytes
    }
}

#[cfg(unix)]
fn replace_hosts_file_with_hook(
    path: &std::path::Path,
    expected_content: Option<&[u8]>,
    hosts: &HostSet,
    hook: &mut impl WriteHook,
) -> Result<(), ReplaceHostsError> {
    use nix::fcntl::{AtFlags, OFlag};
    use nix::sys::stat::{Mode, SFlag};
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::fs::MetadataExt as _;

    if !path.is_absolute() {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::InvalidPath("hosts path must be absolute"),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::InvalidPath("hosts path has no parent directory"),
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::InvalidPath("hosts path has no file name"),
        )
    })?;
    // macOS exposes `/etc` as the system-owned `/private/etc` alias. Resolve
    // the parent once, then anchor every target operation to the resulting
    // directory descriptor. The final component is still opened relative to
    // that descriptor with `O_NOFOLLOW`.
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    let directory = nix::fcntl::open(
        &canonical_parent,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;

    let metadata = nix::sys::stat::fstatat(&directory, name, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    if !SFlag::from_bits_truncate(metadata.st_mode).contains(SFlag::S_IFREG) {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::UnsafeTarget(
                "hosts path must name an existing regular non-symlink file",
            ),
        ));
    }

    let original = read_snapshot(&directory, name)?;
    if expected_content.is_some_and(|expected| original.bytes != expected) {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::ConcurrentModification,
        ));
    }
    hook.check(WriteHookPoint::AfterRead)
        .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    let current_content = std::str::from_utf8(&original.bytes).map_err(|_| {
        ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::UnsafeTarget("hosts file must contain UTF-8 text"),
        )
    })?;
    let replacement = render_hosts_content(current_content, hosts).map_err(|error| {
        ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::MalformedSection(error),
        )
    })?;
    if replacement.len() > MAX_HOSTS_FILE_BYTES {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Write,
            ReplaceHostsErrorKind::SourceTooLarge {
                actual: replacement.len(),
                limit: MAX_HOSTS_FILE_BYTES,
            },
        ));
    }
    if replacement.as_bytes() == original.bytes {
        let current = read_snapshot(&directory, name)?;
        if original.same_target_and_content(&current) {
            // A matching file can be the visible result of an earlier rename
            // whose parent-directory fsync failed. Complete the full durable
            // publication protocol before reporting an idempotent retry as
            // successful.
            current
                .file
                .sync_all()
                .map_err(|error| ReplaceHostsError::io(ReplaceStage::ParentSync, error))?;
            hook.check(WriteHookPoint::AfterRename)
                .map_err(|error| ReplaceHostsError::io(ReplaceStage::ParentSync, error))?;
            nix::unistd::fsync(&directory)
                .map_err(|error| ReplaceHostsError::io(ReplaceStage::ParentSync, error))?;
            return Ok(());
        }
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::ConcurrentModification,
        ));
    }

    let temporary = format!(
        ".{}.locald-tmp.{}.{}",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let temporary_fd = nix::fcntl::openat(
        &directory,
        temporary.as_str(),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;
    let mut temporary_file = File::from(temporary_fd);
    let mut exchange_started = false;
    let mut preserve_temporary = false;

    let result = (|| {
        temporary_file
            .write_all(replacement.as_bytes())
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;
        let temporary_metadata = temporary_file
            .metadata()
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;
        if temporary_metadata.uid() != original.uid || temporary_metadata.gid() != original.gid {
            nix::unistd::fchown(
                &temporary_file,
                Some(nix::unistd::Uid::from_raw(original.uid)),
                Some(nix::unistd::Gid::from_raw(original.gid)),
            )
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;
        }
        let safe_mode = nix::libc::mode_t::try_from(original.mode & 0o777).map_err(|_| {
            ReplaceHostsError::io(
                ReplaceStage::Write,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "hosts file mode is outside the supported range",
                ),
            )
        })?;
        nix::sys::stat::fchmod(&temporary_file, Mode::from_bits_truncate(safe_mode))
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;
        copy_preserved_metadata(&original, &temporary_file)
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;
        temporary_file
            .sync_all()
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;
        hook.check(WriteHookPoint::AfterTemporarySync)
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Write, error))?;

        let current = read_snapshot(&directory, name)?;
        if !original.same_target_and_content(&current) {
            return Err(ReplaceHostsError::new(
                ReplaceStage::Read,
                ReplaceHostsErrorKind::ConcurrentModification,
            ));
        }
        hook.check(WriteHookPoint::BeforeRename)
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Rename, error))?;
        let candidate = read_snapshot(&directory, std::ffi::OsStr::new(&temporary))?;
        if candidate.access_control_list != original.access_control_list {
            return Err(ReplaceHostsError::io(
                ReplaceStage::Write,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "prepared hosts file ACL does not match its captured source ACL",
                ),
            ));
        }
        exchange_entries(&directory, std::ffi::OsStr::new(&temporary), name)
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::Rename, error))?;
        exchange_started = true;
        hook.check(WriteHookPoint::AfterExchange).map_err(|error| {
            preserve_temporary = true;
            ReplaceHostsError::io(ReplaceStage::ParentSync, error)
        })?;

        let displaced =
            read_snapshot(&directory, std::ffi::OsStr::new(&temporary)).map_err(|error| {
                preserve_temporary = true;
                ReplaceHostsError::io(ReplaceStage::ParentSync, std::io::Error::other(error))
            })?;
        if !original.same_target_and_content(&displaced) {
            // The atomic exchange captured a writer that landed after our
            // final snapshot. Restore that exact entry without ever replacing
            // a still-newer writer: each exchange publishes the most recently
            // displaced entry and captures the current target for comparison.
            let mut expected_target = candidate;
            loop {
                let about_to_publish = read_snapshot(&directory, std::ffi::OsStr::new(&temporary))
                    .map_err(|error| {
                        preserve_temporary = true;
                        ReplaceHostsError::io(
                            ReplaceStage::ParentSync,
                            std::io::Error::other(error),
                        )
                    })?;
                exchange_entries(&directory, std::ffi::OsStr::new(&temporary), name).map_err(
                    |error| {
                        preserve_temporary = true;
                        ReplaceHostsError::io(ReplaceStage::ParentSync, error)
                    },
                )?;
                hook.check(WriteHookPoint::AfterRestoreExchange)
                    .map_err(|error| {
                        preserve_temporary = true;
                        ReplaceHostsError::io(ReplaceStage::ParentSync, error)
                    })?;
                let captured = read_snapshot(&directory, std::ffi::OsStr::new(&temporary))
                    .map_err(|error| {
                        preserve_temporary = true;
                        ReplaceHostsError::io(
                            ReplaceStage::ParentSync,
                            std::io::Error::other(error),
                        )
                    })?;
                if expected_target.same_target_and_content(&captured) {
                    nix::unistd::unlinkat(
                        &directory,
                        temporary.as_str(),
                        nix::unistd::UnlinkatFlags::NoRemoveDir,
                    )
                    .map_err(|error| {
                        preserve_temporary = true;
                        ReplaceHostsError::io(ReplaceStage::ParentSync, error)
                    })?;
                    nix::unistd::fsync(&directory)
                        .map_err(|error| ReplaceHostsError::io(ReplaceStage::ParentSync, error))?;
                    return Err(ReplaceHostsError::new(
                        ReplaceStage::Read,
                        ReplaceHostsErrorKind::ConcurrentModification,
                    ));
                }
                expected_target = about_to_publish;
            }
        }

        nix::unistd::unlinkat(
            &directory,
            temporary.as_str(),
            nix::unistd::UnlinkatFlags::NoRemoveDir,
        )
        .map_err(|error| {
            preserve_temporary = true;
            ReplaceHostsError::io(ReplaceStage::ParentSync, error)
        })?;
        hook.check(WriteHookPoint::AfterRename)
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::ParentSync, error))?;
        nix::unistd::fsync(&directory)
            .map_err(|error| ReplaceHostsError::io(ReplaceStage::ParentSync, error))?;
        Ok(())
    })();

    if !exchange_started && !preserve_temporary {
        let _cleanup_failed = nix::unistd::unlinkat(
            &directory,
            temporary.as_str(),
            nix::unistd::UnlinkatFlags::NoRemoveDir,
        )
        .is_err();
    }
    result
}

#[cfg(unix)]
static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn exchange_entries(
    directory: &std::os::fd::OwnedFd,
    left: &std::ffi::OsStr,
    right: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let left = std::ffi::CString::new(left.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file name"))?;
    let right = std::ffi::CString::new(right.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file name"))?;
    let result = unsafe {
        nix::libc::renameat2(
            directory.as_raw_fd(),
            left.as_ptr(),
            directory.as_raw_fd(),
            right.as_ptr(),
            nix::libc::RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn exchange_entries(
    directory: &std::os::fd::OwnedFd,
    left: &std::ffi::OsStr,
    right: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let left = std::ffi::CString::new(left.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file name"))?;
    let right = std::ffi::CString::new(right.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file name"))?;
    let result = unsafe {
        nix::libc::renameatx_np(
            directory.as_raw_fd(),
            left.as_ptr(),
            directory.as_raw_fd(),
            right.as_ptr(),
            nix::libc::RENAME_SWAP,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn exchange_entries(
    _directory: &std::os::fd::OwnedFd,
    _left: &std::ffi::OsStr,
    _right: &std::ffi::OsStr,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this platform has no supported atomic exchange operation",
    ))
}

#[cfg(target_os = "linux")]
nix::ioctl_read!(
    /// Read Linux inode flags from an open file descriptor.
    get_file_flags,
    b'f',
    1,
    nix::libc::c_long
);

#[cfg(target_os = "linux")]
nix::ioctl_write_ptr!(
    /// Replace Linux inode flags on an open file descriptor.
    set_file_flags,
    b'f',
    2,
    nix::libc::c_long
);

#[cfg(unix)]
fn read_extended_attributes(
    file: &std::fs::File,
) -> std::io::Result<Vec<(std::ffi::OsString, Vec<u8>)>> {
    use xattr::FileExt as _;

    let mut attributes = Vec::new();
    let mut total_bytes = 0_usize;
    for name in file.list_xattr()? {
        let Some(value) = file.get_xattr(&name)? else {
            continue;
        };
        total_bytes = total_bytes
            .checked_add(name.as_encoded_bytes().len())
            .and_then(|size| size.checked_add(value.len()))
            .ok_or_else(|| std::io::Error::other("hosts-file metadata exceeds safety bound"))?;
        if total_bytes > MAX_HOSTS_FILE_BYTES {
            return Err(std::io::Error::other(
                "hosts-file metadata exceeds safety bound",
            ));
        }
        attributes.push((name, value));
    }
    attributes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(attributes)
}

#[cfg(unix)]
fn replace_extended_attributes(
    file: &std::fs::File,
    attributes: &[(std::ffi::OsString, Vec<u8>)],
) -> std::io::Result<()> {
    use xattr::FileExt as _;

    let existing = file.list_xattr()?.collect::<Vec<_>>();
    for name in existing {
        file.remove_xattr(name)?;
    }
    for (name, value) in attributes {
        file.set_xattr(name, value)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn read_access_control_list(file: &std::fs::File) -> std::io::Result<Option<Vec<u8>>> {
    use std::ffi::c_void;
    use std::os::fd::AsRawFd as _;

    const ACL_TYPE_EXTENDED: libc::c_int = 0x0000_0100;

    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, acl_type: libc::c_int) -> *mut c_void;
        fn acl_size(acl: *mut c_void) -> libc::ssize_t;
        fn acl_copy_ext(
            buffer: *mut c_void,
            acl: *mut c_void,
            size: libc::ssize_t,
        ) -> libc::ssize_t;
        fn acl_free(object: *mut c_void) -> libc::c_int;
    }

    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(error);
    }

    let result = (|| {
        let size = unsafe { acl_size(acl) };
        if size < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let size = usize::try_from(size).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "hosts-file ACL size is outside the supported range",
            )
        })?;
        if size > MAX_HOSTS_FILE_BYTES {
            return Err(std::io::Error::other("hosts-file ACL exceeds safety bound"));
        }

        let mut bytes = vec![0_u8; size];
        let copied = unsafe {
            acl_copy_ext(
                bytes.as_mut_ptr().cast::<c_void>(),
                acl,
                libc::ssize_t::try_from(size).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "hosts-file ACL size is outside the supported range",
                    )
                })?,
            )
        };
        if copied < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if usize::try_from(copied).ok() != Some(size) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "hosts-file ACL serialization was incomplete",
            ));
        }
        Ok(Some(bytes))
    })();

    let free_result = unsafe { acl_free(acl) };
    if free_result != 0 && result.is_ok() {
        return Err(std::io::Error::last_os_error());
    }
    result
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn apply_access_control_list(
    file: &std::fs::File,
    serialized: Option<&[u8]>,
) -> std::io::Result<()> {
    use std::ffi::c_void;
    use std::os::fd::AsRawFd as _;

    const ACL_TYPE_EXTENDED: libc::c_int = 0x0000_0100;

    unsafe extern "C" {
        fn acl_copy_int(buffer: *const c_void) -> *mut c_void;
        fn acl_init(count: libc::c_int) -> *mut c_void;
        fn acl_set_fd_np(fd: libc::c_int, acl: *mut c_void, acl_type: libc::c_int) -> libc::c_int;
        fn acl_free(object: *mut c_void) -> libc::c_int;
    }

    let acl = serialized.map_or_else(
        || unsafe { acl_init(0) },
        |serialized| unsafe { acl_copy_int(serialized.as_ptr().cast::<c_void>()) },
    );
    if acl.is_null() {
        return Err(std::io::Error::last_os_error());
    }

    let set_result = unsafe { acl_set_fd_np(file.as_raw_fd(), acl, ACL_TYPE_EXTENDED) };
    let set_error = (set_result != 0).then(std::io::Error::last_os_error);
    let free_result = unsafe { acl_free(acl) };
    if let Some(error) = set_error {
        return Err(error);
    }
    if free_result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_file_flags(file: &std::fs::File) -> std::io::Result<Option<u32>> {
    use std::os::macos::fs::MetadataExt as _;
    Ok(Some(file.metadata()?.st_flags()))
}

#[cfg(target_os = "macos")]
fn normalized_file_flags(flags: Option<u32>) -> Result<Option<u32>, ReplaceHostsError> {
    const SAFE_FLAGS: u32 = libc::UF_NODUMP | libc::UF_HIDDEN;

    let flags = flags.unwrap_or_default();
    let unsupported = flags & !SAFE_FLAGS;
    if unsupported != 0 {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::UnsupportedFileFlags { flags: unsupported },
        ));
    }
    Ok(Some(flags & SAFE_FLAGS))
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn read_file_flags(file: &std::fs::File) -> std::io::Result<Option<u32>> {
    use std::os::fd::AsRawFd as _;

    let mut flags: nix::libc::c_long = 0;
    match unsafe { get_file_flags(file.as_raw_fd(), &raw mut flags) } {
        Ok(_) => u32::try_from(flags).map(Some).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "hosts-file flags are outside the supported range",
            )
        }),
        Err(nix::errno::Errno::ENOTTY | nix::errno::Errno::EOPNOTSUPP) => Ok(None),
        Err(error) => Err(std::io::Error::from_raw_os_error(error as i32)),
    }
}

#[cfg(target_os = "linux")]
fn normalized_file_flags(flags: Option<u32>) -> Result<Option<u32>, ReplaceHostsError> {
    // FS_EXTENT_FL is a read-only description commonly returned for ordinary
    // ext4 files. It belongs to the source inode and must not be copied to a
    // newly created replacement. The remaining accepted flags are safe to set
    // on the temporary before rename; every other bit fails closed rather than
    // silently dropping security or lifecycle semantics.
    const FS_SYNC_FL: u32 = 0x0000_0008;
    const FS_NODUMP_FL: u32 = 0x0000_0040;
    const FS_NOATIME_FL: u32 = 0x0000_0080;
    const FS_EXTENT_FL: u32 = 0x0008_0000;
    const SAFE_FLAGS: u32 = FS_SYNC_FL | FS_NODUMP_FL | FS_NOATIME_FL;

    let flags = flags.unwrap_or_default();
    let unsupported = flags & !(SAFE_FLAGS | FS_EXTENT_FL);
    if unsupported != 0 {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::UnsupportedFileFlags { flags: unsupported },
        ));
    }
    Ok(Some(flags & SAFE_FLAGS))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn read_file_flags(_file: &std::fs::File) -> std::io::Result<Option<u32>> {
    Ok(None)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn normalized_file_flags(flags: Option<u32>) -> Result<Option<u32>, ReplaceHostsError> {
    Ok(flags)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn copy_platform_metadata(
    _source: &std::fs::File,
    destination: &std::fs::File,
    file_flags: Option<u32>,
    access_control_list: Option<&[u8]>,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;

    apply_access_control_list(destination, access_control_list)?;
    if let Some(flags) = file_flags {
        let changed = unsafe { libc::fchflags(destination.as_raw_fd(), flags) };
        if changed != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn copy_platform_metadata(
    _source: &std::fs::File,
    destination: &std::fs::File,
    file_flags: Option<u32>,
    _access_control_list: Option<&[u8]>,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;

    if let Some(flags) = file_flags.filter(|flags| *flags != 0) {
        let flags = nix::libc::c_long::from(flags);
        unsafe { set_file_flags(destination.as_raw_fd(), &raw const flags) }
            .map_err(|error| std::io::Error::from_raw_os_error(error as i32))?;
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn copy_platform_metadata(
    _source: &std::fs::File,
    _destination: &std::fs::File,
    _file_flags: Option<u32>,
    _access_control_list: Option<&[u8]>,
) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn copy_preserved_metadata(
    source: &TargetSnapshot,
    destination: &std::fs::File,
) -> std::io::Result<()> {
    replace_extended_attributes(destination, &source.extended_attributes)?;
    copy_platform_metadata(
        &source.file,
        destination,
        source.file_flags,
        source.access_control_list.as_deref(),
    )
}

#[cfg(unix)]
fn read_snapshot(
    directory: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
) -> Result<TargetSnapshot, ReplaceHostsError> {
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;
    use std::io::Read;
    use std::os::unix::fs::MetadataExt as _;

    let fd = nix::fcntl::openat(
        directory,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    let mut file = std::fs::File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    if !metadata.file_type().is_file() {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::UnsafeTarget(
                "hosts path must name an existing regular non-symlink file",
            ),
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_HOSTS_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    if bytes.len() > MAX_HOSTS_FILE_BYTES {
        return Err(ReplaceHostsError::new(
            ReplaceStage::Read,
            ReplaceHostsErrorKind::SourceTooLarge {
                actual: bytes.len(),
                limit: MAX_HOSTS_FILE_BYTES,
            },
        ));
    }
    let extended_attributes = read_extended_attributes(&file)
        .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    #[cfg(target_os = "macos")]
    let access_control_list = read_access_control_list(&file)
        .map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    #[cfg(not(target_os = "macos"))]
    let access_control_list = None;
    let file_flags =
        read_file_flags(&file).map_err(|error| ReplaceHostsError::io(ReplaceStage::Read, error))?;
    let file_flags = normalized_file_flags(file_flags)?;
    Ok(TargetSnapshot {
        file,
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        uid: metadata.uid(),
        gid: metadata.gid(),
        extended_attributes,
        access_control_list,
        file_flags,
        bytes,
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn host_set_canonicalizes_sorts_and_deduplicates() {
        let hosts = HostSet::try_from_strings([
            "Web.Example.Local",
            "api.example.local",
            "web.example.local",
        ])
        .unwrap();

        assert_eq!(
            hosts.as_strings(),
            vec!["api.example.local", "web.example.local"]
        );
    }

    #[test]
    fn host_set_rejects_wildcards_injection_and_invalid_labels() {
        for invalid in [
            "*.example.local",
            "example.local\n127.0.0.1 injected.local",
            "-bad.example.local",
            "bad_.example.local",
            "127.0.0.1",
        ] {
            assert!(HostSet::try_from_strings([invalid]).is_err(), "{invalid}");
        }
    }

    #[test]
    fn strict_parser_rejects_duplicate_and_unbalanced_markers() {
        let duplicate = "# BEGIN locald\n# BEGIN locald\n# END locald\n";
        assert_eq!(
            managed_host_set(duplicate).unwrap_err().kind(),
            ManagedSectionErrorKind::DuplicateStart
        );
        assert_eq!(
            managed_host_set("# BEGIN locald\n").unwrap_err().kind(),
            ManagedSectionErrorKind::MissingEnd
        );
        assert_eq!(
            managed_host_set("# END locald\n").unwrap_err().kind(),
            ManagedSectionErrorKind::MissingStart
        );
    }

    #[test]
    fn strict_parser_reads_legacy_multi_host_lines() {
        let content = concat!(
            "127.0.0.1 localhost\n",
            "# BEGIN locald\n",
            "127.0.0.1 a.example.local B.example.local\n",
            "# END locald\n",
        );

        assert_eq!(
            managed_host_set(content).unwrap().as_strings(),
            vec!["a.example.local", "b.example.local"]
        );
    }

    #[test]
    fn rendering_preserves_every_byte_outside_the_managed_section() {
        let prefix = "127.0.0.1 localhost\n\n# an unrelated comment\n";
        let suffix = "\n::1 localhost\n";
        let current =
            format!("{prefix}# BEGIN locald\n127.0.0.1 old.local\n# END locald\n{suffix}");
        let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();

        let rendered = render_hosts_content(&current, &hosts).unwrap();

        assert!(rendered.starts_with(prefix));
        assert!(rendered.ends_with(suffix));
        assert!(rendered.contains("127.0.0.1 new.example.local\n"));
        assert!(!rendered.contains("old.local"));
    }

    #[test]
    fn empty_set_removes_only_the_managed_section() {
        let current = "before\n# BEGIN locald\n127.0.0.1 old.local\n# END locald\nafter\n";

        assert_eq!(
            render_hosts_content(current, &HostSet::default()).unwrap(),
            "before\nafter\n"
        );
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        #[cfg(target_os = "macos")]
        #[allow(unsafe_code)]
        fn set_fixture_flags(path: &std::path::Path, flags: u32) -> std::io::Result<()> {
            use std::os::fd::AsRawFd as _;

            let file = std::fs::File::open(path)?;
            let result = unsafe { libc::fchflags(file.as_raw_fd(), flags) };
            if result == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        }

        #[cfg(target_os = "macos")]
        fn add_fixture_acl(path: &std::path::Path) -> std::io::Result<()> {
            let status = std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(path)
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("chmod exited with {status}")))
            }
        }

        #[cfg(target_os = "macos")]
        fn clear_fixture_acl(path: &std::path::Path) -> std::io::Result<()> {
            let status = std::process::Command::new("/bin/chmod")
                .arg("-N")
                .arg(path)
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("chmod exited with {status}")))
            }
        }

        struct TestHook {
            point: WriteHookPoint,
            action: Option<Box<dyn FnOnce() + Send>>,
            fail: bool,
        }

        impl WriteHook for TestHook {
            fn check(&mut self, point: WriteHookPoint) -> std::io::Result<()> {
                if point != self.point {
                    return Ok(());
                }
                if let Some(action) = self.action.take() {
                    action();
                }
                if self.fail {
                    Err(std::io::Error::other("injected failure"))
                } else {
                    Ok(())
                }
            }
        }

        struct MultiStageHook {
            before: Option<Box<dyn FnOnce() + Send>>,
            after_initial: Option<Box<dyn FnOnce() + Send>>,
            after_restore: Option<Box<dyn FnOnce() + Send>>,
        }

        impl WriteHook for MultiStageHook {
            fn check(&mut self, point: WriteHookPoint) -> std::io::Result<()> {
                let action = match point {
                    WriteHookPoint::BeforeRename => self.before.take(),
                    WriteHookPoint::AfterExchange => self.after_initial.take(),
                    WriteHookPoint::AfterRestoreExchange => self.after_restore.take(),
                    _ => None,
                };
                if let Some(action) = action {
                    action();
                }
                Ok(())
            }
        }

        #[cfg(target_os = "macos")]
        struct TwoStageHook {
            first_point: WriteHookPoint,
            first: Option<Box<dyn FnOnce() + Send>>,
            second_point: WriteHookPoint,
            second: Option<Box<dyn FnOnce() + Send>>,
        }

        #[cfg(target_os = "macos")]
        impl WriteHook for TwoStageHook {
            fn check(&mut self, point: WriteHookPoint) -> std::io::Result<()> {
                let action = if point == self.first_point {
                    self.first.take()
                } else if point == self.second_point {
                    self.second.take()
                } else {
                    None
                };
                if let Some(action) = action {
                    action();
                }
                Ok(())
            }
        }

        #[test]
        fn durable_replace_preserves_metadata_and_unrelated_content() {
            use xattr::FileExt as _;

            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(
                &path,
                "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 old.local\n# END locald\n",
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
            let attribute_name = if cfg!(target_os = "macos") {
                "com.locald.test-metadata"
            } else {
                "user.locald-test-metadata"
            };
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap()
                .set_xattr(attribute_name, b"preserved")
                .unwrap();
            let before = std::fs::metadata(&path).unwrap();
            let hosts = HostSet::try_from_strings(["custom.example.local"]).unwrap();

            replace_hosts_file(&path, &hosts).unwrap();

            let after = std::fs::metadata(&path).unwrap();
            let content = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after.mode() & 0o777, 0o640);
            assert_eq!(after.uid(), before.uid());
            assert_eq!(after.gid(), before.gid());
            assert_eq!(
                std::fs::File::open(&path)
                    .unwrap()
                    .get_xattr(attribute_name)
                    .unwrap()
                    .as_deref(),
                Some(b"preserved".as_slice())
            );
            assert!(content.starts_with("127.0.0.1 localhost\n"));
            assert!(content.contains("127.0.0.1 custom.example.local\n"));
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn durable_replace_preserves_access_control_list() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            add_fixture_acl(&path).unwrap();
            let before = read_access_control_list(&std::fs::File::open(&path).unwrap()).unwrap();
            assert!(before.is_some());
            let hosts = HostSet::try_from_strings(["custom.example.local"]).unwrap();

            replace_hosts_file(&path, &hosts).unwrap();

            let after = read_access_control_list(&std::fs::File::open(&path).unwrap()).unwrap();
            assert_eq!(after, before);
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn applying_an_absent_access_control_list_clears_an_existing_acl() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            add_fixture_acl(&path).unwrap();
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            assert!(read_access_control_list(&file).unwrap().is_some());

            apply_access_control_list(&file, None).unwrap();

            assert_eq!(read_access_control_list(&file).unwrap(), None);
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn prepared_acl_comes_from_the_snapshot_across_an_aba_source_change() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            let added_path = path.clone();
            let cleared_path = path.clone();
            let mut hook = TwoStageHook {
                first_point: WriteHookPoint::AfterRead,
                first: Some(Box::new(move || {
                    add_fixture_acl(&added_path).unwrap();
                })),
                second_point: WriteHookPoint::AfterTemporarySync,
                second: Some(Box::new(move || {
                    clear_fixture_acl(&cleared_path).unwrap();
                })),
            };
            let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();

            replace_hosts_file_with_hook(&path, None, &hosts, &mut hook).unwrap();

            assert_eq!(
                read_access_control_list(&std::fs::File::open(&path).unwrap()).unwrap(),
                None
            );
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("new.example.local")
            );
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn durable_replace_preserves_safe_nonzero_file_flags() {
            use std::os::macos::fs::MetadataExt as _;

            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            set_fixture_flags(&path, libc::UF_NODUMP).unwrap();
            let hosts = HostSet::try_from_strings(["custom.example.local"]).unwrap();

            let result = replace_hosts_file(&path, &hosts);
            let preserved = std::fs::metadata(&path).unwrap().st_flags();
            set_fixture_flags(&path, 0).unwrap();

            result.unwrap();
            assert_eq!(preserved & libc::UF_NODUMP, libc::UF_NODUMP);
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn durable_replace_rejects_rename_blocking_flags_without_a_temporary() {
            for (name, flag) in [
                ("immutable", libc::UF_IMMUTABLE),
                ("append", libc::UF_APPEND),
            ] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("hosts");
                let original = "127.0.0.1 localhost\n";
                std::fs::write(&path, original).unwrap();
                set_fixture_flags(&path, flag).unwrap();
                let hosts = HostSet::try_from_strings(["custom.example.local"]).unwrap();

                let result = replace_hosts_file(&path, &hosts);
                let temporary_exists = std::fs::read_dir(directory.path()).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .contains("locald-tmp")
                });
                set_fixture_flags(&path, 0).unwrap();

                let error = result.expect_err(name);
                assert_eq!(error.stage(), ReplaceStage::Read);
                assert!(error.to_string().contains("rename-blocking inode flags"));
                assert!(!temporary_exists, "{name} flag left a temporary file");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            }
        }

        #[test]
        fn durable_replace_is_an_inode_preserving_noop_when_content_matches() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            let content = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 custom.example.local\n# END locald\n";
            std::fs::write(&path, content).unwrap();
            let before = std::fs::metadata(&path).unwrap();
            let hosts = HostSet::try_from_strings(["custom.example.local"]).unwrap();

            replace_hosts_file(&path, &hosts).unwrap();

            let after = std::fs::metadata(&path).unwrap();
            assert_eq!(after.ino(), before.ino());
            assert_eq!(std::fs::read_to_string(path).unwrap(), content);
        }

        #[test]
        fn matching_retry_completes_parent_sync_before_reporting_success() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            let hosts = HostSet::try_from_strings(["custom.example.local"]).unwrap();

            let mut first = TestHook {
                point: WriteHookPoint::AfterRename,
                action: None,
                fail: true,
            };
            let first_error =
                replace_hosts_file_with_hook(&path, None, &hosts, &mut first).unwrap_err();
            assert_eq!(first_error.stage(), ReplaceStage::ParentSync);
            assert!(first_error.publication_may_be_visible());

            let mut retry = TestHook {
                point: WriteHookPoint::AfterRename,
                action: None,
                fail: true,
            };
            let retry_error =
                replace_hosts_file_with_hook(&path, None, &hosts, &mut retry).unwrap_err();
            assert_eq!(retry_error.stage(), ReplaceStage::ParentSync);
            assert!(retry_error.publication_may_be_visible());

            replace_hosts_file(&path, &hosts).expect("durably complete matching retry");
            assert!(
                std::fs::read_to_string(path)
                    .unwrap()
                    .contains("custom.example.local")
            );
        }

        #[test]
        fn durable_replace_rejects_symlink_and_non_regular_targets() {
            let directory = tempfile::tempdir().unwrap();
            let real = directory.path().join("real");
            let link = directory.path().join("link");
            std::fs::write(&real, "127.0.0.1 localhost\n").unwrap();
            std::os::unix::fs::symlink(&real, &link).unwrap();

            let error = replace_hosts_file(&link, &HostSet::default()).unwrap_err();
            assert_eq!(error.stage(), ReplaceStage::Read);
            assert_eq!(
                std::fs::read_to_string(real).unwrap(),
                "127.0.0.1 localhost\n"
            );

            let error = replace_hosts_file(directory.path(), &HostSet::default()).unwrap_err();
            assert_eq!(error.stage(), ReplaceStage::Read);
        }

        #[test]
        fn durable_replace_rejects_an_oversized_render_before_writing() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            let original = "127.0.0.1 localhost\n";
            std::fs::write(&path, original).unwrap();
            let hosts = HostSet::try_from_strings(
                (0..20_000).map(|index| format!("host-{index:05}.{}.test", "a".repeat(50))),
            )
            .unwrap();

            let error = replace_hosts_file(&path, &hosts).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::Write);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        #[test]
        fn durable_replace_anchors_a_canonicalized_parent_alias() {
            let directory = tempfile::tempdir().unwrap();
            let real_parent = directory.path().join("real-parent");
            let parent_alias = directory.path().join("parent-alias");
            std::fs::create_dir(&real_parent).unwrap();
            std::os::unix::fs::symlink(&real_parent, &parent_alias).unwrap();
            let real_path = real_parent.join("hosts");
            let alias_path = parent_alias.join("hosts");
            std::fs::write(&real_path, "127.0.0.1 localhost\n").unwrap();
            let hosts = HostSet::try_from_strings(["alias.example.local"]).unwrap();

            replace_hosts_file(&alias_path, &hosts).unwrap();

            assert!(
                std::fs::read_to_string(real_path)
                    .unwrap()
                    .contains("alias.example.local")
            );
        }

        #[test]
        fn pre_rename_failure_keeps_original_and_removes_temporary() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            let original = "127.0.0.1 localhost\n";
            std::fs::write(&path, original).unwrap();
            let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();
            let mut hook = TestHook {
                point: WriteHookPoint::BeforeRename,
                action: None,
                fail: true,
            };

            let error = replace_hosts_file_with_hook(&path, None, &hosts, &mut hook).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::Rename);
            assert!(!error.publication_may_be_visible());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        #[test]
        fn concurrent_change_is_detected_before_rename() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            let changed_path = path.clone();
            let mut hook = TestHook {
                point: WriteHookPoint::AfterTemporarySync,
                action: Some(Box::new(move || {
                    std::fs::write(changed_path, "127.0.0.1 localhost\n# concurrent\n").unwrap();
                })),
                fail: false,
            };
            let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();

            let error = replace_hosts_file_with_hook(&path, None, &hosts, &mut hook).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::Read);
            assert!(!error.publication_may_be_visible());
            assert_eq!(
                std::fs::read_to_string(path).unwrap(),
                "127.0.0.1 localhost\n# concurrent\n"
            );
        }

        #[test]
        fn concurrent_change_after_revalidation_is_exchanged_back_without_overwrite() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            let changed_path = path.clone();
            let concurrent = "127.0.0.1 localhost\n# concurrent after validation\n";
            let mut hook = TestHook {
                point: WriteHookPoint::BeforeRename,
                action: Some(Box::new(move || {
                    let replacement = changed_path.with_extension("external");
                    std::fs::write(&replacement, concurrent).unwrap();
                    std::fs::rename(replacement, changed_path).unwrap();
                })),
                fail: false,
            };
            let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();

            let error = replace_hosts_file_with_hook(&path, None, &hosts, &mut hook).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::Read);
            assert!(!error.publication_may_be_visible());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), concurrent);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn concurrent_acl_change_after_revalidation_is_exchanged_back_without_overwrite() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            let original = "127.0.0.1 localhost\n";
            std::fs::write(&path, original).unwrap();
            let changed_path = path.clone();
            let mut hook = TestHook {
                point: WriteHookPoint::BeforeRename,
                action: Some(Box::new(move || {
                    add_fixture_acl(&changed_path).unwrap();
                })),
                fail: false,
            };
            let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();

            let error = replace_hosts_file_with_hook(&path, None, &hosts, &mut hook).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::Read);
            assert!(!error.publication_may_be_visible());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert!(
                read_access_control_list(&std::fs::File::open(&path).unwrap())
                    .unwrap()
                    .is_some()
            );
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        #[test]
        fn rollback_exchange_preserves_every_finite_concurrent_replacement() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            let first_path = path.clone();
            let second_path = path.clone();
            let third_path = path.clone();
            let first = "127.0.0.1 localhost\n# first concurrent writer\n";
            let second = "127.0.0.1 localhost\n# second concurrent writer\n";
            let third = "127.0.0.1 localhost\n# third concurrent writer\n";
            let mut hook = MultiStageHook {
                before: Some(Box::new(move || {
                    let replacement = first_path.with_extension("first");
                    std::fs::write(&replacement, first).unwrap();
                    std::fs::rename(replacement, first_path).unwrap();
                })),
                after_initial: Some(Box::new(move || {
                    let replacement = second_path.with_extension("second");
                    std::fs::write(&replacement, second).unwrap();
                    std::fs::rename(replacement, second_path).unwrap();
                })),
                after_restore: Some(Box::new(move || {
                    let replacement = third_path.with_extension("third");
                    std::fs::write(&replacement, third).unwrap();
                    std::fs::rename(replacement, third_path).unwrap();
                })),
            };
            let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();

            let error = replace_hosts_file_with_hook(&path, None, &hosts, &mut hook).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::Read);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), third);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        #[test]
        fn conditional_replace_rejects_a_stale_filtering_snapshot() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            let stale = "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 old.local\n# END locald\n";
            let concurrent =
                "127.0.0.1 localhost\n# BEGIN locald\n127.0.0.1 live.example.local\n# END locald\n";
            std::fs::write(&path, concurrent).unwrap();
            let retained = HostSet::try_from_strings(["old.local"]).unwrap();

            let error = replace_hosts_file_if_unchanged(&path, stale, &retained).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::Read);
            assert!(!error.publication_may_be_visible());
            assert_eq!(std::fs::read_to_string(path).unwrap(), concurrent);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        #[test]
        fn post_rename_failure_reports_visible_publication() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("hosts");
            std::fs::write(&path, "127.0.0.1 localhost\n").unwrap();
            let mut hook = TestHook {
                point: WriteHookPoint::AfterRename,
                action: None,
                fail: true,
            };
            let hosts = HostSet::try_from_strings(["new.example.local"]).unwrap();

            let error = replace_hosts_file_with_hook(&path, None, &hosts, &mut hook).unwrap_err();

            assert_eq!(error.stage(), ReplaceStage::ParentSync);
            assert!(error.publication_may_be_visible());
            assert!(
                std::fs::read_to_string(path)
                    .unwrap()
                    .contains("new.example.local")
            );
        }
    }
}
