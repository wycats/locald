---
title: Host-Spawn Crate (Guest-to-Host Execution)
stage: 2
feature: Container Development
exo:
    tool: exo rfc create
    protocol: 1
superseded_by: "0138"
---


# RFC 0134: Host-Spawn Crate (Guest-to-Host Execution)

- **Superseded by**: RFC 0138


**Stage**: 2 (Draft)
**Author**: locald team
**Created**: 2026-01-16
**Related**: RFC 0130 (Host Shim Daemon), RFC 0131 (WSL Privileged Operations)

## Summary

Extract the pattern of "running commands on the host from inside a container" into a dedicated workspace crate (`host-spawn`) with proper type discipline. This crate captures detection of container environments, host-exec mechanisms, privilege escalation, and structured command building—replacing the current ad-hoc string-based approach.

> **Scope Note**: This RFC focuses on Linux container environments (Toolbx, Distrobox, Flatpak). WSL2 support (RFC 0131) involves different primitives (Windows named pipes, cross-VM IPC) and is out of scope for the initial implementation. However, the crate's abstractions are designed to accommodate WSL2 as a future `HostExec` variant if the pattern proves useful.

## Motivation

### The Problem

Currently, `locald-utils/src/container.rs` handles guest-to-host execution with:

1. **Ad-hoc string building**: Commands constructed via `format!()` and split with `split_whitespace()`
2. **Scattered detection logic**: Container detection in one place, host-exec selection in another
3. **No type safety**: Easy to construct invalid command combinations
4. **Hard to test**: String-based commands make mocking difficult

Example of current approach:

```rust
// crates/locald-utils/src/container.rs
let shim_command = format!("pkexec {abs_path} serve");
// ...
Command::new("flatpak-spawn")
    .arg("--host")
    .args(shim_command.split_whitespace())  // Fragile!
    .status()
```

### The Pattern Is General

The "container needs to run something on the host" pattern appears across:

| Environment                | Host-Exec Mechanism    | Notes              |
| -------------------------- | ---------------------- | ------------------ |
| **Toolbx**                 | `flatpak-spawn --host` | Fedora/RHEL        |
| **Distrobox**              | `distrobox-host-exec`  | Any distro         |
| **Flatpak**                | `flatpak-spawn --host` | Sandboxed apps     |
| **VS Code Dev Containers** | Via SSH / socket       | Remote development |
| **WSL2**                   | Named pipes / wsl.exe  | Windows host       |

### Goals

1. **Type safety**: Structured command types instead of strings
2. **Testability**: Trait-based design for mocking in tests
3. **Discoverability**: All host operations visible in one enum
4. **Separation of concerns**: "How to reach host" vs "What command to run"
5. **Future reusability**: Clean enough to publish as a standalone crate

## Detailed Design

### Crate Structure

```
crates/
├── host-spawn/           # NEW crate
│   ├── Cargo.toml
│   ├── README.md
│   └── src/
│       ├── lib.rs        # Public API
│       ├── detect.rs     # Container/mechanism detection
│       ├── exec.rs       # SpawnHost implementations
│       └── command.rs    # HostCommand builder
│
├── locald-utils/         # Existing - becomes consumer
│   └── src/
│       ├── container.rs  # Refactored to use host-spawn
│       └── host_commands.rs  # NEW: locald-specific commands
```

### Core Types

```rust
// host-spawn/src/lib.rs

/// Detected container environment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerEnvironment {
    /// Fedora Toolbx (uses flatpak-spawn)
    Toolbx,
    /// Distrobox container
    Distrobox,
    /// Flatpak sandbox
    Flatpak,
    /// Generic Docker/Podman (no host-exec)
    Docker,
    /// WSL2 on Windows
    Wsl2,
    /// Not in a container
    Native,
}

/// Mechanism for executing commands on the host
#[derive(Debug, Clone)]
pub enum HostExec {
    /// `flatpak-spawn --host <cmd>`
    FlatpakSpawn,
    /// `distrobox-host-exec <cmd>`
    DistroboxHostExec,
    /// Custom template: `ssh host "{command}"` or similar
    /// The `{command}` placeholder is replaced with the full command
    Template(String),
    /// Direct execution (not containerized)
    Direct,
}

/// Privilege escalation method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Privilege {
    /// `pkexec` - polkit GUI dialog
    Pkexec,
    /// `sudo` - terminal password prompt
    Sudo,
    /// No escalation needed
    #[default]
    None,
}

/// A command to be executed on the host
#[derive(Debug, Clone)]
pub struct HostCommand {
    program: String,
    args: Vec<String>,
    privilege: Privilege,
}
```

