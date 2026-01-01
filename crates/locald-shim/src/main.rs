// =============================================================================
// LOCALD-SHIM: Cross-Platform Privileged Helper
// =============================================================================
//
// The shim works on both Linux and macOS using the same architecture:
// - SCM_RIGHTS for FD passing over Unix domain sockets
// - Setuid bit for privilege elevation
//
// Container-related commands (bundle, cgroup) are compile-time gated to Linux.

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use nix::unistd::{Gid, Uid};
use std::env;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf as StdPathBuf;
use std::path::{Path, PathBuf};

// Linux-only imports
#[cfg(target_os = "linux")]
use listeners::get_all;
#[cfg(target_os = "linux")]
use nix::sys::signal::{Signal, kill};
#[cfg(target_os = "linux")]
use nix::unistd::Pid;

// Constants for prctl
// const PR_CAP_AMBIENT: i32 = 47;
// const PR_CAP_AMBIENT_RAISE: i32 = 2;
// const CAP_NET_BIND_SERVICE: u64 = 10;

#[derive(Debug, Parser)]
#[command(name = "locald-shim")]
#[command(about = "Privileged helper for locald (internal protocol)")]
#[command(disable_help_subcommand = true)]
struct Cli {
    /// Print the shim version and exit.
    #[arg(long = "shim-version", action = clap::ArgAction::SetTrue)]
    shim_version: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Execute an OCI bundle. (Linux only - requires libcontainer)
    #[cfg(target_os = "linux")]
    Bundle {
        #[command(subcommand)]
        command: BundleCommand,
    },

    /// Bind a privileged TCP port and pass the FD over a unix socket.
    Bind(BindArgs),

    /// Privileged admin operations.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },

    /// Debug helpers.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[cfg(target_os = "linux")]
#[derive(Debug, Subcommand)]
enum BundleCommand {
    /// Run a bundle as the container init process.
    Run(BundleRunArgs),
}

#[cfg(target_os = "linux")]
#[derive(Debug, Args)]
struct BundleRunArgs {
    /// Path to OCI bundle directory.
    #[arg(long)]
    bundle: PathBuf,

    /// Container identifier.
    #[arg(long)]
    id: String,
}

#[derive(Debug, Args)]
struct BindArgs {
    port: u16,
    socket_path: PathBuf,
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// Synchronize /etc/hosts section for locald domains.
    SyncHosts(AdminSyncHostsArgs),

    /// Non-destructive self-check proving the shim can run with privileges.
    SelfCheck,

    /// Recursively remove a locald-managed directory.
    Cleanup(AdminCleanupArgs),

    /// Cgroup v2 operations. (Linux only)
    #[cfg(target_os = "linux")]
    Cgroup {
        #[command(subcommand)]
        command: AdminCgroupCommand,
    },

    /// Generate (if needed) and install the locald Root CA into the system trust store.
    ///
    /// This runs with privileges and uses the *invoking user's* home directory
    /// (real uid) for CA material at ~/.locald/certs.
    Trust,
}

fn invoking_user_home_dir() -> Result<StdPathBuf> {
    // On both Linux and macOS, we use the real UID (not effective) to find the
    // invoking user's home directory, since the shim runs as setuid root.
    let uid = nix::unistd::getuid();
    let user = nix::unistd::User::from_uid(uid)
        .context("failed to look up invoking user")?
        .ok_or_else(|| anyhow::anyhow!("invoking user not found for uid {uid}"))?;
    Ok(user.dir)
}

fn certs_dir_for_user_home(home: &Path) -> StdPathBuf {
    home.join(".locald").join("certs")
}

fn ensure_root_ca(certs_dir: &Path) -> Result<StdPathBuf> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
        KeyUsagePurpose,
    };

    std::fs::create_dir_all(certs_dir).context("failed to create certs dir")?;

    let ca_cert_path = certs_dir.join("rootCA.pem");
    let ca_key_path = certs_dir.join("rootCA-key.pem");

    let cert_exists = ca_cert_path.exists();
    let key_exists = ca_key_path.exists();
    if cert_exists != key_exists {
        anyhow::bail!(
            "Root CA is partially configured (rootCA.pem/rootCA-key.pem mismatch); run `locald admin setup` to repair HTTPS setup"
        );
    }

    if !cert_exists {
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "locald Development CA");
        dn.push(DnType::OrganizationName, "locald");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

        let key_pair = KeyPair::generate()?;
        let cert = params.self_signed(&key_pair)?;

        {
            use std::io::Write;

            let mut f =
                std::fs::File::create(&ca_cert_path).context("failed to create rootCA.pem")?;
            f.write_all(cert.pem().as_bytes())
                .context("failed to write rootCA.pem")?;
        }

        {
            use std::io::Write;

            let mut f =
                std::fs::File::create(&ca_key_path).context("failed to create rootCA-key.pem")?;
            f.write_all(key_pair.serialize_pem().as_bytes())
                .context("failed to write rootCA-key.pem")?;
        }
    }

    Ok(ca_cert_path)
}

