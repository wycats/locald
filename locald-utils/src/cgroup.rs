//! Cgroup v2 helpers.
//!
//! This module implements the naming and path conventions described in RFC 0099.

use std::path::{Path, PathBuf};

/// How `locald` anchors its cgroup hierarchy on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupRootStrategy {
    /// Systemd-managed slice hierarchy (preferred).
    Systemd,
    /// Direct management under /sys/fs/cgroup (no systemd).
    Direct,
}

/// Detect which cgroup root strategy should be used on this host.
///
/// This is intentionally a best-effort heuristic:
/// - If systemd is present, we prefer the Systemd strategy.
/// - Otherwise we fall back to the Direct strategy.
#[must_use]
pub fn detect_root_strategy() -> CgroupRootStrategy {
    if Path::new("/run/systemd/system").exists() {
        CgroupRootStrategy::Systemd
    } else {
        CgroupRootStrategy::Direct
    }
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
