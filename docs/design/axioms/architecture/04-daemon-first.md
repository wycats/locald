# Axiom 2: Daemon-First Architecture

**The core logic runs as a background daemon (`locald-server`).**

## Rationale

Development processes (servers, databases, watchers) need to persist beyond the lifespan of a single terminal window. If the user closes their terminal, the app should keep running.

## Implications

- **Thin Client**: The `locald` CLI is just a remote control. It does very little logic other than parsing arguments and sending IPC requests.
- **State Management**: The Daemon is the single source of truth for "what is running". It holds the handles to child processes.
- **Resilience**: If the CLI crashes, the apps stay up. If the Daemon crashes, the apps (likely) go down, so the Daemon must be robust.
- **Logs**: Since processes are detached from the terminal, `stdout/stderr` must be captured and buffered by the Daemon so they can be streamed back to the user later.

## Implementation Details

- **Self-Daemonization**: The `locald-server` binary is responsible for daemonizing itself (forking, detaching, PID file management). It does not rely on the CLI or shell operators (`&`) for background execution.
- **Idempotency**: The server checks for its own existence (via socket or PID file) on startup to prevent duplicate instances.
