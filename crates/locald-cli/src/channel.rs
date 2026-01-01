//! Build channel detection and version information.
//!
//! locald uses a channel-based release model similar to Rust and Chrome:
//!
//! - **stable**: Default. Only battle-tested, documented features.
//! - **beta**: Features that are ready for the next stable release.
//! - **nightly**: Includes experimental features (plugins, VMM, CNB, containers).
//!
//! The channel is determined at compile time via Cargo features.

// Channel detection utilities are used for version output and feature gating.

/// The release channel this binary was built for.
///
/// This enum is part of the public API for runtime channel detection.
/// Used by tests and intended for future feature-gating UI.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Stable release channel. Only battle-tested features.
    Stable,
    /// Beta channel. Features graduating to stable.
    Beta,
    /// Nightly channel. Includes experimental features.
    Nightly,
}

#[allow(dead_code)]
impl Channel {
    /// Returns the channel this binary was compiled with.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(feature = "channel-nightly") {
            Self::Nightly
        } else if cfg!(feature = "channel-beta") {
            Self::Beta
        } else {
            Self::Stable
        }
    }

    /// Returns the channel name as a string.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
        }
    }

    /// Returns true if this is a nightly build.
    #[must_use]
    pub const fn is_nightly(self) -> bool {
        matches!(self, Self::Nightly)
    }

    /// Returns true if experimental features are available.
    #[must_use]
    pub const fn has_experimental(self) -> bool {
        self.is_nightly()
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Build version string (includes channel suffix for non-stable).
pub const BUILD_VERSION: &str = env!("LOCALD_BUILD_VERSION");

/// Channel name embedded at build time.
pub const BUILD_CHANNEL: &str = env!("LOCALD_CHANNEL");

/// Returns a formatted version string for display.
///
/// Examples:
/// - `locald 0.1.0 (stable)`
/// - `locald 0.1.0-beta (beta)`
/// - `locald 0.1.0-nightly.1735567200 (nightly)`
#[allow(dead_code)]
#[must_use]
pub fn version_string() -> String {
    format!("locald {} ({})", BUILD_VERSION, BUILD_CHANNEL)
}

/// Returns a short version string (just the version number with channel suffix).
#[allow(dead_code)]
#[must_use]
pub const fn short_version() -> &'static str {
    BUILD_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_current_is_consistent() {
        let channel = Channel::current();
        assert_eq!(channel.name(), BUILD_CHANNEL);
    }

    #[test]
    fn version_string_includes_channel() {
        let version = version_string();
        assert!(version.contains(BUILD_CHANNEL));
        assert!(version.starts_with("locald "));
    }
}
