//! Host command builder.

use bon::Builder;

/// Privilege escalation method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Privilege {
    /// `pkexec` - polkit GUI dialog.
    Pkexec,
    /// `sudo` - terminal password prompt.
    Sudo,
    /// No escalation needed.
    #[default]
    None,
}

/// A command to be executed on the host.
///
/// Use the builder pattern to construct commands:
///
/// ```
/// use host_spawn::{HostCommand, Privilege};
///
/// let cmd = HostCommand::builder()
///     .program("locald-shim")
///     .args(vec!["serve".into()])
///     .privilege(Privilege::Pkexec)
///     .build();
/// ```
#[derive(Debug, Clone, Builder)]
pub struct HostCommand {
    /// The program to execute.
    #[builder(into)]
    program: String,

    /// Arguments to pass to the program.
    #[builder(default)]
    args: Vec<String>,

    /// Privilege escalation mode.
    #[builder(default)]
    privilege: Privilege,
}

impl HostCommand {
    /// Add a single argument after construction (chainable).
    #[must_use]
    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        let arg = arg.into();
        // Defense in depth: reject null bytes
        debug_assert!(!arg.contains('\0'), "Argument contains null byte: {arg:?}");
        self.args.push(arg);
        self
    }

    /// Add multiple arguments after construction (chainable).
    #[must_use]
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for arg in args {
            self = self.with_arg(arg);
        }
        self
    }

    /// Get the program name.
    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Get the arguments.
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Get the privilege mode.
    #[must_use]
    pub const fn privilege(&self) -> Privilege {
        self.privilege
    }
}
