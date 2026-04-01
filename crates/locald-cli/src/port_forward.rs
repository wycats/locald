/// macOS port forwarding via pfctl.
///
/// Installs redirect rules so that traffic to privileged ports (80, 443)
/// on localhost is transparently forwarded to locald's unprivileged proxy
/// ports (8080, 8443). Rules live under a `com.locald/redirect` anchor.
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

    const ANCHOR: &str = "com.locald/redirect";

    fn loopback() -> Ip {
        Ip::from(Ipv4Addr::LOCALHOST)
    }

    /// Install pfctl redirect rules for privileged port forwarding.
    ///
    /// Idempotent: safe to call multiple times.
    pub fn install() -> Result<()> {
        let mut pf = PfCtl::new().context(
            "Failed to open /dev/pf. This requires root — run with `sudo locald admin setup`.",
        )?;

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
    #[allow(dead_code)]
    pub fn is_installed() -> bool {
        let Ok(pf) = PfCtl::new() else {
            return false;
        };
        // If we can read rules from our anchor, they're installed.
        // PfCtl doesn't expose a direct "list rules" API, but we can check
        // if the anchor exists by trying to flush (which succeeds even if empty).
        // A more reliable check: try to add the anchor — if it already exists, good.
        drop(pf);

        // Pragmatic check: see if pfctl can show our anchor.
        std::process::Command::new("pfctl")
            .args(["-s", "Anchors", "-a", "com.locald"])
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    }
}
