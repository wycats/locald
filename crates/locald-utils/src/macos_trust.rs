//! Bounded system-wide trust inspection and repair.
//!
//! Trust is reusable only when exact membership, administrative
//! policy, and the intended user's effective HTTPS evaluation all agree.

use anyhow::{Context, Result, bail};
use sha1::Digest as _;
use std::io::Read as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const REPAIR_TIMEOUT: Duration = Duration::from_mins(2);
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// A successful observation, distinct from an inability to inspect trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustReadiness {
    /// Exact system membership, administrative root policy, and HTTPS verified.
    Ready,
    /// The exact certificate is absent from the System keychain.
    MissingSystemCertificate,
    /// The administrative trust domain has no entry for this certificate.
    MissingAdministrativeTrust,
    /// Known administrative policy does not grant unrestricted root trust.
    InsufficientAdministrativeTrust,
}

#[derive(Debug)]
struct SecurityOutput {
    success: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

trait SecurityRunner {
    fn run(&self, command: &mut Command, timeout: Duration) -> Result<SecurityOutput>;
}

struct SystemSecurity;

impl SecurityRunner for SystemSecurity {
    fn run(&self, command: &mut Command, timeout: Duration) -> Result<SecurityOutput> {
        Self::run_observed(command, timeout, |_| Ok(()))
    }
}

impl SystemSecurity {
    #[allow(clippy::disallowed_methods)]
    fn run_observed(
        command: &mut Command,
        timeout: Duration,
        spawned: impl FnOnce(u32) -> Result<()>,
    ) -> Result<SecurityOutput> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The spawn permit must end before waiting or authorization can block
        // unrelated descriptor acquisition for the entire authorization window.
        let mut child = crate::process_spawn::ProcessSpawnBarrier::global()
            .spawn_std_command(command)
            .context("could not spawn macOS security")?;
        let mut stdout_pipe = child.stdout.take().context("missing security stdout")?;
        let mut stderr_pipe = child.stderr.take().context("missing security stderr")?;
        let setup = (|| -> Result<()> {
            for descriptor in [
                &stdout_pipe as &dyn std::os::fd::AsFd,
                &stderr_pipe as &dyn std::os::fd::AsFd,
            ] {
                let flags = nix::fcntl::fcntl(descriptor.as_fd(), nix::fcntl::FcntlArg::F_GETFL)?;
                nix::fcntl::fcntl(
                    descriptor.as_fd(),
                    nix::fcntl::FcntlArg::F_SETFL(
                        nix::fcntl::OFlag::from_bits_truncate(flags)
                            | nix::fcntl::OFlag::O_NONBLOCK,
                    ),
                )?;
            }
            spawned(child.id())
        })();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let deadline = Instant::now() + timeout;
        let status = setup.and_then(|()| loop {
            if let Err(error) = drain(&mut stdout_pipe, &mut stdout).and_then(|()| drain(&mut stderr_pipe, &mut stderr)) { break Err(error); }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Err(error) => break Err(anyhow::Error::new(error).context("could not wait for macOS security")),
                Ok(None) if Instant::now() >= deadline => break Err(anyhow::anyhow!(
                    "macOS security timed out after {} seconds; authorization may have been interrupted; inspect trust before retrying", timeout.as_secs()
                )),
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            }
        });
        if let Err(cause) = &status {
            // Only our direct child is owned here. Never signal securityd or an
            // authorization UI process shared with other applications.
            cleanup_owned_child(&mut child).with_context(|| {
                format!(
                    "macOS security failed ({cause:#}); cleanup failed for owned PID {}",
                    child.id()
                )
            })?;
        }
        let status = status?;
        drain(&mut stdout_pipe, &mut stdout)?;
        drain(&mut stderr_pipe, &mut stderr)?;
        Ok(SecurityOutput {
            success: status.success(),
            code: status.code(),
            stdout,
            stderr,
        })
    }
}

