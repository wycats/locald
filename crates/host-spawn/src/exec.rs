//! Host execution mechanisms and the `SpawnHost` trait.

use std::process::{ExitStatus, Output};

use async_trait::async_trait;
use tokio::process::Command;

use crate::command::{HostCommand, Privilege};
use crate::error::{HostSpawnError, Result};

/// Mechanism for executing commands on the host.
#[derive(Debug, Clone)]
pub enum HostExec {
    /// `flatpak-spawn --host <cmd>` - for Toolbx and Flatpak.
    FlatpakSpawn,
    /// `distrobox-host-exec <cmd>` - for Distrobox.
    DistroboxHostExec,
    /// Custom template: `ssh host "{command}"` or similar.
    /// The `{command}` placeholder is replaced with the full command.
    Template(String),
    /// Direct execution (not containerized).
    Direct,
}

impl HostExec {
    /// Create a Template variant with validation.
    ///
    /// # Errors
    ///
    /// Returns [`HostSpawnError::InvalidTemplate`] if the template doesn't
    /// contain the `{command}` placeholder.
    pub fn template(template: impl Into<String>) -> Result<Self> {
        let template = template.into();
        if !template.contains("{command}") {
            return Err(HostSpawnError::InvalidTemplate);
        }
        Ok(Self::Template(template))
    }
}

/// Trait for executing commands on the host from a container.
#[async_trait]
pub trait SpawnHost {
    /// Execute a command on the host, returning the exit status.
    ///
    /// # Errors
    ///
    /// Returns an error if the command cannot be spawned or if the
    /// host-exec mechanism is unavailable.
    async fn spawn(&self, cmd: &HostCommand) -> Result<ExitStatus>;

    /// Execute and capture output.
    ///
    /// # Errors
    ///
    /// Returns an error if the command cannot be spawned or if the
    /// host-exec mechanism is unavailable.
    async fn output(&self, cmd: &HostCommand) -> Result<Output>;

    /// Get the full command line for logging/debugging.
    fn command_line(&self, cmd: &HostCommand) -> Vec<String>;

    /// Check if this mechanism is available.
    async fn is_available(&self) -> bool;
}

#[async_trait]
impl SpawnHost for HostExec {
    async fn spawn(&self, cmd: &HostCommand) -> Result<ExitStatus> {
        self.build_command(cmd)
            .status()
            .await
            .map_err(HostSpawnError::Spawn)
    }

    async fn output(&self, cmd: &HostCommand) -> Result<Output> {
        self.build_command(cmd)
            .output()
            .await
            .map_err(HostSpawnError::Spawn)
    }

    fn command_line(&self, cmd: &HostCommand) -> Vec<String> {
        let mut parts = Vec::new();

        match self {
            Self::FlatpakSpawn => {
                parts.push("flatpak-spawn".into());
                parts.push("--host".into());
            }
            Self::DistroboxHostExec => {
                parts.push("distrobox-host-exec".into());
            }
            Self::Template(t) => {
                parts.push(format!("[template: {t}]"));
            }
            Self::Direct => {}
        }

        // Add privilege escalation
        match cmd.privilege() {
            Privilege::Pkexec => parts.push("pkexec".into()),
            Privilege::Sudo => {
                parts.push("sudo".into());
                parts.push("--".into());
            }
            Privilege::None => {}
        }

        parts.push(cmd.program().into());
        parts.extend(cmd.args().iter().cloned());
        parts
    }

    async fn is_available(&self) -> bool {
        match self {
            Self::FlatpakSpawn => crate::detect::command_exists("flatpak-spawn").await,
            Self::DistroboxHostExec => crate::detect::command_exists("distrobox-host-exec").await,
            Self::Template(_) | Self::Direct => true,
        }
    }
}

impl HostExec {
    fn build_command(&self, cmd: &HostCommand) -> Command {
        match self {
            Self::FlatpakSpawn => {
                let mut c = Command::new("flatpak-spawn");
                c.arg("--host");
                add_privilege_and_cmd(&mut c, cmd);
                c
            }
            Self::DistroboxHostExec => {
                let mut c = Command::new("distrobox-host-exec");
                add_privilege_and_cmd(&mut c, cmd);
                c
            }
            Self::Template(template) => {
                // Build the inner command as a string with shell escaping
                let inner = build_inner_command_string(cmd);
                let expanded = template.replace("{command}", &inner);
                let mut c = Command::new("sh");
                c.args(["-c", &expanded]);
                c
            }
            Self::Direct => {
                let mut c = match cmd.privilege() {
                    Privilege::Pkexec => {
                        let mut c = Command::new("pkexec");
                        c.arg(cmd.program());
                        c
                    }
                    Privilege::Sudo => {
                        let mut c = Command::new("sudo");
                        c.arg("--").arg(cmd.program());
                        c
                    }
                    Privilege::None => Command::new(cmd.program()),
                };
                c.args(cmd.args());
                c
            }
        }
    }
}

