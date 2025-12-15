---
title: "North Star: The Zero-Friction Development Workflow"
stage: 0 # 0: Strawman
feature: Developer Experience
---

# RFC 0057: North Star - The Zero-Friction Development Workflow

## 1. Summary

This RFC defines the "North Star" vision for the `locald` development experience. It consolidates our designs for **Complex App Configuration** (RFC 0054/0055) and **Extract Build Environment** (RFC 0056) into a single, coherent workflow.

The goal is to allow a developer to clone a repo, run `locald dev`, and immediately start coding in VS Code with a fully configured, production-parity environment—without installing language runtimes, databases, or configuring SSH keys.

## 2. The Vision: "It Just Works"

### 2.1. The User Journey

1.  **Clone**: `git clone https://github.com/my-org/complex-app`
2.  **Start**: `locald dev`
    - `locald` detects the project structure.
    - It builds the app using Cloud Native Buildpacks (CNB).
    - It spins up dependencies (Postgres, Redis) defined in `locald.toml`.
    - It starts the app in "Development Mode" (with hot reloading if available).
3.  **Edit**: The user opens VS Code.
    - They click "Open in Locald Environment" (via the `locald` extension).
    - VS Code reloads _inside_ the container.
    - Rust Analyzer / ESLint / Pyright just work.
    - `localhost:8080` forwards to the app.

### 2.2. The "Magic" Behind the Scenes

- **No Dockerfile**: The environment is derived from the source code.
- **No Docker Compose**: `locald` manages the orchestration.
- **No SSH Config**: The connection is handled via a direct tunnel between VS Code and `locald`.

## 3. Detailed Design

### 3.1. Configuration: `locald.toml`

The `locald.toml` is the single source of truth. It handles "Complex Apps" by allowing multiple services and implicit dependencies.

```toml
# locald.toml

[project]
name = "shop-app"

# 1. The Main Application (Implicit Type="cnb")
[service "backend"]
path = "backend" # Source directory
language = "rust" # Optional hint
depends_on = ["db", "cache"]

    [service.backend.dev]
    # Tools injected into the dev environment
    install = ["cargo-watch", "diesel-cli"]
    # VS Code extensions to auto-install in the remote
    extensions = ["rust-lang.rust-analyzer", "tamasfe.even-better-toml"]

# 2. A Database Dependency (Implicit Type="image")
[service "db"]
image = "postgres:15"
ports = ["5432"] # Internal port
# Environment variables injected into dependents automatically:
# DATABASE_URL=postgres://postgres:password@db:5432/db

# 3. A Cache Dependency
[service "cache"]
image = "redis:7"
```

### 3.2. The Architecture

The system consists of three main components working in unison:

1.  **`locald-server` (The Brain)**:

    - Manages the lifecycle of containers (using `runc` + `locald-shim`).
    - Provides a **Tunnel API** (`locald tunnel <service>`) that streams stdin/stdout to a shell inside the container.
    - Watches files and triggers rebuilds/restarts.

2.  **`locald-vscode` (The Bridge)**:

    - A lightweight VS Code extension.
    - Implements the `RemoteAuthorityResolver` API.
    - Registers `vscode-remote://locald+<service-name>`.
    - When activated, it spawns `locald tunnel <service-name>` and pipes the connection to VS Code's remote server.

3.  **The Container (The Environment)**:
    - Built via CNB.
    - Contains the app runtime + dev tools (injected via `[dev.install]`).
    - Runs a "Dev Shim" (PID 1) that handles signal forwarding and keeps the container alive even if the app crashes.

### 3.3. The VS Code Extension Protocol

The `locald-vscode` extension does _not_ need to know about Docker or SSH. It only needs to know how to talk to `locald`.

**Resolution Flow:**

1.  User clicks "Open Remote... -> Locald: backend".
2.  VS Code activates `locald-vscode`.
3.  Extension calls `locald tunnel backend`.
4.  `locald` verifies the container is running (starts it if not).
5.  `locald` executes `locald-shim exec <container-id> /bin/sh`.
6.  `locald` proxies the stdio of that shell back to the extension.
7.  VS Code sends its "Server Bootstrap" script down the pipe.
8.  The VS Code Server starts inside the container and takes over.

## 4. Implementation Roadmap

### Phase 1: The Foundation (Current)

- [x] CNB Build Support.
- [x] Basic `locald up` (Process Management).
- [ ] `locald.toml` "Complex App" parsing (Dependencies).

### Phase 2: The Tunnel (Next)

- [ ] Implement `locald tunnel <service>` CLI command.
  - Must support interactive PTY or raw stream.
  - Must connect to the running `runc` container.
- [ ] Implement `locald exec` (prerequisite for tunnel).

### Phase 3: The Extension

- [ ] Scaffold `locald-vscode` extension.
- [ ] Implement `RemoteAuthorityResolver`.
- [ ] Add "Status Bar" item to show current `locald` status.

### Phase 4: Polish

- [ ] "Dev Mode" in `locald`: File watching and auto-rebuilds.
- [ ] Port Forwarding: Auto-detect ports opened by the app and forward them to VS Code.

## 5. Alignment with Axioms

- **Zero Config**: The user clones and runs. No `Dockerfile`, no `docker-compose.yml`, no `.devcontainer.json` (unless they want advanced customization).
- **Tooling Independence**: The logic lives in `locald-server`. The VS Code extension is just a dumb pipe. We could write a JetBrains plugin or a TUI using the same `locald tunnel` command.
- **Production Parity**: The dev environment _is_ the production environment (plus some debug tools). No more "it works on my machine".

## 6. FAQ

**Q: How does this differ from DevContainers?**
A: DevContainers rely on Docker and a `Dockerfile`. We rely on CNB and `locald.toml`. We remove the need to maintain a Dockerfile that drifts from your production build.

**Q: Can I still use `docker-compose`?**
A: You don't need to. `locald.toml` replaces it for development orchestration, with the added benefit of being tightly integrated with the build system.

**Q: What if I need root access?**
A: CNB images are non-root by default. We can configure the "Dev Shim" to allow `sudo` (via `locald-shim` capabilities) for installing ephemeral packages.