fn admin_trust() -> Result<i32> {
    let home = invoking_user_home_dir()?;
    let certs_dir = certs_dir_for_user_home(&home);
    let ca_cert_path = ensure_root_ca(&certs_dir)?;

    // Platform-specific trust store installation
    #[cfg(target_os = "linux")]
    {
        install_ca_linux(&ca_cert_path)?;
    }

    #[cfg(target_os = "macos")]
    {
        install_ca_macos(&ca_cert_path)?;
    }

    println!("Installed locald Root CA from {}", ca_cert_path.display());
    Ok(0)
}

/// Install CA certificate on Linux using ca_injector with fallback.
#[cfg(target_os = "linux")]
fn install_ca_linux(ca_cert_path: &Path) -> Result<()> {
    let ca_str = ca_cert_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid CA path"))?;

    if let Err(e) = ca_injector::install_ca(ca_str) {
        let msg = e.to_string();

        // Fallback to common Linux trust-store tools.
        if let Err(fallback_err) = install_ca_linux_fallback(ca_cert_path) {
            return Err(fallback_err)
                .context("failed to install CA into system trust store")
                .with_context(|| format!("ca_injector error: {msg}"));
        }
    }

    Ok(())
}

/// Install CA certificate on macOS using the security-framework crate.
///
/// This uses the native Keychain API to add the certificate to the System
/// Keychain with admin trust settings, making it trusted for SSL.
#[cfg(target_os = "macos")]
fn install_ca_macos(ca_cert_path: &Path) -> Result<()> {
    use security_framework::certificate::SecCertificate;
    use security_framework::trust_settings::{Domain, TrustSettings};

    // Read the PEM certificate and convert to DER
    let pem_data = std::fs::read_to_string(ca_cert_path)
        .with_context(|| format!("Failed to read certificate from {}", ca_cert_path.display()))?;

    let der_data = pem_to_der(&pem_data).context("Failed to parse PEM certificate")?;

    // Create SecCertificate from DER data
    let certificate = SecCertificate::from_der(&der_data)
        .context("Failed to create SecCertificate from DER data")?;

    // Add to admin trust settings (requires root/admin privileges)
    // Domain::Admin is the system-wide trust store
    let trust_settings = TrustSettings::new(Domain::Admin);

    // Set trust settings for SSL - this makes the cert trusted for all purposes
    trust_settings
        .set_trust_settings_always(&certificate)
        .context("Failed to add certificate to Keychain")?;

    Ok(())
}

