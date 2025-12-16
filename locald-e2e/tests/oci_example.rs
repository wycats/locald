#![allow(clippy::disallowed_methods)]

use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

fn effective_uid() -> Option<u32> {
    // Linux-only best-effort UID discovery without adding extra deps.
    // Format: "Uid:\t<real>\t<effective>\t<saved>\t<fs>"
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let mut parts = rest.split_whitespace();
            let _real = parts.next()?;
            let effective = parts.next()?;
            return effective.parse::<u32>().ok();
        }
    }
    None
}

fn unprivileged_userns_available() -> bool {
    // If we're root, user namespaces are typically available regardless of the sysctl.
    if effective_uid() == Some(0) {
        return true;
    }

    // Ubuntu 23.10+ / 24.04 often ships with this enabled by default.
    // When set, rootless container creation via user namespaces will fail.
    if let Ok(val) =
        std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
    {
        if val.trim() == "1" {
            return false;
        }
    }

    // Best-effort probe: can we create a user namespace via util-linux unshare?
    // Use `-r` (map-root-user) to avoid needing newuidmap/newgidmap.
    let true_path = if std::path::Path::new("/usr/bin/true").exists() {
        "/usr/bin/true"
    } else {
        "/bin/true"
    };

    let Ok(status) = Command::new("unshare")
        .args(["-Ur", true_path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    else {
        return false;
    };

    status.success()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn target_dir(workspace_root: &std::path::Path) -> PathBuf {
    let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") else {
        return workspace_root.join("target");
    };

    let dir = PathBuf::from(dir);
    if dir.is_absolute() {
        dir
    } else {
        workspace_root.join(dir)
    }
}

fn bin_path(debug_dir: &std::path::Path, name: &str) -> PathBuf {
    debug_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(unix)]
fn is_setuid_root(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let is_root = metadata.uid() == 0;
    let is_setuid = (metadata.mode() & 0o4000) != 0;
    is_root && is_setuid
}

fn find_privileged_shim(candidate_dirs: &[PathBuf]) -> Option<PathBuf> {
    // 1) Look in explicitly-provided directories (e.g., target/debug).
    #[cfg(unix)]
    {
        for dir in candidate_dirs {
            let candidate = dir.join("locald-shim");
            if is_setuid_root(&candidate) {
                return Some(candidate);
            }
        }
    }

    // 2) Fall back to PATH.
    #[cfg(unix)]
    {
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                let candidate = dir.join("locald-shim");
                if is_setuid_root(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

#[test]
fn test_oci_example_integration() -> Result<()> {
    let force = std::env::var_os("LOCALD_E2E_FORCE_OCI_EXAMPLE").is_some();
    let ci = std::env::var_os("CI").is_some();

    // This test requires a container-capable environment (namespaces/mounts/cgroups) and network
    // access to pull an image. We run it by default in CI, but make it opt-in for local dev.
    if !ci && !force {
        eprintln!(
            "Skipping oci-example integration test (opt-in outside CI).\n\
             Set LOCALD_E2E_FORCE_OCI_EXAMPLE=1 to run it locally."
        );
        return Ok(());
    }

    // GitHub runners (and many dev machines) may restrict unprivileged user namespaces.
    // Rootless/libcontainer execution depends on user namespaces, so skip rather than
    // failing the entire Rust Checks job.
    if !force && !unprivileged_userns_available() {
        eprintln!(
            "Skipping oci-example integration test: unprivileged user namespaces are not available.\n\
             (If this is Ubuntu 24.04+, check kernel.apparmor_restrict_unprivileged_userns=1.)\n\
             Set LOCALD_E2E_FORCE_OCI_EXAMPLE=1 to force running it."
        );
        return Ok(());
    }

    // 1. Build the example.
    // IMPORTANT: Do not rebuild `locald-shim` here.
    // In CI we run `sudo .../locald admin setup` before tests, which installs a
    // setuid-root shim. Rebuilding `locald-shim` as the unprivileged test user
    // would overwrite that binary and strip the setuid bit.
    let status = Command::new("cargo")
        .args(&["build", "-p", "oci-example"])
        .status()?;
    assert!(status.success(), "Failed to build oci-example");

    // 2. Locate binaries.
    // CI sets CARGO_TARGET_DIR when running coverage, so avoid hard-coding `target/debug`.
    let workspace_root = workspace_root();
    let debug_dir = target_dir(&workspace_root).join("debug");

    let example_binary = bin_path(&debug_dir, "oci-example");

    assert!(example_binary.exists(), "oci-example binary not found");

    // 2b. Require a privileged shim (setuid root). This is the project contract:
    // unprivileged daemon + privileged leaf shim.
    let privileged_shim = find_privileged_shim(&[debug_dir.clone()]);
    if privileged_shim.is_none() && !force {
        if ci {
            anyhow::bail!(
                "oci-example integration test requires a setuid-root locald-shim. Run `sudo target/debug/locald admin setup` (or `sudo locald admin setup`) in CI setup before running tests. If CI sets CARGO_TARGET_DIR, the shim must be installed for that target directory."
            );
        }

        eprintln!(
            "Skipping oci-example integration test: privileged locald-shim (setuid root) not found.\n\
             Run `sudo target/debug/locald admin setup` (repo dev) or `sudo locald admin setup` (installed) to enable container execution.\n\
             Set LOCALD_E2E_FORCE_OCI_EXAMPLE=1 to force running it."
        );
        return Ok(());
    }

    // 3. Create a temp dir for the test execution
    let temp_dir = tempfile::tempdir()?;

    // 4. Run the example
    // We run it inside the temp dir so its "tmp-oci-example" folder is isolated
    let mut cmd = Command::new(&example_binary);

    // Reuse a stable OCI layout cache directory so CI doesn't re-download layers each run.
    // In GitHub Actions, we additionally cache this directory via actions/cache.
    let layout_cache_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| debug_dir.clone())
        .join(".cache")
        .join("locald")
        .join("oci-layout");
    cmd.env("LOCALD_OCI_LAYOUT_CACHE_DIR", &layout_cache_dir);

    if let Some(shim_path) = privileged_shim {
        if let Some(shim_dir) = shim_path.parent() {
            let mut paths = vec![shim_dir.to_path_buf()];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            cmd.env("PATH", std::env::join_paths(paths)?);
        }
    }

    let status = cmd
        .current_dir(temp_dir.path())
        .arg("alpine:latest")
        .arg("echo")
        .arg("Integration Test Success")
        .status()?;

    assert!(status.success(), "oci-example failed to execute");

    Ok(())
}