### Command Builder API

Uses the [`bon`](https://crates.io/crates/bon) crate for compile-time checked builders:

- **Modern**: Used by crates.io backend, actively maintained
- **Typestate-based**: Compile-time verification, no runtime panics
- **Flexible**: Works on structs, functions, and methods
- **Ergonomic**: `#[builder(into)]` for automatic conversions

```rust
use bon::Builder;

/// A command to be executed on the host
#[derive(Debug, Clone, Builder)]
pub struct HostCommand {
    #[builder(into)]
    program: String,

    #[builder(default)]
    args: Vec<String>,

    #[builder(default)]
    privilege: Privilege,
}

impl HostCommand {
    /// Add arguments after construction (chainable)
    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments after construction (chainable)
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    // Getters remain the same
    pub fn program(&self) -> &str { &self.program }
    pub fn args(&self) -> &[String] { &self.args }
    pub fn privilege(&self) -> Privilege { self.privilege }
}
```

Usage:

```rust
// Simple command
let cmd = HostCommand::builder()
    .program("locald-shim")
    .build();

// With privilege (Into<String> works automatically)
let cmd = HostCommand::builder()
    .program(shim_path.display())  // PathBuf Display works via Into<String>
    .privilege(Privilege::Pkexec)
    .build();

// With args after build
let cmd = HostCommand::builder()
    .program("sudo")
    .privilege(Privilege::Sudo)
    .build()
    .with_args(["--", "locald-shim", "start"]);
```

### The SpawnHost Trait

```rust
use crate::error::{HostSpawnError, Result};

/// Trait for executing commands on the host from a container
pub trait SpawnHost {
    /// Execute a command on the host, returning the exit status
    fn spawn(&self, cmd: &HostCommand) -> Result<std::process::ExitStatus>;

    /// Execute and capture output
    fn output(&self, cmd: &HostCommand) -> Result<std::process::Output>;

    /// Get the full command line for logging/debugging
    fn command_line(&self, cmd: &HostCommand) -> Vec<String>;

    /// Check if this mechanism is available
    fn is_available(&self) -> bool;
}
```

### HostExec Implementations

```rust
impl SpawnHost for HostExec {
    fn spawn(&self, cmd: &HostCommand) -> std::io::Result<std::process::ExitStatus> {
        self.build_command(cmd).status()
    }

    fn output(&self, cmd: &HostCommand) -> std::io::Result<std::process::Output> {
        self.build_command(cmd).output()
    }

    fn command_line(&self, cmd: &HostCommand) -> Vec<String> {
        // Returns the full command for logging
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
                parts.push(format!("[template: {}]", t));
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

    fn is_available(&self) -> bool {
        match self {
            Self::FlatpakSpawn => command_exists("flatpak-spawn"),
            Self::DistroboxHostExec => command_exists("distrobox-host-exec"),
            Self::Template(_) => true, // Assume template is valid
            Self::Direct => true,
        }
    }
}

impl HostExec {
    fn build_command(&self, cmd: &HostCommand) -> std::process::Command {
        match self {
            Self::FlatpakSpawn => {
                let mut c = Command::new("flatpak-spawn");
                c.arg("--host");
                self.add_privilege_and_cmd(&mut c, cmd);
                c
            }
            Self::DistroboxHostExec => {
                let mut c = Command::new("distrobox-host-exec");
                self.add_privilege_and_cmd(&mut c, cmd);
                c
            }
            Self::Template(template) => {
                // Build the inner command as a string
                let inner = self.build_inner_command_string(cmd);
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

    fn add_privilege_and_cmd(&self, c: &mut Command, cmd: &HostCommand) {
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

    fn build_inner_command_string(&self, cmd: &HostCommand) -> String {
        let mut parts = Vec::new();
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
        parts.join(" ")
    }
}
```

### Detection Functions

Detection follows a tiered hierarchy based on specification stability:

**Tier 1 - Official Standards:**

- systemd [Container Interface](https://systemd.io/CONTAINER_INTERFACE/) defines `$container` env var and `/run/systemd/container` file
- Flatpak documents `$FLATPAK_ID` and `/.flatpak-info`

**Tier 2 - De Facto Standards:**

- `/run/.containerenv` - Podman/Buildah (parseable INI format)
- `/.dockerenv` - Docker marker file

**Tier 3 - Implementation Details** (undocumented, may change):

- `$TOOLBOX_PATH` - Toolbx
- `$DISTROBOX_ENTER_PATH` - Distrobox

```rust
// host-spawn/src/detect.rs

/// Detect the current container environment
pub fn detect_container() -> ContainerEnvironment {
    // Tier 1: Official standards (most reliable)

    // Flatpak - officially documented env var
    if std::env::var("FLATPAK_ID").is_ok() {
        return ContainerEnvironment::Flatpak;
    }

    // Also check the official Flatpak marker file
    if Path::new("/.flatpak-info").exists() {
        return ContainerEnvironment::Flatpak;
    }

    // systemd Container Interface: user-readable file (preferred over env var
    // because $container is only set on PID 1, not inherited)
    if let Ok(manager) = std::fs::read_to_string("/run/systemd/container") {
        let manager = manager.trim();
        // Map known manager names to our enum
        return match manager {
            "toolbox" => ContainerEnvironment::Toolbx,
            "podman" | "docker" | "lxc" | "lxc-libvirt" => ContainerEnvironment::Docker,
            _ => ContainerEnvironment::Docker, // Unknown but containerized
        };
    }

    // WSL2 - Microsoft-defined
    if std::env::var("WSL_DISTRO_NAME").is_ok() {
        return ContainerEnvironment::Wsl2;
    }

    // Tier 2: De facto standards
    if Path::new("/run/.containerenv").exists() {
        // Could parse this file for more info (engine, name, rootless flag)
        return ContainerEnvironment::Docker;
    }
    if Path::new("/.dockerenv").exists() {
        return ContainerEnvironment::Docker;
    }

    // Tier 3: Implementation-specific env vars (undocumented but stable in practice)
    if std::env::var("TOOLBOX_PATH").is_ok() {
        return ContainerEnvironment::Toolbx;
    }
    if std::env::var("DISTROBOX_ENTER_PATH").is_ok() {
        return ContainerEnvironment::Distrobox;
    }

    ContainerEnvironment::Native
}

/// Auto-detect the best host-exec mechanism for the current environment
pub fn detect_host_exec() -> Option<HostExec> {
    match detect_container() {
        ContainerEnvironment::Toolbx | ContainerEnvironment::Flatpak => {
            if command_exists("flatpak-spawn") {
                return Some(HostExec::FlatpakSpawn);
            }
        }
        ContainerEnvironment::Distrobox => {
            if command_exists("distrobox-host-exec") {
                return Some(HostExec::DistroboxHostExec);
            }
        }
        ContainerEnvironment::Native => {
            return Some(HostExec::Direct);
        }
        _ => {}
    }

    // Fallback: try available mechanisms
    if command_exists("flatpak-spawn") {
        return Some(HostExec::FlatpakSpawn);
    }
    if command_exists("distrobox-host-exec") {
        return Some(HostExec::DistroboxHostExec);
    }

    None
}

/// Check if running in any container environment
pub fn is_containerized() -> bool {
    detect_container() != ContainerEnvironment::Native
}
```

### locald Integration (in locald-utils)

```rust
// crates/locald-utils/src/host_commands.rs

use host_spawn::{HostCommand, Privilege};
use std::path::PathBuf;

/// Structured commands that locald needs to run on the host
pub enum LocaldHostCommand {
    /// Start the privileged shim daemon
    StartShim {
        shim_path: PathBuf,
        foreground: bool,
        idle_timeout: Option<u64>,
    },
    /// Run `locald admin setup`
    AdminSetup {
        locald_path: PathBuf,
        skip_cgroup: bool,
    },
    /// Sync domains to /etc/hosts
    SyncHosts {
        shim_path: PathBuf,
        domains: Vec<String>,
    },
    /// Install polkit policy
    InstallPolkit {
        shim_path: PathBuf,
    },
}

impl LocaldHostCommand {
    /// Convert to a generic HostCommand
    pub fn into_host_command(self) -> HostCommand {
        match self {
            Self::StartShim { shim_path, foreground, idle_timeout } => {
                let mut cmd = HostCommand::new(shim_path.display().to_string())
                    .arg("serve")
                    .privilege(Privilege::Pkexec);
                if foreground {
                    cmd = cmd.arg("--foreground");
                }
                if let Some(timeout) = idle_timeout {
                    cmd = cmd.args(["--idle-timeout", &timeout.to_string()]);
                }
                cmd
            }
            Self::AdminSetup { locald_path, skip_cgroup } => {
                let mut cmd = HostCommand::new(locald_path.display().to_string())
                    .args(["admin", "setup"])
                    .privilege(Privilege::Sudo);
                if skip_cgroup {
                    cmd = cmd.arg("--skip-cgroup");
                }
                cmd
            }
            Self::SyncHosts { shim_path, domains } => {
                HostCommand::new(shim_path.display().to_string())
                    .args(["admin", "sync-hosts"])
                    .args(domains)
                    .privilege(Privilege::None) // Shim is already privileged
            }
            Self::InstallPolkit { shim_path } => {
                HostCommand::new(shim_path.display().to_string())
                    .args(["admin", "install-polkit"])
                    .privilege(Privilege::Sudo)
            }
        }
    }
}
```

### Refactored container.rs

```rust
// crates/locald-utils/src/container.rs (after refactor)

use host_spawn::{detect_host_exec, HostExec, SpawnHost};
use crate::host_commands::LocaldHostCommand;

pub fn start_host_shim(config: &ContainerConfig) -> Result<()> {
    let shim_path = crate::shim::find()?
        .ok_or_else(|| anyhow::anyhow!("Could not find locald-shim"))?;

    let cmd = LocaldHostCommand::StartShim {
        shim_path,
        foreground: false,
        idle_timeout: None,
    }.into_host_command();

    // Determine host-exec mechanism
    let host_exec = if let Some(template) = &config.host_exec {
        HostExec::Template(template.clone())
    } else {
        detect_host_exec().ok_or_else(|| anyhow::anyhow!(
            "No host-exec mechanism available"
        ))?
    };

    info!("Starting host shim: {:?}", host_exec.command_line(&cmd));

    let status = host_exec.spawn(&cmd)?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to start host shim: exit code {:?}", status.code())
    }
}
```

## Implementation Plan (Stage 2)

- [ ] Create `crates/host-spawn/` with Cargo.toml (dependencies: `bon`, `thiserror`)
- [ ] Implement `ContainerEnvironment` detection (tiered approach from Detection Functions)
- [ ] Implement `HostExec` and `SpawnHost` trait
- [ ] Implement `HostCommand` with `bon::Builder` derive
- [ ] Add comprehensive unit tests using DI-based command execution
- [ ] Create `LocaldHostCommand` enum in `locald-utils`
- [ ] Refactor `container.rs` to use new types
- [ ] Update `handlers.rs` admin commands to use structured types
- [ ] Add integration tests for container detection
- [ ] Security audit of shell escaping in Template mode

## Drawbacks

1. **More code initially**: Structured approach requires more boilerplate than string interpolation
2. **Internal overhead**: Adds a new crate to the workspace (though very small)
3. **Learning curve**: Developers must learn the new API

## Alternatives

### Alternative 1: Just improve current code without new crate

Keep everything in `locald-utils/src/container.rs` but add better types:

```rust
struct ShimCommand { path: PathBuf, args: Vec<String> }
```

**Rejected because**: Still mixes host-exec logic with locald-specific concerns. Not reusable.

### Alternative 2: Use an existing crate

Search for existing guest-to-host execution crates.

**Rejected because**: No mature crate exists for this specific pattern. The closest (`flatpak-spawn` bindings) are too narrow.

### Alternative 3: Publish to crates.io immediately

Make `host-spawn` a public crate from day one.

**Deferred**: Start as workspace crate, validate the design with real usage, then publish if appropriate.

## Unresolved Questions

~~1. **Crate name**: `host-spawn` vs `container-host` vs `guest-exec`?~~
**Resolved**: `host-spawn`

~~2. **WSL2 support**: Should this crate include WSL2 named-pipe support, or defer to RFC 0131?~~
**Resolved**: Defer to RFC 0131. The crate abstractions can accommodate WSL2 as a future `HostExec` variant, but initial implementation focuses on Linux containers.

~~3. **Async support**: Should we provide `async fn spawn()` variants?~~
**Resolved**: Yes. The crate is async-first using `tokio` and `async-trait`. This aligns with the workspace's linting rules against blocking I/O (`clippy.toml` disallows `std::process::Command::new` and `std::fs::read_to_string`). The `SpawnHost` trait uses `async fn spawn()` and `async fn output()` methods.

~~4. **Error types**: Use `std::io::Error` or custom error type?~~
**Resolved**: Use `thiserror` for a custom `HostSpawnError` enum (see Error Handling section below). This aligns with RFC 0058's principle of structured error enums and matches patterns in `locald-utils` and `locald-core`.

## Error Handling

Following the project's established patterns (RFC 0058: "Use Error Enums, not strings"), `host-spawn` defines a structured error type:

```rust
// host-spawn/src/error.rs

use thiserror::Error;

/// Errors that can occur during host command execution
#[derive(Debug, Error)]
pub enum HostSpawnError {
    /// No mechanism available to execute commands on the host
    #[error("no host-exec mechanism available (tried flatpak-spawn, distrobox-host-exec)")]
    NoHostExecMechanism,

    /// The host-exec mechanism exists but failed to execute
    #[error("host-exec mechanism '{mechanism}' is not available")]
    MechanismUnavailable { mechanism: &'static str },

    /// I/O error during command execution
    #[error("failed to spawn command: {0}")]
    Spawn(#[source] std::io::Error),

    /// Command executed but returned non-zero exit code
    #[error("command failed with exit code {code:?}")]
    NonZeroExit { code: Option<i32> },

    /// Template substitution error
    #[error("invalid template: missing {{command}} placeholder")]
    InvalidTemplate,

    /// Argument contains characters that cannot be safely escaped for shell
    #[error("argument contains unescapable characters: {arg:?}")]
    CommandEscapingFailed { arg: String },
}

/// Convenience type alias
pub type Result<T> = std::result::Result<T, HostSpawnError>;
```

This design:

- Allows callers to match on specific error variants (e.g., retry logic for `NonZeroExit`)
- Preserves the underlying `io::Error` via `#[source]`
- Provides actionable error messages

### Error Granularity

The error types distinguish between different failure modes:

| Error                   | Meaning                                              | Typical Response                    |
| ----------------------- | ---------------------------------------------------- | ----------------------------------- |
| `NoHostExecMechanism`   | No flatpak-spawn, distrobox-host-exec, or template   | Guide user to configure `host_exec` |
| `MechanismUnavailable`  | Binary exists but isn't functional                   | Try next mechanism                  |
| `Spawn(io::Error)`      | OS-level spawn failure (permissions, missing binary) | Show system error                   |
| `NonZeroExit { code }`  | Command ran but failed                               | Show command output                 |
| `InvalidTemplate`       | User template missing `{command}`                    | Guide user to fix config            |
| `CommandEscapingFailed` | Argument contains unescapable characters             | Show problematic argument           |

## Security Considerations

### Shell Injection Risks

The `HostExec::Template` variant uses `sh -c` to execute user-configured commands:

```rust
Self::Template(template) => {
    let inner = self.build_inner_command_string(cmd);
    let expanded = template.replace("{command}", &inner);
    Command::new("sh").args(["-c", &expanded])
}
```

**Risk**: If `build_inner_command_string` doesn't properly escape arguments, malicious input could execute arbitrary commands.

### Mitigation Strategy

1. **Arguments are NOT shell-interpreted in non-Template modes**:
   - `FlatpakSpawn` and `DistroboxHostExec` pass arguments directly to `Command::arg()`, bypassing shell interpretation entirely.

2. **Template mode uses proper shell escaping**:

   ```rust
   fn build_inner_command_string(&self, cmd: &HostCommand) -> String {
       let mut parts = Vec::new();
       // ... privilege handling ...
       parts.push(shell_escape(cmd.program()));
       for arg in cmd.args() {
           parts.push(shell_escape(arg));
       }
       parts.join(" ")
   }

   /// Escape a string for safe use in shell commands
   fn shell_escape(s: &str) -> String {
       // If string is safe (alphanumeric, underscore, dash, dot, slash), use as-is
       if s.chars().all(|c| c.is_ascii_alphanumeric()
           || matches!(c, '_' | '-' | '.' | '/' | ':')) {
           return s.to_string();
       }
       // Otherwise, wrap in single quotes and escape any embedded single quotes
       format!("'{}'", s.replace('\'', "'\\''"))
   }
   ```

3. **Validation at construction**:

   ```rust
   impl HostExec {
       pub fn template(s: impl Into<String>) -> Result<Self, HostSpawnError> {
           let s = s.into();
           if !s.contains("{command}") {
               return Err(HostSpawnError::InvalidTemplate);
           }
           Ok(Self::Template(s))
       }
   }
   ```

4. **Argument validation** (optional, for defense in depth):
   ```rust
   impl HostCommand {
       pub fn arg(mut self, arg: impl Into<String>) -> Self {
           let arg = arg.into();
           // Reject null bytes (would truncate in shell)
           debug_assert!(!arg.contains('\0'), "Argument contains null byte");
           self.args.push(arg);
           self
       }
   }
   ```

### Security Invariants

- **Direct modes** (`FlatpakSpawn`, `DistroboxHostExec`, `Direct`): Arguments passed to OS without shell interpretation.
- **Template mode**: All arguments are shell-escaped before substitution.
- **Privilege escalation**: Only `pkexec` and `sudo` are supported; arbitrary privilege commands are not allowed.

## Testing Strategy

### Dependency Injection Approach

Rather than mocking `std::process::Command`, we use dependency injection via the `SpawnHost` trait. This allows tests to inject alternative implementations:

```rust
/// Trait for command execution - enables DI for testing
pub trait CommandRunner: Send + Sync {
    fn run(&self, cmd: &std::process::Command) -> std::io::Result<std::process::ExitStatus>;
    fn output(&self, cmd: &std::process::Command) -> std::io::Result<std::process::Output>;
}

/// Default implementation that actually runs commands
#[derive(Default)]
pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&self, cmd: &std::process::Command) -> std::io::Result<std::process::ExitStatus> {
        // Clone and run - Command doesn't impl Clone, so we rebuild
        unimplemented!("Real implementation uses internal build_command")
    }
    fn output(&self, cmd: &std::process::Command) -> std::io::Result<std::process::Output> {
        unimplemented!("Real implementation uses internal build_command")
    }
}

/// Test implementation that records commands without executing
pub struct RecordingRunner {
    pub commands: std::sync::Mutex<Vec<RecordedCommand>>,
    pub response: Box<dyn Fn(&RecordedCommand) -> std::io::Result<ExitStatus> + Send + Sync>,
}

pub struct RecordedCommand {
    pub program: String,
    pub args: Vec<String>,
}
```

### Test Examples

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_spawn_builds_correct_command() {
        let cmd = HostCommand::builder()
            .program("locald-shim")
            .args(vec!["serve".into()])
            .privilege(Privilege::Pkexec)
            .build();

        let exec = HostExec::FlatpakSpawn;
        let command_line = exec.command_line(&cmd);

        assert_eq!(command_line, vec![
            "flatpak-spawn", "--host", "pkexec", "locald-shim", "serve"
        ]);
    }

    #[test]
    fn template_escapes_arguments_with_spaces() {
        let cmd = HostCommand::builder()
            .program("/path/to/shim")
            .args(vec!["arg with spaces".into()])
            .build();

        let exec = HostExec::Template("ssh host {command}".into());
        let command_line = exec.command_line(&cmd);

        // Should be properly escaped
        assert_eq!(command_line, vec![
            "[template: ssh host {command}]",
            "/path/to/shim",
            "'arg with spaces'"
        ]);
    }

    #[test]
    fn template_escapes_single_quotes() {
        let cmd = HostCommand::builder()
            .program("echo")
            .args(vec!["it's".into()])
            .build();

        let inner = HostExec::Direct.build_inner_command_string(&cmd);
        assert_eq!(inner, "echo 'it'\\''s'");
    }

    #[test]
    fn direct_mode_no_escaping_needed() {
        // Direct mode passes to Command::arg() which doesn't interpret shell
        let cmd = HostCommand::builder()
            .program("echo")
            .args(vec!["$HOME".into(), "; rm -rf /".into()])
            .build();

        let exec = HostExec::Direct;
        // These are passed directly to execve, not interpreted by shell
        let command_line = exec.command_line(&cmd);
        assert_eq!(command_line, vec!["echo", "$HOME", "; rm -rf /"]);
    }

    #[test]
    fn detect_container_caches_result() {
        // First call does filesystem checks
        let env1 = detect_container();
        // Subsequent calls return cached value
        let env2 = detect_container();
        assert_eq!(env1, env2);
    }
}
```

### Integration Tests

```rust
// tests/container_detection.rs

#[test]
#[ignore] // Run manually in actual container environments
fn detect_toolbx_environment() {
    if std::env::var("TOOLBOX_PATH").is_err() {
        eprintln!("Skipping: not in Toolbx");
        return;
    }
    assert_eq!(detect_container(), ContainerEnvironment::Toolbx);
    assert!(matches!(detect_host_exec(), Some(HostExec::FlatpakSpawn)));
}
```

## Migration Guide

### Phase 1: Add host-spawn crate (non-breaking)

1. Create `crates/host-spawn/` with types from this RFC
2. Add `host-spawn` as dependency to `locald-utils`
3. No changes to existing public APIs yet

### Phase 2: Create locald-specific wrappers

1. Create `crates/locald-utils/src/host_commands.rs` with `LocaldHostCommand`
2. Add conversion methods that use new types internally

### Phase 3: Migrate container.rs internally

**Before:**

```rust
// crates/locald-utils/src/container.rs
let shim_command = format!("pkexec {abs_path} serve");
Command::new("flatpak-spawn")
    .arg("--host")
    .args(shim_command.split_whitespace())
    .status()
```

**After:**

```rust
// crates/locald-utils/src/container.rs
use host_spawn::{HostExec, SpawnHost};
use crate::host_commands::LocaldHostCommand;

let cmd = LocaldHostCommand::StartShim {
    shim_path: abs_path,
    foreground: false,
    idle_timeout: None,
}.into_host_command();

let exec = detect_host_exec().ok_or(...)?;
exec.spawn(&cmd)?;
```

### Phase 4: Migrate CLI handlers

**File**: `crates/locald-cli/src/handlers.rs`

Current pattern (duplicated logic):

```rust
fn ensure_host_shim_internal(...) {
    if command_exists("flatpak-spawn") { ... }
    else if command_exists("distrobox-host-exec") { ... }
}
```

Replace with:

```rust
fn ensure_host_shim_internal(...) {
    let exec = config.host_exec
        .as_ref()
        .map(|t| HostExec::Template(t.clone()))
        .or_else(|| detect_host_exec())
        .ok_or_else(|| anyhow!("No host-exec mechanism"))?;

    let cmd = LocaldHostCommand::StartShim { ... }.into_host_command();
    exec.spawn(&cmd)?;
}
```

### Phase 5: Migrate CLI utils

**File**: `crates/locald-cli/src/utils.rs`

Remove duplicate `substitute_host_exec_template` function; use `HostExec::Template` instead.

### Migration Checklist

| File                            | Function                        | Status |
| ------------------------------- | ------------------------------- | ------ |
| `locald-utils/src/container.rs` | `start_host_shim`               | ◯      |
| `locald-utils/src/container.rs` | `is_probably_container`         | ◯      |
| `locald-utils/src/container.rs` | `substitute_template`           | ◯      |
| `locald-utils/src/container.rs` | `command_exists`                | ◯      |
| `locald-cli/src/handlers.rs`    | `ensure_host_shim_internal`     | ◯      |
| `locald-cli/src/handlers.rs`    | Container detection             | ◯      |
| `locald-cli/src/utils.rs`       | `substitute_host_exec_template` | ◯      |
| `locald-cli/src/utils.rs`       | `is_in_container`               | ◯      |

## Dependency Justification

### Why `bon`?

The [`bon`](https://crates.io/crates/bon) crate provides compile-time checked builders with minimal boilerplate:

| Approach             | Pros                                                    | Cons                              |
| -------------------- | ------------------------------------------------------- | --------------------------------- |
| **Manual builders**  | No dependencies                                         | Verbose, easy to forget fields    |
| **`derive_builder`** | Popular                                                 | Runtime errors (returns `Result`) |
| **`typed-builder`**  | Compile-time                                            | Limited to structs                |
| **`bon`**            | Compile-time, works on functions too, used by crates.io | Newer crate                       |

**Decision**: Use `bon` because:

1. **Compile-time safety**: Missing required fields are caught at compile time, not runtime
2. **Ergonomic**: `#[builder(into)]` handles `impl Into<String>` automatically
3. **Production-proven**: Used by the crates.io backend
4. **Low footprint**: Only a proc-macro, no runtime dependency
5. **General-purpose**: Follows the principle of using established crates for common patterns

## Future Possibilities

1. **crates.io publication**: Once battle-tested in locald, publish for broader use
2. **WSL2 integration**: Add Windows named-pipe support for RFC 0131
3. **VS Code Dev Container support**: Detect and integrate with devcontainer CLI
4. **Podman Desktop integration**: Detect Podman Desktop's host-exec mechanism
5. **Async execution**: Add tokio-based async variants