/// Convert PEM-encoded certificate to DER format.
#[cfg(target_os = "macos")]
fn pem_to_der(pem: &str) -> Result<Vec<u8>> {
    // Find the base64 content between BEGIN and END markers
    let start_marker = "-----BEGIN CERTIFICATE-----";
    let end_marker = "-----END CERTIFICATE-----";

    let start = pem
        .find(start_marker)
        .context("PEM missing BEGIN CERTIFICATE marker")?
        + start_marker.len();
    let end = pem
        .find(end_marker)
        .context("PEM missing END CERTIFICATE marker")?;

    let base64_content: String = pem[start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    // Simple base64 decode without external crate
    base64_decode(&base64_content)
}

/// Minimal base64 decoder (no external crate needed for this simple use case).
#[cfg(target_os = "macos")]
fn base64_decode(input: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn decode_char(c: u8) -> Option<u8> {
        if c == b'=' {
            return Some(0);
        }
        ALPHABET.iter().position(|&x| x == c).map(|p| p as u8)
    }

    let bytes: Vec<u8> = input.bytes().collect();
    let mut output = Vec::with_capacity(bytes.len() * 3 / 4);

    for chunk in bytes.chunks(4) {
        if chunk.len() < 4 {
            anyhow::bail!("Invalid base64 padding");
        }

        let a = decode_char(chunk[0]).context("Invalid base64 character")?;
        let b = decode_char(chunk[1]).context("Invalid base64 character")?;
        let c = decode_char(chunk[2]).context("Invalid base64 character")?;
        let d = decode_char(chunk[3]).context("Invalid base64 character")?;

        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            output.push((c << 6) | d);
        }
    }

    Ok(output)
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn install_ca_linux_fallback(cert_path: &Path) -> Result<()> {
    use std::process::Command;

    let anchors_dir = Path::new("/etc/pki/ca-trust/source/anchors");
    if anchors_dir.exists() {
        let target = anchors_dir.join("locald-rootCA.pem");
        std::fs::copy(cert_path, &target)
            .with_context(|| format!("Failed to copy CA to {}", target.display()))?;

        let status = Command::new("update-ca-trust")
            .arg("extract")
            .status()
            .context("Failed to execute update-ca-trust extract")?;

        if !status.success() {
            anyhow::bail!("update-ca-trust extract failed with status: {status}");
        }
        return Ok(());
    }

    let debian_dir = Path::new("/usr/local/share/ca-certificates");
    if debian_dir.exists() {
        let target = debian_dir.join("locald-rootCA.crt");
        std::fs::copy(cert_path, &target)
            .with_context(|| format!("Failed to copy CA to {}", target.display()))?;

        let status = Command::new("update-ca-certificates")
            .status()
            .context("Failed to execute update-ca-certificates")?;

        if !status.success() {
            anyhow::bail!("update-ca-certificates failed with status: {status}");
        }
        return Ok(());
    }

    anyhow::bail!(
        "No known Linux trust-store directories found; install p11-kit-trust / update-ca-trust or equivalent"
    );
}

#[derive(Debug, Args)]
struct AdminSyncHostsArgs {
    domains: Vec<String>,
}

#[derive(Debug, Args)]
struct AdminCleanupArgs {
    path: PathBuf,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Subcommand)]
enum AdminCgroupCommand {
    /// Establish the locald cgroup root (systemd Anchor or direct Driver).
    Setup,

    /// Kill all processes in a locald-managed cgroup and prune it.
    Kill(AdminCgroupKillArgs),
}

#[cfg(target_os = "linux")]
#[derive(Debug, Args)]
struct AdminCgroupKillArgs {
    /// Absolute cgroupsPath (as used by OCI linux.cgroupsPath), e.g.
    /// /locald.slice/locald-default.slice/service-project-web.scope
    #[arg(long)]
    path: String,
}

#[derive(Debug, Subcommand)]
enum DebugCommand {
    /// Show processes listening on a port (requires root for full visibility).
    #[cfg(target_os = "linux")]
    Port(DebugPortArgs),

    /// Print shim capabilities and platform info.
    Info,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Args)]
struct DebugPortArgs {
    port: u16,
}