#[allow(clippy::disallowed_methods)] // Dedicated synchronous runner; polling has a one-second deadline.
fn cleanup_owned_child(child: &mut std::process::Child) -> Result<()> {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return Ok(());
    }
    if let Err(error) = child.kill() {
        // A simultaneous natural exit can race with kill. Reap that case;
        // otherwise report the owned PID instead of entering an unbounded wait.
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Ok(());
        }
        return Err(error).context("could not terminate owned child; no blocking wait attempted");
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child
            .try_wait()
            .context("could not reap terminated owned child")?
        {
            Some(_) => return Ok(()),
            None if Instant::now() >= deadline => {
                bail!("owned child did not become reapable within the one-second cleanup deadline")
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn drain(pipe: &mut impl std::io::Read, output: &mut Vec<u8>) -> Result<()> {
    let mut buffer = [0; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if count > OUTPUT_LIMIT.saturating_sub(output.len()) {
                    bail!("macOS security exceeded its output limit");
                }
                output.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

#[allow(clippy::disallowed_methods)] // Every command uses the bounded, spawn-barrier-aware runner.
fn security() -> Command {
    Command::new("/usr/bin/security")
}

fn successful(output: SecurityOutput, operation: &str) -> Result<Vec<u8>> {
    if !output.success {
        bail!(
            "macOS security {operation} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

/// Inspect system trust without modifying a keychain or trust settings.
///
/// `owner` is an already validated setup identity; only the verifier child
/// changes identity. The private CA key never becomes readable by another user.
///
/// # Errors
/// Returns malformed-policy, access, process, timeout, and effective-verifier
/// failures separately from the repairable states in [`TrustReadiness`].
pub fn probe(certificate: &Path, owner: Option<(u32, u32)>) -> Result<TrustReadiness> {
    probe_with(certificate, owner, &SystemSecurity)
}

/// Bounded effective HTTPS check for frequent health polling. Full System
/// membership and administrative policy inspection belong to setup and doctor.
pub(crate) fn verify_https(certificate: &Path) -> Result<()> {
    verify_https_with(certificate, &SystemSecurity)
}

fn verify_https_with(certificate: &Path, runner: &impl SecurityRunner) -> Result<()> {
    let directory = certificate.parent().context("CA path has no parent")?;
    let (leaf, ca) =
        crate::cert::ssl_trust_probe_pems(certificate, &directory.join("rootCA-key.pem"))?;
    verify_https_pems(&leaf, &ca, None, runner)
}

fn probe_with(
    certificate: &Path,
    owner: Option<(u32, u32)>,
    runner: &impl SecurityRunner,
) -> Result<TrustReadiness> {
    let directory = certificate.parent().context("CA path has no parent")?;
    crate::cert::validate_root_ca_material_in_dir(directory)?;
    let (leaf, ca) =
        crate::cert::ssl_trust_probe_pems(certificate, &directory.join("rootCA-key.pem"))?;
    let pem = pem::parse(&ca).context("invalid CA PEM")?;
    let mut command = security();
    command.args(["find-certificate", "-a", "-p", SYSTEM_KEYCHAIN]);
    let certificates = successful(
        runner.run(&mut command, PROBE_TIMEOUT)?,
        "System keychain inspection",
    )?;
    let system_member = system_contains(&certificates, pem.contents())?;
    let export = crate::cert::TemporarySecurityFile::new("trust-settings", b"")?;
    let mut command = security();
    command
        .args(["trust-settings-export", "-d"])
        .arg(&export.path);
    let export_output = runner.run(&mut command, PROBE_TIMEOUT)?;
    let mut settings = Vec::new();
    std::fs::File::open(&export.path)?
        .take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut settings)?;
    if settings.len() > OUTPUT_LIMIT {
        bail!("administrative trust export exceeds safety limit");
    }
    // Administrative policy survives keychain certificate removal. Inspect it
    // before treating missing membership as permission to repair: retained
    // denials, constraints, or inspection failures must still stop setup.
    let policy = if empty_administrative_domain(&export_output, &settings) {
        TrustReadiness::MissingAdministrativeTrust
    } else {
        successful(export_output, "administrative trust export")?;
        administrative_policy(&settings, pem.contents())?
    };
    if !system_member {
        return Ok(TrustReadiness::MissingSystemCertificate);
    }
    if policy != TrustReadiness::Ready {
        return Ok(policy);
    }
    verify_https_pems(&leaf, &ca, owner, runner)?;
    Ok(TrustReadiness::Ready)
}

fn verify_https_pems(
    leaf: &str,
    ca: &[u8],
    owner: Option<(u32, u32)>,
    runner: &impl SecurityRunner,
) -> Result<()> {
    let leaf = crate::cert::TemporarySecurityFile::new_for_owner(
        "ssl-trust-leaf",
        leaf.as_bytes(),
        owner,
    )?;
    let ca = crate::cert::TemporarySecurityFile::new_for_owner("ssl-trust-root", ca, owner)?;
    let mut command = security();
    command
        .arg("verify-cert")
        .arg("-c")
        .arg(&leaf.path)
        .arg("-c")
        .arg(&ca.path)
        .args(["-p", "ssl", "-n", "localhost", "-L", "-q"]);
    if let Some((uid, gid)) = owner {
        command.gid(gid).uid(uid);
    }
    successful(
        runner.run(&mut command, PROBE_TIMEOUT)?,
        "intended-user HTTPS verification (trust was not changed)",
    )?;
    Ok(())
}

fn system_contains(pems: &[u8], expected: &[u8]) -> Result<bool> {
    let certificates =
        pem::parse_many(pems).context("malformed System keychain certificate export")?;
    if !pems.is_empty() && certificates.is_empty() {
        bail!("System keychain export contained no PEM certificates");
    }
    Ok(certificates.iter().any(|certificate| {
        certificate.tag() == "CERTIFICATE" && certificate.contents() == expected
    }))
}

fn empty_administrative_domain(output: &SecurityOutput, settings: &[u8]) -> bool {
    // SecurityTool maps errSecNoTrustSettings (-25263) to exit 1 and this
    // diagnostic. Other export failures, including localization we cannot
    // recognize, remain inspection errors and must not authorize a write.
    !output.success
        && output.code == Some(1)
        && output.stdout.is_empty()
        && settings.is_empty()
        && output.stderr
            == b"SecTrustSettingsCreateExternalRepresentation: No Trust Settings were found.\n"
}

fn administrative_policy(bytes: &[u8], certificate: &[u8]) -> Result<TrustReadiness> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .context("malformed administrative trust plist")?;
    let root = value
        .as_dictionary()
        .context("administrative trust plist is not a dictionary")?;
    if root
        .get("trustVersion")
        .and_then(plist::Value::as_unsigned_integer)
        != Some(1)
    {
        bail!("unsupported administrative trust version");
    }
    let list = root
        .get("trustList")
        .and_then(plist::Value::as_dictionary)
        .context("missing administrative trustList")?;
    let fingerprint = hex::encode_upper(sha1::Sha1::digest(certificate));
    let Some(entry) = list.get(&fingerprint) else {
        return Ok(TrustReadiness::MissingAdministrativeTrust);
    };
    let entry = entry
        .as_dictionary()
        .context("malformed administrative certificate entry")?;
    let (_, parsed) = x509_parser::parse_x509_certificate(certificate)
        .map_err(|error| anyhow::anyhow!("invalid CA DER: {error}"))?;
    if entry.get("issuerName").and_then(plist::Value::as_data) != Some(parsed.issuer().as_raw())
        || entry.get("serialNumber").and_then(plist::Value::as_data) != Some(parsed.raw_serial())
    {
        bail!("administrative trust certificate identity does not match the exact locald CA");
    }
    // An existing certificate entry with omitted/empty trustSettings is Apple's
    // unconditional TrustRoot representation. A missing entry is not equivalent.
    let Some(settings) = entry.get("trustSettings") else {
        return Ok(TrustReadiness::Ready);
    };
    let settings = settings
        .as_array()
        .context("malformed administrative trustSettings")?;
    if settings.is_empty() {
        return Ok(TrustReadiness::Ready);
    }
    let mut sufficient = true;
    for setting in settings {
        let setting = setting.as_dictionary().context("malformed trust setting")?;
        if setting.keys().any(|key| key != "kSecTrustSettingsResult") {
            bail!("constrained or unknown administrative trust policy requires inspection");
        }
        let result = setting
            .get("kSecTrustSettingsResult")
            .and_then(plist::Value::as_unsigned_integer)
            .context("missing or malformed trust result")?;
        match result {
            1 | 2 => {}
            4 => sufficient = false,
            3 => bail!(
                "administrative trust explicitly denies this CA; inspect the policy before repairing trust"
            ),
            _ => bail!("unknown administrative trust result {result}"),
        }
    }
    Ok(if sufficient {
        TrustReadiness::Ready
    } else {
        TrustReadiness::InsufficientAdministrativeTrust
    })
}

/// Repair only an observed, repairable absence, then prove readiness again.
/// The callback is emitted before the potentially interactive authorization.
///
/// # Errors
/// Inspection, authorization, timeout, and failed postflight errors stop setup.
pub fn ensure_system_trust(
    certificate: &Path,
    owner: (u32, u32),
    before_repair: impl FnOnce(),
) -> Result<()> {
    converge(
        || probe(certificate, Some(owner)),
        || {
            before_repair();
            install(certificate)
        },
    )
}

fn converge(
    mut inspect: impl FnMut() -> Result<TrustReadiness>,
    repair: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if inspect()? == TrustReadiness::Ready {
        return Ok(());
    }
    repair()?;
    let postflight = inspect().context("trust repair completed but readiness inspection failed")?;
    if postflight != TrustReadiness::Ready {
        bail!("trust repair completed but readiness remains {postflight:?}");
    }
    Ok(())
}

fn install(certificate: &Path) -> Result<()> {
    let mut command = security();
    command.args([
        "add-trusted-cert",
        "-d",
        "-r",
        "trustRoot",
        "-k",
        SYSTEM_KEYCHAIN,
    ]);
    command.arg(certificate);
    successful(
        SystemSecurity.run(&mut command, REPAIR_TIMEOUT)?,
        "trust authorization",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn certificate() -> Vec<u8> {
        rcgen::generate_simple_self_signed(vec!["localhost".into()])
            .unwrap()
            .cert
            .der()
            .to_vec()
    }

    fn policy(certificate: &[u8], settings: Option<plist::Value>) -> Vec<u8> {
        let (_, parsed) = x509_parser::parse_x509_certificate(certificate).unwrap();
        let mut entry = plist::Dictionary::new();
        entry.insert(
            "issuerName".into(),
            plist::Value::Data(parsed.issuer().as_raw().to_vec()),
        );
        entry.insert(
            "serialNumber".into(),
            plist::Value::Data(parsed.raw_serial().to_vec()),
        );
        if let Some(settings) = settings {
            entry.insert("trustSettings".into(), settings);
        }
        let mut list = plist::Dictionary::new();
        list.insert(
            hex::encode_upper(sha1::Sha1::digest(certificate)),
            plist::Value::Dictionary(entry),
        );
        let mut root = plist::Dictionary::new();
        root.insert("trustVersion".into(), plist::Value::Integer(1.into()));
        root.insert("trustList".into(), plist::Value::Dictionary(list));
        let mut bytes = Vec::new();
        plist::Value::Dictionary(root)
            .to_writer_xml(&mut bytes)
            .unwrap();
        bytes
    }

    #[test]
    fn exact_der_membership_rejects_same_named_and_user_only_certificates() {
        let ca = certificate();
        let same_name = certificate();
        assert!(!system_contains(b"", &ca).unwrap());
        let export = pem::encode(&pem::Pem::new("CERTIFICATE", same_name));
        assert!(!system_contains(export.as_bytes(), &ca).unwrap());
        let export = pem::encode(&pem::Pem::new("CERTIFICATE", ca.clone()));
        assert!(system_contains(export.as_bytes(), &ca).unwrap());
        assert!(system_contains(b"invalid export", &ca).is_err());
    }

    #[test]
    fn omitted_and_empty_settings_mean_root_trust_only_for_existing_exact_entry() {
        let ca = certificate();
        assert_eq!(
            administrative_policy(&policy(&ca, None), &ca).unwrap(),
            TrustReadiness::Ready
        );
        assert_eq!(
            administrative_policy(&policy(&ca, Some(plist::Value::Array(vec![]))), &ca).unwrap(),
            TrustReadiness::Ready
        );
        assert_eq!(
            administrative_policy(&policy(&certificate(), None), &ca).unwrap(),
            TrustReadiness::MissingAdministrativeTrust
        );
    }

    #[test]
    fn administrative_policy_preserves_denials_constraints_and_parse_errors() {
        let ca = certificate();
        for result in [1, 2, 4] {
            let mut setting = plist::Dictionary::new();
            setting.insert(
                "kSecTrustSettingsResult".into(),
                plist::Value::Integer(result.into()),
            );
            assert_eq!(
                administrative_policy(
                    &policy(
                        &ca,
                        Some(plist::Value::Array(vec![plist::Value::Dictionary(setting)]))
                    ),
                    &ca
                )
                .unwrap(),
                if result == 4 {
                    TrustReadiness::InsufficientAdministrativeTrust
                } else {
                    TrustReadiness::Ready
                }
            );
        }
        for result in [0, 3, 5, 999] {
            let mut setting = plist::Dictionary::new();
            setting.insert(
                "kSecTrustSettingsResult".into(),
                plist::Value::Integer(result.into()),
            );
            assert!(
                administrative_policy(
                    &policy(
                        &ca,
                        Some(plist::Value::Array(vec![plist::Value::Dictionary(setting)]))
                    ),
                    &ca
                )
                .is_err()
            );
        }
        let mut constrained = plist::Dictionary::new();
        constrained.insert(
            "kSecTrustSettingsPolicy".into(),
            plist::Value::String("ssl".into()),
        );
        assert!(
            administrative_policy(
                &policy(
                    &ca,
                    Some(plist::Value::Array(vec![plist::Value::Dictionary(
                        constrained
                    )]))
                ),
                &ca
            )
            .is_err()
        );
        assert!(
            administrative_policy(
                &policy(&ca, Some(plist::Value::String("invalid".into()))),
                &ca
            )
            .is_err()
        );
        assert!(administrative_policy(b"invalid plist", &ca).is_err());
    }

    #[test]
    fn repeated_setup_reuses_ready_trust_without_authorization() {
        let repairs = Cell::new(0);
        for _ in 0..2 {
            converge(
                || Ok(TrustReadiness::Ready),
                || {
                    repairs.set(repairs.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
        }
        assert_eq!(repairs.get(), 0);
    }

    #[test]
    fn absence_repairs_once_and_requires_postflight() {
        for missing in [
            TrustReadiness::MissingSystemCertificate,
            TrustReadiness::MissingAdministrativeTrust,
            TrustReadiness::InsufficientAdministrativeTrust,
        ] {
            let probes = Cell::new(0);
            let repairs = Cell::new(0);
            converge(
                || {
                    probes.set(probes.get() + 1);
                    Ok(if probes.get() == 1 {
                        missing
                    } else {
                        TrustReadiness::Ready
                    })
                },
                || {
                    repairs.set(repairs.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(probes.get(), 2);
            assert_eq!(repairs.get(), 1);
            assert!(converge(|| Ok(missing), || Ok(())).is_err());
        }
    }

    #[test]
    fn inspection_errors_never_authorize_and_denial_never_advances() {
        let repairs = Cell::new(0);
        assert!(
            converge(
                || bail!("inspection timed out"),
                || {
                    repairs.set(1);
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(repairs.get(), 0);
        let probes = Cell::new(0);
        assert!(
            converge(
                || {
                    probes.set(probes.get() + 1);
                    Ok(TrustReadiness::MissingSystemCertificate)
                },
                || bail!("authorization denied")
            )
            .is_err()
        );
        assert_eq!(probes.get(), 1);
    }

    #[test]
    fn pipe_output_is_bounded() {
        let mut bytes = std::io::Cursor::new(vec![0; OUTPUT_LIMIT + 1]);
        let mut output = Vec::new();
        assert!(drain(&mut bytes, &mut output).is_err());
        assert!(output.len() <= OUTPUT_LIMIT);
    }

    #[test]
    fn owned_child_timeout_reaps_without_security_or_trust_mutations() {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let pid = Cell::new(0);
        let error = SystemSecurity::run_observed(&mut command, Duration::ZERO, |child| {
            pid.set(child);
            let _guard = crate::process_spawn::ProcessSpawnBarrier::global()
                .enter_descriptor_acquisition_before(Instant::now() + Duration::from_secs(2))?;
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert_eq!(
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(i32::try_from(pid.get()).unwrap()),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG)
            ),
            Err(nix::errno::Errno::ECHILD)
        );
    }

    #[test]
    fn command_failure_is_not_a_negative_trust_probe() {
        let mut command = Command::new("/usr/bin/false");
        let output = SystemSecurity.run(&mut command, PROBE_TIMEOUT).unwrap();
        assert!(successful(output, "fixture").is_err());
        let mut command = Command::new("/does-not-exist/locald-security-fixture");
        assert!(SystemSecurity.run(&mut command, PROBE_TIMEOUT).is_err());
    }

    #[test]
    fn only_exact_empty_domain_diagnostic_is_repairable() {
        let mut output = SecurityOutput {
            success: false,
            code: Some(1),
            stdout: vec![],
            stderr:
                b"SecTrustSettingsCreateExternalRepresentation: No Trust Settings were found.\n"
                    .to_vec(),
        };
        assert!(empty_administrative_domain(&output, b""));
        assert!(!empty_administrative_domain(&output, b"partial export"));
        output.code = Some(2);
        assert!(!empty_administrative_domain(&output, b""));
        output.code = Some(1);
        output.stderr.extend_from_slice(b"other failure");
        assert!(!empty_administrative_domain(&output, b""));
    }

    struct ProbeFixture {
        ca: Vec<u8>,
        calls: Cell<usize>,
        fail_verification: bool,
        system_member: bool,
        admin_settings: Option<plist::Value>,
        fail_export: bool,
    }

    impl SecurityRunner for ProbeFixture {
        fn run(&self, command: &mut Command, timeout: Duration) -> Result<SecurityOutput> {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(timeout, PROBE_TIMEOUT);
            assert_eq!(command.get_program(), "/usr/bin/security");
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            let index = self.calls.get();
            self.calls.set(index + 1);
            let stdout = match index {
                0 => {
                    assert_eq!(args, ["find-certificate", "-a", "-p", SYSTEM_KEYCHAIN]);
                    if self.system_member {
                        self.ca.clone()
                    } else {
                        vec![]
                    }
                }
                1 => {
                    assert_eq!(&args[..2], ["trust-settings-export", "-d"]);
                    if self.fail_export {
                        bail!("injected administrative export failure");
                    }
                    let certificate = pem::parse(&self.ca).unwrap();
                    std::fs::write(
                        &args[2],
                        policy(certificate.contents(), self.admin_settings.clone()),
                    )
                    .unwrap();
                    vec![]
                }
                2 => {
                    assert_eq!(args[0], "verify-cert");
                    assert_eq!(args[1], "-c");
                    assert_eq!(args[3], "-c");
                    assert_eq!(&args[5..], ["-p", "ssl", "-n", "localhost", "-L", "-q"]);
                    assert!(!args.iter().any(|arg| arg == "-r"));
                    for path in [&args[2], &args[4]] {
                        let metadata = std::fs::metadata(path).unwrap();
                        assert_eq!(metadata.mode() & 0o777, 0o600);
                        assert_eq!(metadata.uid(), nix::unistd::geteuid().as_raw());
                        assert!(
                            !std::fs::read_to_string(path)
                                .unwrap()
                                .contains("PRIVATE KEY")
                        );
                    }
                    if self.fail_verification {
                        bail!("injected effective verifier failure");
                    }
                    vec![]
                }
                _ => panic!("unexpected command after verification"),
            };
            Ok(SecurityOutput {
                success: true,
                code: Some(0),
                stdout,
                stderr: vec![],
            })
        }
    }

    #[test]
    fn repeated_health_checks_only_invoke_effective_https_verification() {
        let directory = tempfile::tempdir().unwrap();
        let certs = directory.path().canonicalize().unwrap();
        let owner = (
            nix::unistd::geteuid().as_raw(),
            nix::unistd::getegid().as_raw(),
        );
        let ca = crate::cert::repair_root_ca_in_dir(&certs, owner.0, owner.1).unwrap();
        for fail_verification in [false, true] {
            for _ in 0..3 {
                // Enter the fixture at its verifier stage: any certificate or
                // policy export command on the polling path fails the test.
                let fixture = ProbeFixture {
                    ca: vec![],
                    calls: Cell::new(2),
                    fail_verification,
                    system_member: false,
                    admin_settings: None,
                    fail_export: true,
                };
                let result = verify_https_with(&ca.paths.cert_path, &fixture);
                assert_eq!(result.is_ok(), !fail_verification);
                assert_eq!(fixture.calls.get(), 3);
            }
        }
    }

    #[test]
    fn trust_as_root_requires_effective_https_and_never_requests_repair() {
        let directory = tempfile::tempdir().unwrap();
        let certs = directory.path().canonicalize().unwrap();
        let owner = (
            nix::unistd::geteuid().as_raw(),
            nix::unistd::getegid().as_raw(),
        );
        let ca = crate::cert::repair_root_ca_in_dir(&certs, owner.0, owner.1).unwrap();
        let mut setting = plist::Dictionary::new();
        setting.insert(
            "kSecTrustSettingsResult".into(),
            plist::Value::Integer(2.into()),
        );
        for fail_verification in [false, true] {
            let fixture = ProbeFixture {
                ca: std::fs::read(&ca.paths.cert_path).unwrap(),
                calls: Cell::new(0),
                fail_verification,
                system_member: true,
                admin_settings: Some(plist::Value::Array(vec![plist::Value::Dictionary(
                    setting.clone(),
                )])),
                fail_export: false,
            };
            let repairs = Cell::new(0);
            let result = converge(
                || probe_with(&ca.paths.cert_path, Some(owner), &fixture),
                || {
                    repairs.set(repairs.get() + 1);
                    Ok(())
                },
            );
            assert_eq!(result.is_ok(), !fail_verification);
            assert_eq!(fixture.calls.get(), 3);
            assert_eq!(repairs.get(), 0);
        }
    }

    #[test]
    fn probe_requires_exact_system_admin_and_owner_https_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let certs = directory.path().canonicalize().unwrap();
        let owner = (
            nix::unistd::geteuid().as_raw(),
            nix::unistd::getegid().as_raw(),
        );
        let ca = crate::cert::repair_root_ca_in_dir(&certs, owner.0, owner.1).unwrap();
        for fail_verification in [false, true] {
            let fixture = ProbeFixture {
                ca: std::fs::read(&ca.paths.cert_path).unwrap(),
                calls: Cell::new(0),
                fail_verification,
                system_member: true,
                admin_settings: None,
                fail_export: false,
            };
            let result = probe_with(&ca.paths.cert_path, Some(owner), &fixture);
            if fail_verification {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), TrustReadiness::Ready);
            }
            assert_eq!(fixture.calls.get(), 3);
        }
    }

    #[test]
    fn retained_administrative_protection_blocks_repair_even_without_system_membership() {
        let directory = tempfile::tempdir().unwrap();
        let certs = directory.path().canonicalize().unwrap();
        let owner = (
            nix::unistd::geteuid().as_raw(),
            nix::unistd::getegid().as_raw(),
        );
        let ca = crate::cert::repair_root_ca_in_dir(&certs, owner.0, owner.1).unwrap();
        let mut deny = plist::Dictionary::new();
        deny.insert(
            "kSecTrustSettingsResult".into(),
            plist::Value::Integer(3.into()),
        );
        let mut constraint = plist::Dictionary::new();
        constraint.insert(
            "kSecTrustSettingsPolicy".into(),
            plist::Value::String("ssl".into()),
        );
        for system_member in [false, true] {
            for (admin_settings, fail_export) in [
                (
                    Some(plist::Value::Array(vec![plist::Value::Dictionary(
                        deny.clone(),
                    )])),
                    false,
                ),
                (
                    Some(plist::Value::Array(vec![plist::Value::Dictionary(
                        constraint.clone(),
                    )])),
                    false,
                ),
                (None, true),
            ] {
                let fixture = ProbeFixture {
                    ca: std::fs::read(&ca.paths.cert_path).unwrap(),
                    calls: Cell::new(0),
                    fail_verification: false,
                    system_member,
                    admin_settings,
                    fail_export,
                };
                let repairs = Cell::new(0);
                let result = converge(
                    || probe_with(&ca.paths.cert_path, Some(owner), &fixture),
                    || {
                        repairs.set(repairs.get() + 1);
                        Ok(())
                    },
                );
                assert!(result.is_err());
                assert_eq!(fixture.calls.get(), 2);
                assert_eq!(repairs.get(), 0);
            }
        }
        let fixture = ProbeFixture {
            ca: std::fs::read(&ca.paths.cert_path).unwrap(),
            calls: Cell::new(0),
            fail_verification: false,
            system_member: false,
            admin_settings: None,
            fail_export: false,
        };
        let repairs = Cell::new(0);
        converge(
            || {
                if repairs.get() != 0 {
                    return Ok(TrustReadiness::Ready);
                }
                let state = probe_with(&ca.paths.cert_path, Some(owner), &fixture)?;
                assert_eq!(state, TrustReadiness::MissingSystemCertificate);
                Ok(state)
            },
            || {
                repairs.set(repairs.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(repairs.get(), 1);
        assert_eq!(fixture.calls.get(), 2);
    }
}