fn add_privilege_and_cmd(c: &mut Command, cmd: &HostCommand) {
    match cmd.privilege() {
        Privilege::Pkexec => {
            c.arg("pkexec").arg(cmd.program());
        }
        Privilege::Sudo => {
            c.arg("sudo").arg("--").arg(cmd.program());
        }
        Privilege::None => {
            c.arg(cmd.program());
        }
    }
    c.args(cmd.args());
}

fn build_inner_command_string(cmd: &HostCommand) -> String {
    let mut parts = Vec::new();

    match cmd.privilege() {
        Privilege::Pkexec => parts.push("pkexec".into()),
        Privilege::Sudo => {
            parts.push("sudo".into());
            parts.push("--".into());
        }
        Privilege::None => {}
    }

    parts.push(shell_escape(cmd.program()));
    for arg in cmd.args() {
        parts.push(shell_escape(arg));
    }

    parts.join(" ")
}

/// Escape a string for safe use in shell commands.
///
/// Uses single-quote wrapping with proper escaping of embedded single quotes.
fn shell_escape(s: &str) -> String {
    // If string is safe (alphanumeric, underscore, dash, dot, slash, colon), use as-is
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':'))
    {
        return s.to_string();
    }

    // Otherwise, wrap in single quotes and escape any embedded single quotes
    // The pattern 'text'\''more' closes the quote, adds an escaped quote, reopens
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_safe_string() {
        assert_eq!(shell_escape("hello"), "hello");
        assert_eq!(shell_escape("/path/to/file"), "/path/to/file");
        assert_eq!(shell_escape("file.txt"), "file.txt");
        assert_eq!(shell_escape("host:port"), "host:port");
    }

    #[test]
    fn shell_escape_string_with_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn shell_escape_string_with_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_string_with_special_chars() {
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
        assert_eq!(shell_escape("; rm -rf /"), "'; rm -rf /'");
    }

    #[test]
    fn flatpak_spawn_command_line() {
        let cmd = HostCommand::builder()
            .program("locald-shim")
            .args(vec!["serve".into()])
            .privilege(Privilege::Pkexec)
            .build();

        let exec = HostExec::FlatpakSpawn;
        let line = exec.command_line(&cmd);

        assert_eq!(
            line,
            vec!["flatpak-spawn", "--host", "pkexec", "locald-shim", "serve"]
        );
    }

    #[test]
    fn distrobox_command_line() {
        let cmd = HostCommand::builder()
            .program("echo")
            .args(vec!["hello".into()])
            .build();

        let exec = HostExec::DistroboxHostExec;
        let line = exec.command_line(&cmd);

        assert_eq!(line, vec!["distrobox-host-exec", "echo", "hello"]);
    }

    #[test]
    fn direct_command_line_with_sudo() {
        let cmd = HostCommand::builder()
            .program("systemctl")
            .args(vec!["restart".into(), "nginx".into()])
            .privilege(Privilege::Sudo)
            .build();

        let exec = HostExec::Direct;
        let line = exec.command_line(&cmd);

        assert_eq!(line, vec!["sudo", "--", "systemctl", "restart", "nginx"]);
    }

    #[test]
    fn template_validation_success() {
        let exec = HostExec::template("ssh host {command}");
        assert!(exec.is_ok());
    }

    #[test]
    fn template_validation_failure() {
        let exec = HostExec::template("ssh host");
        assert!(matches!(exec, Err(HostSpawnError::InvalidTemplate)));
    }

    #[test]
    fn template_inner_command_escapes_args() {
        let cmd = HostCommand::builder()
            .program("echo")
            .args(vec!["hello world".into()])
            .build();

        let inner = build_inner_command_string(&cmd);

        assert_eq!(inner, "echo 'hello world'");
    }
}