#[cfg(target_os = "linux")]
fn run_bundle(bundle_path: &Path, container_id: &str) -> Result<i32> {
    let canonical_bundle_path = bundle_path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize bundle path: {bundle_path:?}"))?;

    // Keep container state inside the bundle directory to remain sandboxed/cleanup-able.
    let state_root = canonical_bundle_path.join(".locald-shim-state");
    std::fs::create_dir_all(&state_root).context("Failed to create shim state root")?;

    let mut container = libcontainer::container::builder::ContainerBuilder::new(
        container_id.to_string(),
        libcontainer::syscall::syscall::SyscallType::Linux,
    )
    .with_root_path(&state_root)?
    .as_init(&canonical_bundle_path)
    .with_systemd(false)
    .with_detach(false)
    .build()?;

    container.start()?;

    let init_pid = container
        .pid()
        .ok_or_else(|| anyhow::anyhow!("libcontainer did not report an init pid"))?;

    let init_pid_raw = init_pid.as_raw();

    // Forward termination-ish signals to the container init process.
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGQUIT,
        signal_hook::consts::SIGHUP,
    ])
    .context("Failed to register signal handlers")?;

    std::thread::spawn(move || {
        for sig in signals.forever() {
            // Avoid nix::Pid type mismatches (libcontainer depends on a different nix).
            // libc::kill uses the raw pid_t.
            unsafe {
                let _ = libc::kill(init_pid_raw, sig);
            }
        }
    });

    // Wait for the container init process to exit.
    loop {
        let mut status: libc::c_int = 0;
        let res = unsafe { libc::waitpid(init_pid_raw, &mut status, 0) };

        if res < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }

            let _ = std::fs::remove_dir_all(container.root);
            return Err(anyhow::anyhow!("waitpid failed: {err}"));
        }

        // Decode wait status. This is the traditional waitpid encoding:
        // - exited: (status & 0x7f) == 0, code = (status >> 8) & 0xff
        // - signaled: (status & 0x7f) != 0 && (status & 0x7f) != 0x7f
        let status_i32 = status as i32;
        let low = status_i32 & 0x7f;

        if low == 0 {
            let code = (status_i32 >> 8) & 0xff;
            let _ = std::fs::remove_dir_all(container.root);
            return Ok(code);
        }

        // Stopped/continued: keep waiting.
        if low == 0x7f {
            continue;
        }

        let _ = std::fs::remove_dir_all(container.root);
        return Ok(128 + low);
    }
}

#[cfg(target_os = "linux")]
fn check_port(port: u16) -> Result<()> {
    println!("Checking port {}...", port);

    let listeners = get_all()
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .context("Failed to get system listeners")?;

    let mut found = false;
    let mut printed_header = false;

    for listener in listeners {
        if listener.socket.port() == port {
            if !printed_header {
                println!("{:<10} {:<20} {:<20}", "PID", "NAME", "ADDRESS");
                printed_header = true;
            }
            found = true;
            println!(
                "{:<10} {:<20} {:<20}",
                listener.process.pid, listener.process.name, listener.socket
            );
        }
    }

    if !found {
        println!("No process found listening on port {}.", port);
    }

    Ok(())
}

// =============================================================================
// LINUX-ONLY: Cgroup and systemd support
// =============================================================================
// These functions use Linux-specific APIs (cgroups, /proc, systemd) and are
// not available on macOS. Container support on macOS will use Lima (future).

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn is_systemd_present() -> bool {
    // A common failure mode in CI/containers is that systemd-related files exist on disk,
    // but systemd is not actually PID 1.
    use std::io::Read;

    let mut buf = String::new();
    let Ok(mut f) = std::fs::File::open("/proc/1/comm") else {
        return false;
    };

    if f.read_to_string(&mut buf).is_err() {
        return false;
    }

    buf.trim() == "systemd"
}

#[cfg(target_os = "linux")]
fn ensure_cgroup2_mount() -> Result<()> {
    // Heuristic: cgroup v2 exposes these files at the mount root.
    let root = Path::new("/sys/fs/cgroup");
    if !root.join("cgroup.controllers").exists() {
        anyhow::bail!(
            "/sys/fs/cgroup does not look like a cgroup v2 mount (missing cgroup.controllers)"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn write_file(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("Failed to write {}", path.display()))
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))
}

