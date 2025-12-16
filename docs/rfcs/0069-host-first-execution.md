---
title: "Host-First Execution Strategy"
stage: 3 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Runtime
---

# RFC 0069: Host-First Execution Strategy

## 1. Summary

This RFC proposes a pivot in the default execution strategy for `locald`. Instead of defaulting to Cloud Native Buildpacks (CNB) and containerized execution, `locald` will default to **Host-First Execution** (running processes directly on the host OS).

CNB and containerization will become an **Opt-In** feature for advanced use cases or deployment artifact generation.

Additionally, we propose a future "Dump Layer" mode for CNB, allowing buildpacks to provision the host environment without forcing containerization at runtime.

## 2. Motivation

While CNB offers excellent reproducibility and "Zero Config" builds, forcing it as the default for local development introduces significant friction:

1.  **Complexity**: Debugging containerized processes, managing file permissions, and integrating with host tools (IDEs, LSPs) is inherently more complex than running a local binary.
2.  **Performance**: Even with caching, the container build lifecycle adds overhead compared to a simple `cargo run` or `npm start`.
3.  **Expectations**: Developers using a tool like `locald` (similar to `foreman` or `overmind`) expect it to manage their local processes, not necessarily to containerize them.
4.  **"Finicky" Nature**: Getting CNB to work perfectly across all edge cases (distro differences, user namespaces, bind mounts) has proven to be a high-effort endeavor that distracts from the core value proposition of "just running my app".

By defaulting to "Host-First", we align with the "Happy Path" of least resistance while keeping the power of CNB available when needed.

## 3. Detailed Design

### 3.1 Default Behavior: Host Process

When a user runs `locald up` or `locald start`, `locald` will:

1.  Look for a `command` in `locald.toml`.
2.  Look for a `Procfile`.
3.  Execute the command directly as a child process of the daemon (using `portable-pty` for terminal emulation).
4.  Apply environment variables from `.env` and `locald.toml`.

**No containerization, no namespaces, no build step (unless specified).**

### 3.2 Opt-In CNB / Docker

Users can opt-in to containerized execution by specifying a `build` section or an `image` in `locald.toml`.

```toml
# Host-First (Default)
[service.web]
command = "npm run dev"

# CNB (Opt-In)
[service.api]
build = { builder = "heroku/builder:22" }

# Docker Image (Opt-In)
[service.db]
image = "postgres:15"
```

### 3.3 The "Dump Layer" Concept (Future)

To bridge the gap between "Host Raw" and "CNB Container", we propose a mode where CNB is used as a **Package Manager**.

1.  **Build**: Run the CNB lifecycle up to the `export` phase (or a custom phase).
2.  **Provision**: Extract the `layers` directory (containing language runtimes, dependencies, etc.) to a user-accessible location (e.g., `~/.local/share/locald/layers`).
3.  **Run**: Execute the process **on the host**, but with `PATH`, `LD_LIBRARY_PATH`, and other environment variables configured to point to the extracted layers.

This allows users to get the "Zero Config" environment setup of CNB (e.g., "I need Python 3.11 and these pip packages") without the runtime isolation of a container.

## 4. Implementation Plan

### Phase 1: Configuration & Default Pivot

- [ ] Update `locald.toml` schema to support optional `[service.build]`.
- [ ] Refactor `ProcessManager` to default to `ProcessRuntime` (Host) unless `build` or `image` is present.
- [ ] Ensure `ProcessRuntime` supports "Raw" execution (no `runc`, no `shim`).

### Phase 2: CNB Opt-In

- [ ] Ensure existing CNB logic is gated behind the `build` configuration.
- [ ] Verify that `locald build` still works for creating images.

### Phase 3: Dump Layer (Future)

- [ ] Research `lifecycle` flags or custom phases to support "build-only" or "export-to-host".
- [ ] Implement environment variable translation (container paths -> host paths).

## 5. Migration Strategy

- Existing projects with `locald.toml` containing `image` will continue to work as Docker services.
- Existing projects relying on implicit CNB builds will need to add `[service.name.build]` to their config OR add a `command` to run locally.
- We will update the documentation to reflect "Host-First" as the primary workflow.
