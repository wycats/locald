/// macOS port forwarding via pfctl.
///
/// Installs redirect rules so that traffic to privileged ports (80, 443)
/// on localhost is transparently forwarded to locald's unprivileged proxy
/// ports (8080, 8443). Rules live under a `com.locald` anchor.
///
/// Requires root to open `/dev/pf`.
#[cfg(target_os = "macos")]
pub mod macos {
    use anyhow::{Context, Result};
    use pfctl::{
        AnchorKind, Endpoint, Ip, PfCtl, Proto, RedirectRuleAction, RedirectRuleBuilder,
        RulesetKind,
    };
    use std::net::Ipv4Addr;

    const ANCHOR: &str = "com.locald";

    fn loopback() -> Ip {
        Ip::from(Ipv4Addr::LOCALHOST)
    }

    /// Install pfctl redirect rules for privileged port forwarding.
    ///
    /// Idempotent: safe to call multiple times.
    pub fn install() -> Result<()> {
        let mut pf = PfCtl::new()
            .context("Failed to open /dev/pf. This requires root — run `locald admin setup`.")?;

        pf.try_enable().context("Failed to enable pf")?;
        pf.try_add_anchor(ANCHOR, AnchorKind::Redirect)
            .context("Failed to add pfctl anchor")?;

        // Clear any existing rules in our anchor before adding fresh ones.
        let _ = pf.flush_rules(ANCHOR, RulesetKind::Redirect);

        let http = RedirectRuleBuilder::default()
            .action(RedirectRuleAction::Redirect)
            .interface("lo0")
            .proto(Proto::Tcp)
            .to(Endpoint::new(loopback(), 80))
            .redirect_to(Endpoint::new(loopback(), 8080))
            .build()
            .context("Failed to build HTTP redirect rule")?;

        let https = RedirectRuleBuilder::default()
            .action(RedirectRuleAction::Redirect)
            .interface("lo0")
            .proto(Proto::Tcp)
            .to(Endpoint::new(loopback(), 443))
            .redirect_to(Endpoint::new(loopback(), 8443))
            .build()
            .context("Failed to build HTTPS redirect rule")?;

        pf.add_redirect_rule(ANCHOR, &http)
            .context("Failed to add HTTP redirect rule")?;
        pf.add_redirect_rule(ANCHOR, &https)
            .context("Failed to add HTTPS redirect rule")?;

        Ok(())
    }

    /// Remove pfctl redirect rules installed by locald.
    #[allow(dead_code)]
    pub fn remove() -> Result<()> {
        let mut pf = PfCtl::new().context("Failed to open /dev/pf (need root)")?;

        let _ = pf.flush_rules(ANCHOR, RulesetKind::Redirect);
        let _ = pf.try_remove_anchor(ANCHOR, AnchorKind::Redirect);

        Ok(())
    }

    /// Check whether locald's pfctl redirect rules are installed.
    ///
    /// Verifies actual redirect (nat/rdr) rules exist in the anchor.
    /// Requires root to read `/dev/pf` — appropriate for `admin setup` context.
    pub fn is_installed() -> bool {
        std::process::Command::new("pfctl")
            .args(["-a", "com.locald", "-s", "nat"])
            .output()
            .map(|o| o.status.success() && o.stdout.windows(3).any(|w| w == b"rdr"))
            .unwrap_or(false)
    }

    const ANCHOR_FILE: &str = "/etc/pf.anchors/com.locald";
    const PF_CONF: &str = "/etc/pf.conf";
    const RDR_ANCHOR_LINE: &str = "rdr-anchor \"com.locald\"";
    const LOAD_ANCHOR_LINE: &str = "load anchor \"com.locald\" from \"/etc/pf.anchors/com.locald\"";

    const ANCHOR_RULES: &str = "\
rdr pass on lo0 proto tcp from any to 127.0.0.1 port 80 -> 127.0.0.1 port 8080
rdr pass on lo0 proto tcp from any to 127.0.0.1 port 443 -> 127.0.0.1 port 8443
";