#[cfg(target_os = "linux")]
fn enable_controllers(parent: &Path) -> Result<()> {
    let controllers_path = parent.join("cgroup.controllers");
    let subtree_control_path = parent.join("cgroup.subtree_control");

    let controllers = read_to_string(&controllers_path)?;
    let mut tokens = Vec::new();
    for c in controllers.split_whitespace() {
        tokens.push(format!("+{c}"));
    }

    if tokens.is_empty() {
        // Not necessarily an error, but it means there's nothing to enable.
        return Ok(());
    }

    let value = tokens.join(" ");
    write_file(&subtree_control_path, &value)
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn cgroup_setup_systemd() -> Result<()> {
    // Write /etc/systemd/system/locald.slice
    let unit_path = Path::new("/etc/systemd/system/locald.slice");
    let unit = "[Unit]\nDescription=Locald Container Runtime Root\n\n[Slice]\nDelegate=yes\n";
    write_file(unit_path, unit)?;

    // Reload systemd so the slice exists.
    let status = std::process::Command::new("systemctl")
        .arg("daemon-reload")
        .status()
        .context("Failed to execute systemctl daemon-reload")?;

    if !status.success() {
        anyhow::bail!("systemctl daemon-reload failed: {status}");
    }

    // Ensure the slice is actually realized in the cgroup tree.
    let status = std::process::Command::new("systemctl")
        .arg("start")
        .arg("locald.slice")
        .status()
        .context("Failed to execute systemctl start locald.slice")?;

    if !status.success() {
        anyhow::bail!("systemctl start locald.slice failed: {status}");
    }

    Ok(())
}

fn cgroup_setup_driver() -> Result<()> {
    ensure_cgroup2_mount()?;
    let root = Path::new("/sys/fs/cgroup");

    // Create /sys/fs/cgroup/locald
    let locald_root = root.join("locald");
    std::fs::create_dir_all(&locald_root)
        .with_context(|| format!("Failed to create {}", locald_root.display()))?;

    // Enable controllers for nesting.
    // 1) Enable at the root
    enable_controllers(root)?;
    // 2) Propagate into /locald
    enable_controllers(&locald_root)?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn cgroup_setup() -> Result<()> {
    if is_systemd_present() {
        cgroup_setup_systemd()?;

        // Best-effort check: locald.slice should now exist.
        let slice = Path::new("/sys/fs/cgroup/locald.slice");
        if !slice.exists() {
            anyhow::bail!(
                "systemd is present but /sys/fs/cgroup/locald.slice did not appear after setup"
            );
        }
        Ok(())
    } else {
        cgroup_setup_driver()
    }
}

#[cfg(target_os = "linux")]
fn validate_locald_cgroup_path(path: &str) -> Result<()> {
    if !path.starts_with('/') {
        anyhow::bail!("cgroup path must be absolute (start with /)");
    }

    // Guardrails: only allow locald-managed roots.
    if !(path.starts_with("/locald.slice/") || path.starts_with("/locald/")) {
        anyhow::bail!("refusing to operate on non-locald cgroup path: {path}");
    }

    // Disallow parent traversal.
    if path.split('/').any(|c| c == "..") {
        anyhow::bail!("cgroup path must not contain ..");
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn cgroup_mount_path(cgroups_path: &str) -> Result<PathBuf> {
    validate_locald_cgroup_path(cgroups_path)?;
    let rel = cgroups_path.trim_start_matches('/');
    Ok(Path::new("/sys/fs/cgroup").join(rel))
}

#[cfg(target_os = "linux")]
fn collect_cgroup_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }

    out.push(root.to_path_buf());
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("Failed to read cgroup directory {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_cgroup_dirs(&path)?);
        }
    }
    Ok(out)
}

#[cfg(target_os = "linux")]
fn kill_all_pids_in_dir(dir: &Path) -> Result<()> {
    let procs_path = dir.join("cgroup.procs");
    if !procs_path.exists() {
        return Ok(());
    }

    let content = read_to_string(&procs_path)?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let pid_i32: i32 = line
            .parse()
            .with_context(|| format!("Invalid pid in {}: {line}", procs_path.display()))?;

        // Best-effort kill; ignore ESRCH.
        match kill(Pid::from_raw(pid_i32), Signal::SIGKILL) {
            Ok(()) => {}
            Err(nix::errno::Errno::ESRCH) => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to SIGKILL pid {pid_i32} from {}: {e}",
                    procs_path.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn cgroup_kill_and_prune(cgroups_path: &str) -> Result<()> {
    ensure_cgroup2_mount()?;
    let dir = cgroup_mount_path(cgroups_path)?;

    if !dir.exists() {
        // Already gone, nothing to do.
        return Ok(());
    }

    let kill_path = dir.join("cgroup.kill");
    if kill_path.exists() {
        // Preferred: kernel-level kill of entire subtree.
        write_file(&kill_path, "1")?;
    } else {
        // Fallback: recursively enumerate cgroup.procs and SIGKILL.
        // Note: this is a best-effort implementation; it is intentionally conservative and
        // avoids freezer semantics.
        let dirs = collect_cgroup_dirs(&dir)?;
        for d in dirs.into_iter().rev() {
            kill_all_pids_in_dir(&d)?;
        }
    }

    // Best-effort cleanup.
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        return Err(anyhow::anyhow!(e)).context("Failed to remove cgroup directory after kill");
    }

    Ok(())
}

use std::fmt::Write;

#[allow(clippy::disallowed_methods)]
fn update_hosts_file(domains: &[String]) -> Result<()> {
    let path = if cfg!(windows) {
        std::path::PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    } else {
        std::path::PathBuf::from("/etc/hosts")
    };

    let current_content = std::fs::read_to_string(&path).context("Failed to read hosts file")?;

    let start_marker = "# BEGIN locald";
    let end_marker = "# END locald";

    let mut new_section = String::new();
    new_section.push_str(start_marker);
    new_section.push('\n');
    for domain in domains {
        let _ = writeln!(new_section, "127.0.0.1 {domain}");
    }
    new_section.push_str(end_marker);

    let new_content = if let Some(start) = current_content.find(start_marker) {
        if let Some(end_idx) = current_content[start..].find(end_marker) {
            let end = start + end_idx;
            // Replace existing section
            let mut output = String::from(&current_content[..start]);
            output.push_str(&new_section);
            output.push_str(&current_content[end + end_marker.len()..]);
            output
        } else {
            // Malformed block, append
            let mut output = String::from(&current_content);
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&new_section);
            output.push('\n');
            output
        }
    } else {
        // Append if not found
        let mut output = String::from(&current_content);
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&new_section);
        output.push('\n');
        output
    };

    std::fs::write(&path, new_content).context("Failed to write hosts file")?;
    Ok(())
}

