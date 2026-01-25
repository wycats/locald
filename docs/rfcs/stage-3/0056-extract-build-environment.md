---
title: "Extract Build Environment & DevContainers"
stage: 0 # 0: Strawman
feature: Developer Experience
---

# RFC: Extract Build Environment & DevContainers

## 1. Summary

This RFC proposes mechanisms to expose the isolated build environment created by Cloud Native Buildpacks (CNB) to the developer. This includes a `locald run` command for ad-hoc execution (analogous to `heroku run`) and a zero-config **SSH Bridge** integration for VS Code.

## 2. Motivation

While `locald up` runs the application in a reproducible environment, developers often need to perform ad-hoc tasks in that same environment:

- Running unit tests (`npm test`, `cargo test`).
- Running linters or formatters.
- Accessing a language REPL (`node`, `irb`, `python`).
- Debugging with tools not installed on the host.

Currently, users must install these tools globally or use a separate tool (like `asdf` or `nvm`) which may drift from the production environment.

By extracting the CNB environment, we ensure that **development, testing, and production** all share the exact same dependencies and system libraries.

## 3. Detailed Design

### 3.1. `locald run`

A new CLI command that spawns a shell or runs a command inside the project's execution environment.

```bash
locald run [service_name] [-- command...]
```

- **Mechanism**: It uses the CNB `launcher` binary found in the build artifacts. The `launcher` is designed to set up the environment variables (from buildpacks) and then exec a process.
- **Execution**:
  - If `locald-shim` is available (Linux), it runs via `runc` for full isolation.
  - If running natively (macOS/Windows), it sets up the environment variables and runs the shell directly (or proxies to the VM).
- **Mounts**: It mounts the current working directory into the container/environment so changes persist.
- **Zero Config**: The user does not need to specify an image or build arguments. `locald` automatically detects the project context, ensures the build artifacts are up-to-date (triggering a build if necessary), and then executes the command.

### 3.2. The VS Code Bridge (Two-Stage Strategy)

We will approach VS Code integration in two stages, moving from a robust MVP to a fully polished, zero-config experience.

#### Stage 1: The SSH Bridge (MVP)

We leverage the standard **Remote - SSH** extension. This requires no custom VS Code extension code, only `locald` automation.

**The Workflow:**

1.  **`locald dev`**:
    - Builds the project image.
    - Starts a container with an **SSH Server** running (injected via a `locald-dev-shim` or buildpack).
    - Mounts the source code.
    - Updates `~/.ssh/config` with a block for `locald-<project>`.
2.  **Connect**: User uses "Remote-SSH: Connect to Host..." and selects the project.

**Pros**: Robust, uses existing tools.
**Cons**: Modifies user's global SSH config, requires key management.

#### Stage 2: The Native Extension (Gold Standard)

We build a small VS Code extension (`locald-vscode`) that implements the **Remote Authority Resolver API**.

**The Workflow:**

1.  **Extension**: Registers a custom remote authority `vscode-remote://locald+<service-name>`.
2.  **Resolution**: When the user clicks "Open in Locald" (added to the status bar):
    - The extension calls `locald tunnel <service-name>`.
    - `locald` ensures the container is running and exposes a stream (or local port) to the container's shell.
    - The extension tells VS Code to connect to this stream.
3.  **Experience**: VS Code installs its server directly into the container over this tunnel.

**Pros**: True "Zero Config", no SSH keys, no config file pollution.
**Cons**: Requires maintaining a VS Code extension.

### 3.3. Configuration (`locald.toml`)

We can add a `[dev]` section to `locald.toml` to customize this environment.

```toml
[service]
name = "my-app"
language = "rust"

[dev]
# Packages to install in the dev environment (if supported by builder)
install = ["ripgrep", "fd-find"]

# VS Code extensions to suggest (future work)
extensions = ["rust-lang.rust-analyzer"]
```

## 4. Implementation Plan (Stage 2)

- [ ] **Implement `locald run`**:
  - Add `Run` variant to `IpcRequest`.
  - Implement `launcher` invocation in `locald-server`.
  - Connect stdin/stdout/stderr via PTY.
- [ ] **Implement SSH Injection**:
  - Create a mechanism to inject `sshd` and authorized keys into the container at runtime.
  - Ensure the container has a valid shell.
- [ ] **Implement SSH Config Management**:
  - Create a helper to manage `~/.ssh/config` entries or provide a custom config file.
- [ ] **VS Code Integration**:
  - Document the workflow.
  - (Optional) Create a small VS Code extension to automate the "Connect" step.

## 5. Drawbacks

- **Shell Overhead**: Running a shell inside a container (via `runc`) has a slight startup cost compared to a native shell.
- **Tooling Friction**: IDEs running on the host might not see the tools inside the container (unless using Remote-SSH).
- **DevContainer Spec**: We lose the automatic processing of `devcontainer.json` (extensions, settings) unless we re-implement it.

## 6. Alternatives

- **Mock Docker Socket**: Implement a fake Docker API.
  - _Why not?_ High effort, large API surface area, fragile.
- **`asdf` / `nix`**: Use host-based package managers.
  - _Why not?_ Doesn't guarantee parity with the production container.

## 7. Unresolved Questions

- **Root vs Non-Root**: CNB images usually run as a non-root user. `locald run` might need to be root to install extra debug tools (e.g., `apt-get install strace`). Should we default to root or the app user?
- **Persistence**: Where do we store shell history? (Probably a volume mount).

```

```
