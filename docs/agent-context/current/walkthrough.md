# Phase 3.5: Self-Hosting & Robustness Walkthrough

## Overview

In this sub-phase, we focused on "dogfooding" `locald` by using it to host its own documentation site (`locald-docs`). This process exposed several issues with process management and CLI ergonomics, which we addressed.

## Changes

### 1. Self-Hosting Documentation
We created a `locald.toml` in `locald-docs/` to run the Astro dev server.
```toml
[project]
name = "locald-docs"

[services.web]
command = "pnpm astro dev --port $PORT"
```

### 2. Daemon Robustness
- **Stdin Handling**: We discovered that background processes inheriting `stdin` from the daemon would crash (SIGTTIN/EIO) when the daemon was detached. We fixed this by explicitly setting `stdin(Stdio::null())` for child processes in `ProcessManager`.
- **Detachment**: We switched to using `setsid` when spawning the daemon from the CLI. This creates a new session, ensuring the daemon is fully detached from the CLI's terminal and survives `Ctrl+C`.
- **Idempotency**: We updated `locald server` to check if the daemon is already running (via IPC Ping) before attempting to start it. This prevents "Address in use" errors and zombie processes.

### 3. CLI Improvements
- **`shutdown`**: Added a dedicated command to gracefully stop the daemon.
- **`stop`**: Made the command context-aware. If run without arguments in a directory with `locald.toml`, it stops the services defined in that file.
- **`status`**: Improved the output table to include a `URL` column, making it easier to access running services.
- **Error Handling**: Improved client error messages when the daemon is not running.

## Verification
We verified these changes by:
1. Starting the daemon (`locald server`).
2. Starting the docs (`cd locald-docs && locald start`).
3. Verifying the site is accessible at the URL shown in `locald status`.
4. Stopping the docs (`locald stop`).
5. Shutting down the daemon (`locald shutdown`).
