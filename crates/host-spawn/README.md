# host-spawn

Guest-to-host command execution for containerized development environments.

## Overview

`host-spawn` provides type-safe async abstractions for executing commands on the host
system from within containerized environments like Toolbx, Distrobox, or Flatpak.

## Features

- **Container detection**: Automatically detect Toolbx, Distrobox, Flatpak, Docker, and WSL2
- **Host-exec mechanisms**: Support for `flatpak-spawn`, `distrobox-host-exec`, and custom templates
- **Privilege escalation**: Built-in support for `pkexec` and `sudo`
- **Type safety**: Structured command building instead of string interpolation
- **Shell escaping**: Safe argument escaping for template mode
- **Async-first**: Built on `tokio` for non-blocking execution

## Usage

```rust
use host_spawn::{detect_host_exec, HostCommand, Privilege, SpawnHost};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Build a command
    let cmd = HostCommand::builder()
        .program("locald-shim")
        .args(vec!["serve".into()])
        .privilege(Privilege::Pkexec)
        .build();

    // Detect and use the host-exec mechanism
    if let Some(exec) = detect_host_exec().await {
        let status = exec.spawn(&cmd).await?;
        println!("Command exited with: {:?}", status.code());
    }

    Ok(())
}
```

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
