//! Cgroup v2 helpers.
//!
//! This module implements the naming and path conventions described in RFC 0099.

use std::io::Read;
use std::path::PathBuf;

/// How `locald` anchors its cgroup hierarchy on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupRootStrategy {
    /// Systemd-managed slice hierarchy (preferred).
    Systemd,
    /// Direct management under /sys/fs/cgroup (no systemd).
    Direct,
}

fn root_strategy_from_pid1_comm(comm: &str) -> CgroupRootStrategy {
    if comm.trim() == "systemd" {
        CgroupRootStrategy::Systemd
    } else {
        CgroupRootStrategy::Direct
    }
}

/// Detect which cgroup root strategy should be used on this host.
///
/// This is intentionally a best-effort heuristic:
/// - If PID 1 is systemd, we prefer the Systemd strategy.
/// - Otherwise we fall back to the Direct strategy.
#[must_use]
pub fn detect_root_strategy() -> CgroupRootStrategy {
    // A common failure mode in CI/containers is that systemd artifacts exist on disk,
    // but systemd is not actually PID 1.
    let mut comm = String::new();
    if let Ok(mut file) = std::fs::File::open("/proc/1/comm")
        && file.read_to_string(&mut comm).is_ok()
    {
        return root_strategy_from_pid1_comm(&comm);
    }

    CgroupRootStrategy::Direct
}

/// Returns the cgroup v2 filesystem mountpoint (`/sys/fs/cgroup`).
#[must_use]
pub fn cgroup_fs_root() -> PathBuf {
    PathBuf::from("/sys/fs/cgroup")
}

/// Returns true if the cgroup root for the chosen strategy appears to be established.
///
/// This is used as a safety gate: we avoid setting `linux.cgroupsPath` in OCI bundles
/// until the expected root exists.
#[must_use]
pub fn is_root_ready(strategy: CgroupRootStrategy) -> bool {
    let root = cgroup_fs_root();
    match strategy {
        CgroupRootStrategy::Systemd => root.join("locald.slice").exists(),
        CgroupRootStrategy::Direct => root.join("locald").exists(),
    }
}

fn sanitize_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('-');
        }
    }

    // Trim and collapse common junk.
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

/// Compute an absolute cgroup path (as used by OCI `linux.cgroupsPath`) for a service.
///
/// `service_id` is typically the fully-qualified locald service identifier (often
/// `project:service`). We intentionally include the full identifier to avoid collisions.
#[must_use]
pub fn cgroup_path_for_service(
    strategy: CgroupRootStrategy,
    sandbox_name: &str,
    service_id: &str,
) -> String {
    let sandbox = sanitize_component(sandbox_name);
    // Preserve project/service disambiguation by mapping ':' and other delimiters to '-'.
    let service = sanitize_component(service_id);

    match strategy {
        CgroupRootStrategy::Systemd => {
            format!("/locald.slice/locald-{sandbox}.slice/service-{service}.scope")
        }
        CgroupRootStrategy::Direct => {
            format!("/locald/locald-{sandbox}/service-{service}")
        }
    }
}

/// Compute an absolute cgroup path for an ad-hoc leaf name.
#[must_use]
pub fn cgroup_path_for_leaf(
    strategy: CgroupRootStrategy,
    sandbox_name: &str,
    leaf: &str,
) -> String {
    let sandbox = sanitize_component(sandbox_name);
    let leaf = sanitize_component(leaf);

    match strategy {
        CgroupRootStrategy::Systemd => {
            format!("/locald.slice/locald-{sandbox}.slice/service-{leaf}.scope")
        }
        CgroupRootStrategy::Direct => format!("/locald/locald-{sandbox}/service-{leaf}"),
    }
}

/// Returns a cgroup path for `service_id` if the root is already established.
///
/// This uses `LOCALD_SANDBOX_NAME` when present (and sandbox mode is active), otherwise
/// defaults to the sandbox name "default".
#[must_use]
pub fn maybe_cgroup_path_for_service(service_id: &str) -> Option<String> {
    let strategy = detect_root_strategy();
    if !is_root_ready(strategy) {
        return None;
    }

    let sandbox = crate::env::sandbox_name_or_default();
    Some(cgroup_path_for_service(strategy, &sandbox, service_id))
}

/// Returns a cgroup path for `leaf` if the root is already established.
#[must_use]
pub fn maybe_cgroup_path_for_leaf(leaf: &str) -> Option<String> {
    let strategy = detect_root_strategy();
    if !is_root_ready(strategy) {
        return None;
    }

    let sandbox = crate::env::sandbox_name_or_default();
    Some(cgroup_path_for_leaf(strategy, &sandbox, leaf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn pid1_comm_systemd_selects_systemd() {
        assert_eq!(
            root_strategy_from_pid1_comm("systemd\n"),
            CgroupRootStrategy::Systemd
        );
    }

    proptest! {
        #[test]
        fn pid1_comm_non_systemd_selects_direct(comm in ".*") {
            prop_assume!(comm.trim() != "systemd");
            prop_assert_eq!(root_strategy_from_pid1_comm(&comm), CgroupRootStrategy::Direct);
        }

        #[test]
        fn sanitize_component_produces_safe_nonempty(raw in ".*") {
            let out = sanitize_component(&raw);
            prop_assert!(!out.is_empty());
            prop_assert!(!out.starts_with('-'));
            prop_assert!(!out.ends_with('-'));

            for ch in out.chars() {
                prop_assert!(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
            }
        }

        #[test]
        fn cgroup_paths_use_expected_prefix(sandbox in ".*", service in ".*") {
            let s = cgroup_path_for_service(CgroupRootStrategy::Systemd, &sandbox, &service);
            prop_assert!(s.starts_with("/locald.slice/"));
            prop_assert!(!s.contains(':'));

            let d = cgroup_path_for_service(CgroupRootStrategy::Direct, &sandbox, &service);
            prop_assert!(d.starts_with("/locald/"));
            prop_assert!(!d.contains(':'));
        }

        #[test]
        fn cgroup_paths_have_no_empty_or_parent_components(sandbox in ".*", service in ".*") {
            for path in [
                cgroup_path_for_service(CgroupRootStrategy::Systemd, &sandbox, &service),
                cgroup_path_for_service(CgroupRootStrategy::Direct, &sandbox, &service),
                cgroup_path_for_leaf(CgroupRootStrategy::Systemd, &sandbox, &service),
                cgroup_path_for_leaf(CgroupRootStrategy::Direct, &sandbox, &service),
            ] {
                prop_assert!(path.starts_with('/'));
                prop_assert!(!path.contains("//"));

                let rel = path.trim_start_matches('/');
                for component in rel.split('/') {
                    prop_assert!(!component.is_empty());
                    prop_assert_ne!(component, "..");
                }
            }
        }
    }
}