    /// Install persistent pfctl rules via a pf anchor and pf.conf modification.
    ///
    /// Writes `/etc/pf.anchors/com.locald` with redirect rules, then adds
    /// anchor references to `/etc/pf.conf` so rules survive reboot.
    ///
    /// Idempotent: safe to call multiple times.
    #[allow(clippy::disallowed_methods)]
    pub fn install_persistent() -> Result<()> {
        // Step 1: Write the anchor file.
        std::fs::write(ANCHOR_FILE, ANCHOR_RULES)
            .context("Failed to write /etc/pf.anchors/com.locald")?;

        // Step 2: Add anchor references to pf.conf (idempotent).
        let content = std::fs::read_to_string(PF_CONF).context("Failed to read /etc/pf.conf")?;

        let has_rdr = content.lines().any(|l| l.trim() == RDR_ANCHOR_LINE);
        let has_load = content.lines().any(|l| l.trim() == LOAD_ANCHOR_LINE);

        if has_rdr && has_load {
            return Ok(());
        }

        let mut lines: Vec<String> = content.lines().map(String::from).collect();

        if !has_rdr {
            // Insert after the last rdr-anchor line (typically "com.apple/*").
            let pos = lines
                .iter()
                .rposition(|l| l.trim().starts_with("rdr-anchor"));
            let insert_at = pos.map_or(lines.len(), |i| i + 1);
            lines.insert(insert_at, RDR_ANCHOR_LINE.to_string());
        }

        if !has_load {
            // Insert after the last load anchor line (typically "com.apple").
            let pos = lines
                .iter()
                .rposition(|l| l.trim().starts_with("load anchor"));
            let insert_at = pos.map_or(lines.len(), |i| i + 1);
            lines.insert(insert_at, LOAD_ANCHOR_LINE.to_string());
        }

        let mut new_content = lines.join("\n");
        if !new_content.ends_with('\n') {
            new_content.push('\n');
        }

        // Atomic write: temp file + rename to avoid corruption on interrupt.
        let tmp = format!("{PF_CONF}.locald.tmp");
        std::fs::write(&tmp, &new_content).context("Failed to write temporary pf.conf")?;
        std::fs::rename(&tmp, PF_CONF).context("Failed to rename temporary pf.conf")?;

        Ok(())
    }

    /// Remove persistent pfctl anchor from pf.conf and the anchor file.
    #[allow(clippy::disallowed_methods)]
    pub fn remove_persistent() -> Result<()> {
        // Remove anchor file.
        if std::path::Path::new(ANCHOR_FILE).exists() {
            std::fs::remove_file(ANCHOR_FILE)
                .context("Failed to remove /etc/pf.anchors/com.locald")?;
        }

        // Remove our lines from pf.conf.
        let content = std::fs::read_to_string(PF_CONF).context("Failed to read /etc/pf.conf")?;

        let filtered: Vec<&str> = content
            .lines()
            .filter(|l| l.trim() != RDR_ANCHOR_LINE && l.trim() != LOAD_ANCHOR_LINE)
            .collect();

        let mut new_content = filtered.join("\n");
        if !new_content.ends_with('\n') {
            new_content.push('\n');
        }

        let tmp = format!("{PF_CONF}.locald.tmp");
        std::fs::write(&tmp, &new_content).context("Failed to write temporary pf.conf")?;
        std::fs::rename(&tmp, PF_CONF).context("Failed to rename temporary pf.conf")?;

        Ok(())
    }

    /// Check whether persistent pfctl rules are configured.
    ///
    /// Verifies both the anchor file exists and pf.conf has the load anchor
    /// line (which is what actually triggers rule loading on boot).
    #[allow(clippy::disallowed_methods)]
    pub fn is_persistent() -> bool {
        std::path::Path::new(ANCHOR_FILE).exists()
            && std::fs::read_to_string(PF_CONF)
                .map(|c| c.contains(RDR_ANCHOR_LINE) && c.contains(LOAD_ANCHOR_LINE))
                .unwrap_or(false)
    }
}