fn main() -> Result<()> {
    // Sanitize environment: Unset LOCALD_SHIM_ACTIVE to prevent spoofing
    // SAFETY: This is safe because we are single-threaded at this point.
    unsafe {
        env::remove_var("LOCALD_SHIM_ACTIVE");
    }

    let cli = Cli::parse();
    if cli.shim_version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let Some(command) = cli.command else {
        // clap will print help for `--help`; this is just for the no-args case.
        anyhow::bail!("Missing command. Run with --help for usage.");
    };

    // Determine the real user (who ran the shim)
    let _real_uid = Uid::current();
    let _real_gid = Gid::current();

    // We expect to be setuid root
    let effective_uid = Uid::effective();
    if !effective_uid.is_root() {
        // If not root, we cannot perform privileged operations.
        // We must NOT exec locald, as that creates a loop if locald called us.
        eprintln!("locald-shim must be setuid root to function.");
        eprintln!("Current effective UID: {}", effective_uid);
        eprintln!("Please run sudo locald admin setup to fix permissions.");
        std::process::exit(1);
    }

    match command {
        #[cfg(target_os = "linux")]
        Commands::Bundle {
            command: BundleCommand::Run(args),
        } => {
            let code = run_bundle(&args.bundle, &args.id)?;
            std::process::exit(code);
        }
        Commands::Bind(args) => {
            // Case: Bind - Run as root
            // Bind a privileged port and pass the FD to locald via Unix socket.
            // This works on both Linux and macOS using SCM_RIGHTS.
            let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", args.port))
                .with_context(|| format!("Failed to bind to port {}", args.port))?;

            let stream =
                std::os::unix::net::UnixStream::connect(&args.socket_path).with_context(|| {
                    format!("Failed to connect to socket {}", args.socket_path.display())
                })?;

            let fd = listener.as_raw_fd();
            let stream_fd = stream.as_raw_fd();

            let iov = [std::io::IoSlice::new(b"fd")];
            let cmsgs = [nix::sys::socket::ControlMessage::ScmRights(&[fd])];

            nix::sys::socket::sendmsg::<nix::sys::socket::UnixAddr>(
                stream_fd,
                &iov,
                &cmsgs,
                nix::sys::socket::MsgFlags::empty(),
                None,
            )
            .context("Failed to send file descriptor")?;

            Ok(())
        }
        Commands::Admin {
            command: AdminCommand::SyncHosts(args),
        } => {
            // Works on both Linux and macOS - /etc/hosts is the same path.
            update_hosts_file(&args.domains)?;
            Ok(())
        }
        Commands::Admin {
            command: AdminCommand::SelfCheck,
        } => {
            // At this point we already verified effective UID is root.
            // Perform a minimal, non-destructive probe to catch common “looks installed
            // but can’t actually do privileged work” failures (e.g. nosuid setuid ignored).
            //
            // Keep this intentionally conservative: it should not create long-lived state.

            // Platform-specific self-check
            #[cfg(target_os = "linux")]
            #[allow(clippy::disallowed_methods)]
            // Blocking I/O is fine in this synchronous CLI
            {
                // If cgroup v2 is available, ensure we can at least read the controller list.
                let root = Path::new("/sys/fs/cgroup");
                let controllers = root.join("cgroup.controllers");
                if controllers.exists() {
                    std::fs::read_to_string(&controllers)
                        .context("Failed to read /sys/fs/cgroup/cgroup.controllers")?;
                }
            }

            #[cfg(target_os = "macos")]
            #[allow(clippy::disallowed_methods)]
            // Blocking I/O is fine in this synchronous CLI
            {
                // On macOS, verify we can read /etc/hosts as a basic privilege check.
                std::fs::read_to_string("/etc/hosts").context("Failed to read /etc/hosts")?;
            }

            Ok(())
        }
        Commands::Admin {
            command: AdminCommand::Trust,
        } => {
            let code = admin_trust()?;
            std::process::exit(code);
        }
        Commands::Admin {
            command: AdminCommand::Cleanup(args),
        } => {
            let path = &args.path;

            if !path.is_absolute() {
                anyhow::bail!("Cleanup path must be absolute");
            }

            let is_safe = path.components().any(|c| c.as_os_str() == ".locald");
            if !is_safe {
                anyhow::bail!("Cleanup path must be within a .locald directory");
            }

            std::fs::remove_dir_all(path).context("Failed to remove directory")?;
            Ok(())
        }
        #[cfg(target_os = "linux")]
        Commands::Admin {
            command:
                AdminCommand::Cgroup {
                    command: AdminCgroupCommand::Setup,
                },
        } => {
            cgroup_setup()?;
            Ok(())
        }
        #[cfg(target_os = "linux")]
        Commands::Admin {
            command:
                AdminCommand::Cgroup {
                    command: AdminCgroupCommand::Kill(args),
                },
        } => {
            cgroup_kill_and_prune(&args.path)?;
            Ok(())
        }
        #[cfg(target_os = "linux")]
        Commands::Debug {
            command: DebugCommand::Port(args),
        } => {
            check_port(args.port)?;
            Ok(())
        }
        Commands::Debug {
            command: DebugCommand::Info,
        } => {
            println!("locald-shim version: {}", env!("CARGO_PKG_VERSION"));
            println!("Platform: {}", std::env::consts::OS);
            println!("Architecture: {}", std::env::consts::ARCH);

            #[cfg(target_os = "linux")]
            println!("Container support: yes (libcontainer)");
            #[cfg(target_os = "macos")]
            println!("Container support: no (use Lima for containers)");

            Ok(())
        }
    }
}
