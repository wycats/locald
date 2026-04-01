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
    ///
    /// Verifies actual redirect rules exist in the anchor, not just that the
    /// anchor was created (which could be empty after a partial install failure).
    pub fn is_installed() -> bool {
        std::process::Command::new("pfctl")
            .args(["-a", "com.locald/redirect", "-s", "rules"])
            .output()
            .map(|o| o.status.success() && o.stdout.windows(3).any(|w| w == b"rdr"))
            .unwrap_or(false)
    }
}
