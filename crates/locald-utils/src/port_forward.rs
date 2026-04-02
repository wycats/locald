//! Port forwarding health checks (macOS).
//!
//! Lightweight checks that work without root — suitable for the tray agent
//! and doctor diagnostics.

/// Check whether persistent pfctl rules are configured by reading
/// `/etc/pf.conf` and `/etc/pf.anchors/com.locald`.
///
/// No root needed — both files are world-readable.
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
pub fn is_persistent() -> bool {
    const ANCHOR_FILE: &str = "/etc/pf.anchors/com.locald";
    const PF_CONF: &str = "/etc/pf.conf";
    const RDR_ANCHOR_LINE: &str = "rdr-anchor \"com.locald\"";
    const LOAD_ANCHOR_LINE: &str = "load anchor \"com.locald\" from \"/etc/pf.anchors/com.locald\"";

    std::path::Path::new(ANCHOR_FILE).exists()
        && std::fs::read_to_string(PF_CONF)
            .map(|c| c.contains(RDR_ANCHOR_LINE) && c.contains(LOAD_ANCHOR_LINE))
            .unwrap_or(false)
}
