---
title: "Host-Level Build Commands"
stage: 0
feature: Build
---

# RFC: Host-Level Build Commands

## Summary

Expand the `[service.build]` configuration to support simple host-level build commands (e.g., `npm run build`, `./scripts/sync.sh`) in addition to the existing Cloud Native Buildpacks (CNB) configuration.

## Motivation

Currently, `locald` supports two modes of operation:

1.  **Exec**: Run a command directly (`command = "npm start"`).
2.  **CNB**: Build a container image (`[service.build]`).

However, many local workflows require a "pre-flight" build step that runs on the host before the main service starts. Examples include:

- Compiling assets (Webpack/Vite).
- Syncing documentation (our own `sync-manifesto.sh`).
- Database migrations (sometimes).

Users currently have to chain these in the `command` (e.g., `command = "npm run build && npm start"`), which has downsides:

- **Restart Loop**: If the service crashes and restarts, the build runs again unnecessarily.
- **No Feedback**: The dashboard shows "Running" while it's actually building.
- **Timeout**: Long builds might trigger health check timeouts.

## Guide-level explanation

You can now specify a build command directly in the `build` field:

```toml
[services.docs]
# Run this ONCE before starting the service
build = "./scripts/sync-manifesto.sh"
command = "pnpm astro dev"
```

If you need the advanced CNB behavior, you use the table syntax:

```toml
[services.web.build]
builder = "heroku/builder:22"
```

## Reference-level explanation

The `build` field in `ServiceConfig` becomes polymorphic (or we add a `build_command` field and keep `build` for CNB).

**Option A: Polymorphic `build`**

- If `build` is a **String**: It is treated as a shell command to run on the host.
- If `build` is a **Table**: It is treated as CNB configuration.

**Option B: Explicit `build_command`**

- `build_command = "..."`: Host command.
- `[build]`: CNB config.

**Lifecycle**:

1.  **State: Building**: `locald` transitions the service to a `Building` state.
2.  **Execution**: The build command runs.
    - Output is streamed to a separate log stream or the main log.
3.  **Success**: Service transitions to `Starting`.
4.  **Failure**: Service transitions to `Error` (Backoff applies).

**Caching**:

- By default, the build runs on every `locald up` or `restart`.
- Future work could explore file-watching triggers (`watch = ["src/**/*"]`).

## Rationale and alternatives

- **Chaining in `command`**: `command = "build && run"`.
  - _Pros_: Simple, no code changes.
  - _Cons_: Re-runs on crash-restart. Bad UX.
- **Task Runner**: `locald run build`.
  - _Pros_: Explicit.
  - _Cons_: Doesn't solve the "auto-build on start" requirement for dev servers.

## Unresolved questions

- **Watch Mode**: Should the build re-run if files change? (Probably out of scope for V1).
- **Concurrency**: Do builds block dependent services? (Yes, `depends_on` should wait for `Healthy`, which implies `Running`, which implies `Build` complete).
