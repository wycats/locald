//! Installation transaction for the authenticated macOS privileged helper.

use anyhow::{Context, Result};
use locald_helper_protocol::code_signing;
use locald_helper_protocol::{
    AUTHORITY_PATH, HELPER_PATH, HELPER_PLIST_PATH, HelperAuthority, MACH_SERVICE, load_authority,
};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const ROOT_UID: u32 = 0;
const WHEEL_GID: u32 = 0;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
struct InstallPaths {
    authority: PathBuf,
    helper: PathBuf,
    plist: PathBuf,
}

impl InstallPaths {
    fn system() -> Self {
        Self {
            authority: PathBuf::from(AUTHORITY_PATH),
            helper: PathBuf::from(HELPER_PATH),
            plist: PathBuf::from(HELPER_PLIST_PATH),
        }
    }

    #[cfg(test)]
    fn under(root: &Path) -> Self {
        Self {
            authority: root.join("Library/Application Support/locald/helper-authority.json"),
            helper: root.join("Library/PrivilegedHelperTools/com.locald.helper"),
            plist: root.join("Library/LaunchDaemons/com.locald.helper.plist"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FileOwner {
    uid: u32,
    gid: u32,
}

impl FileOwner {
    const ROOT_WHEEL: Self = Self {
        uid: ROOT_UID,
        gid: WHEEL_GID,
    };
}

trait Launchctl {
    fn bootout(&self, service: &str) -> Result<()>;
    fn bootstrap(&self, domain: &str, plist: &Path) -> Result<()>;
}

struct SystemLaunchctl;

impl Launchctl for SystemLaunchctl {
    fn bootout(&self, service: &str) -> Result<()> {
        let output = std::process::Command::new("launchctl")
            .args(["bootout", service])
            .output()
            .context("failed to run launchctl bootout for helper")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No such process") || stderr.contains("Could not find service") {
                tracing::debug!(service, "existing helper was not loaded");
            } else {
                anyhow::bail!("launchctl bootout {service} failed: {}", stderr.trim());
            }
        }
        Ok(())
    }

    fn bootstrap(&self, domain: &str, plist: &Path) -> Result<()> {
        let output = std::process::Command::new("launchctl")
            .args(["bootstrap", domain])
            .arg(plist)
            .output()
            .context("failed to run launchctl bootstrap for helper")?;
        if !output.status.success() {
            anyhow::bail!(
                "launchctl bootstrap {domain} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

/// Build authority for the exact executable running explicit setup.
pub fn authority_for_current_executable(configured_uid: u32) -> Result<HelperAuthority> {
    let executable_path = std::env::current_exe()
        .context("could not resolve the locald executable used for setup")?
        .canonicalize()
        .context("could not canonicalize the locald executable used for setup")?;
    let designated_requirement = code_signing::designated_requirement_for_path(&executable_path)
        .context("could not derive the locald code requirement")?;
    HelperAuthority::new(
        configured_uid,
        designated_requirement,
        executable_path,
        env!("LOCALD_BUILD_VERSION").to_string(),
    )
    .context("could not construct helper authority")
}

/// Atomically install helper authority, executable, and `LaunchDaemon`, then restart it.
pub fn install(helper_bytes: &[u8], authority: &HelperAuthority) -> Result<()> {
    let paths = InstallPaths::system();
    install_with(
        &paths,
        FileOwner::ROOT_WHEEL,
        helper_bytes,
        authority,
        &SystemLaunchctl,
    )?;
    load_authority(&paths.authority)
        .context("installed helper authority failed its security validation")?;
    Ok(())
}

/// Perform the authenticated, non-mutating setup postflight.
pub fn probe() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("could not create helper probe runtime")?;
    runtime
        .block_on(locald_server::helper_client::probe_helper())
        .context("privileged helper postflight failed")
}

/// Remove helper authority together with the helper and `LaunchDaemon`.
pub fn remove() -> Result<()> {
    remove_with(&InstallPaths::system(), &SystemLaunchctl)
}

fn install_with(
    paths: &InstallPaths,
    owner: FileOwner,
    helper_bytes: &[u8],
    authority: &HelperAuthority,
    launchctl: &impl Launchctl,
) -> Result<()> {
    authority.validate().context("invalid helper authority")?;
    let mut authority_bytes =
        serde_json::to_vec_pretty(authority).context("could not serialize helper authority")?;
    authority_bytes.push(b'\n');
    let plist = render_launch_daemon_plist();

    ensure_parent(&paths.authority, owner)?;
    ensure_parent(&paths.helper, owner)?;
    ensure_parent(&paths.plist, owner)?;
    // Publish the fail-closed helper first. If setup is interrupted before the
    // authority follows, the new helper refuses to open its listener.
    atomic_install_file(&paths.helper, helper_bytes, 0o755, owner)?;
    atomic_install_file(&paths.authority, &authority_bytes, 0o600, owner)?;
    atomic_install_file(&paths.plist, plist.as_bytes(), 0o644, owner)?;

    launchctl.bootout("system/com.locald.helper")?;
    launchctl.bootstrap("system", &paths.plist)?;
    Ok(())
}

fn remove_with(paths: &InstallPaths, launchctl: &impl Launchctl) -> Result<()> {
    launchctl.bootout("system/com.locald.helper")?;
    let mut failures = Vec::new();
    for path in [&paths.plist, &paths.helper, &paths.authority] {
        if let Err(error) = remove_installed_file(path) {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "could not remove privileged helper installation: {}",
            failures.join("; ")
        )
    }
}

fn remove_installed_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                File::open(parent)?.sync_all()?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_parent(path: &Path, owner: FileOwner) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("installation path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("could not create {}", parent.display()))?;
    let metadata = std::fs::symlink_metadata(parent)
        .with_context(|| format!("could not inspect {}", parent.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "installation directory is not a real directory: {}",
            parent.display()
        );
    }
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755))?;
    nix::unistd::chown(
        parent,
        Some(nix::unistd::Uid::from_raw(owner.uid)),
        Some(nix::unistd::Gid::from_raw(owner.gid)),
    )?;
    Ok(())
}

fn atomic_install_file(path: &Path, bytes: &[u8], mode: u32, owner: FileOwner) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("installation path has no parent: {}", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("installation path has no file name: {}", path.display()))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.tmp.{}.{}",
        file_name.to_string_lossy(),
        std::process::id(),
        sequence
    ));

    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("could not create {}", temporary.display()))?;
        file.write_all(bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        nix::unistd::fchown(
            &file,
            Some(nix::unistd::Uid::from_raw(owner.uid)),
            Some(nix::unistd::Gid::from_raw(owner.gid)),
        )?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("could not publish {}", path.display()))?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn render_launch_daemon_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{MACH_SERVICE}</string>
    <key>MachServices</key>
    <dict>
        <key>{MACH_SERVICE}</key>
        <true/>
    </dict>
    <key>ProgramArguments</key>
    <array>
        <string>{HELPER_PATH}</string>
    </array>
    <key>StandardOutPath</key>
    <string>/var/log/com.locald.helper.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/com.locald.helper.log</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::expect_used,
    clippy::let_underscore_must_use
)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingLaunchctl {
        calls: Mutex<Vec<String>>,
    }

    impl Launchctl for RecordingLaunchctl {
        fn bootout(&self, service: &str) -> Result<()> {
            self.calls
                .lock()
                .expect("launchctl calls")
                .push(format!("bootout {service}"));
            Ok(())
        }

        fn bootstrap(&self, domain: &str, plist: &Path) -> Result<()> {
            self.calls
                .lock()
                .expect("launchctl calls")
                .push(format!("bootstrap {domain} {}", plist.display()));
            Ok(())
        }
    }

    fn owner() -> FileOwner {
        FileOwner {
            uid: nix::unistd::geteuid().as_raw(),
            gid: nix::unistd::getegid().as_raw(),
        }
    }

    fn authority(version: &str) -> HelperAuthority {
        HelperAuthority::new(
            501,
            "identifier locald".to_string(),
            PathBuf::from("/usr/local/bin/locald"),
            version.to_string(),
        )
        .expect("valid authority")
    }

    fn assert_metadata(path: &Path, expected_mode: u32, expected_owner: FileOwner) {
        let metadata = std::fs::metadata(path).expect("installed metadata");
        assert_eq!(metadata.mode() & 0o7777, expected_mode);
        assert_eq!(metadata.uid(), expected_owner.uid);
        assert_eq!(metadata.gid(), expected_owner.gid);
    }

    #[test]
    fn transaction_installs_exact_files_modes_and_launchctl_order() {
        let root = tempfile::tempdir().expect("temporary install root");
        let paths = InstallPaths::under(root.path());
        let launchctl = RecordingLaunchctl::default();
        let owner = owner();

        install_with(&paths, owner, b"helper-v1", &authority("0.1.0"), &launchctl)
            .expect("install helper");

        assert_eq!(
            std::fs::read(&paths.helper).expect("helper bytes"),
            b"helper-v1"
        );
        assert_metadata(&paths.helper, 0o755, owner);
        assert_metadata(&paths.plist, 0o644, owner);
        assert_metadata(&paths.authority, 0o600, owner);
        assert_eq!(
            locald_helper_protocol::parse_authority(
                &std::fs::read(&paths.authority).expect("authority bytes")
            )
            .expect("authority parse"),
            authority("0.1.0")
        );
        assert_eq!(
            *launchctl.calls.lock().expect("launchctl calls"),
            vec![
                "bootout system/com.locald.helper".to_string(),
                format!("bootstrap system {}", paths.plist.display()),
            ]
        );
    }

    #[test]
    fn rerun_atomically_replaces_stale_helper_and_authority() {
        let root = tempfile::tempdir().expect("temporary install root");
        let paths = InstallPaths::under(root.path());
        let launchctl = RecordingLaunchctl::default();
        let owner = owner();

        install_with(&paths, owner, b"helper-v1", &authority("0.1.0"), &launchctl)
            .expect("first install");
        install_with(&paths, owner, b"helper-v2", &authority("0.2.0"), &launchctl)
            .expect("repair install");

        assert_eq!(
            std::fs::read(&paths.helper).expect("helper bytes"),
            b"helper-v2"
        );
        let installed = locald_helper_protocol::parse_authority(
            &std::fs::read(&paths.authority).expect("authority bytes"),
        )
        .expect("authority parse");
        assert_eq!(installed.executable_version, "0.2.0");
        assert_metadata(&paths.helper, 0o755, owner);
        assert_metadata(&paths.authority, 0o600, owner);
        assert!(
            std::fs::read_dir(paths.helper.parent().expect("helper parent"))
                .expect("helper parent entries")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp."))
        );
    }

    #[test]
    fn teardown_removes_helper_authority_and_plist_idempotently() {
        let root = tempfile::tempdir().expect("temporary install root");
        let paths = InstallPaths::under(root.path());
        let launchctl = RecordingLaunchctl::default();
        install_with(&paths, owner(), b"helper", &authority("0.1.0"), &launchctl)
            .expect("install helper");

        remove_with(&paths, &launchctl).expect("remove helper");
        remove_with(&paths, &launchctl).expect("repeat helper removal");
        assert!(!paths.helper.exists());
        assert!(!paths.authority.exists());
        assert!(!paths.plist.exists());
    }

    #[test]
    fn invalid_authority_is_rejected_before_files_or_commands() {
        let root = tempfile::tempdir().expect("temporary install root");
        let paths = InstallPaths::under(root.path());
        let launchctl = RecordingLaunchctl::default();
        let owner = owner();
        let mut invalid = authority("0.1.0");
        invalid.console_user_uid = 0;

        assert!(install_with(&paths, owner, b"helper", &invalid, &launchctl).is_err());
        assert!(!paths.helper.exists());
        assert!(launchctl.calls.lock().expect("launchctl calls").is_empty());
    }

    #[test]
    fn symlinked_install_directory_is_rejected() {
        let root = tempfile::tempdir().expect("temporary install root");
        let elsewhere = tempfile::tempdir().expect("symlink target");
        let paths = InstallPaths::under(root.path());
        let authority_parent = paths.authority.parent().expect("authority parent");
        std::fs::create_dir_all(authority_parent.parent().expect("application support"))
            .expect("create application support");
        std::os::unix::fs::symlink(elsewhere.path(), authority_parent)
            .expect("create authority symlink");

        let error = install_with(
            &paths,
            owner(),
            b"helper",
            &authority("0.1.0"),
            &RecordingLaunchctl::default(),
        )
        .expect_err("symlinked authority directory must fail");
        assert!(error.to_string().contains("not a real directory"));
    }

    #[test]
    fn launch_daemon_has_only_the_helper_mach_service_and_executable() {
        let plist = render_launch_daemon_plist();
        assert!(plist.contains("<string>com.locald.helper</string>"));
        assert!(plist.contains(HELPER_PATH));
        assert!(!plist.contains("setup"));
        assert!(!plist.contains("trust"));
    }
}
