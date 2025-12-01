# Phase 5 Implementation Plan: Web UI & TUI Basics

## Goal
Create a robust, "rock solid" Web UI dashboard and CLI log streaming capability. The Web UI will be the primary visual interface for monitoring services, while the CLI remains the primary control interface.

## User Requirements
- **Robustness**: The Web UI must not feel "flaky".
- **Error Handling**: Clear communication of status and errors.
- **Remediation**: In-web remediation steps where appropriate.
- **Communication**: Explicit status indicators (Connection State, Service State).

## Architecture

### 1. Web Server & Asset Serving
- **Domain**: `locald.local` (reserved for the dashboard).
- **Embedding**: Use `rust-embed` to compile the frontend assets into the `locald-server` binary.
- **Routing**:
  - `GET /`: Dashboard HTML.
  - `GET /api/state`: Current state of all services (JSON).
  - `GET /api/logs`: WebSocket endpoint for log streaming.
  - `POST /api/services/:id/action`: Start/Stop/Restart.

### 2. Frontend Application (The Dashboard)
- **Technology**: Minimal dependencies. Vanilla JS or a very light library (e.g., Preact via CDN or bundled) to ensure stability and low overhead. Let's stick to **Vanilla JS + ES Modules** for now to avoid a complex build step in the Rust repo, or a simple `build.rs` integration if we need bundling.
- **State Management**:
  - Robust WebSocket connection handling (auto-reconnect with backoff).
  - Visual indicator for "Daemon Connection" (Green/Red).
- **Components**:
  - **Service List**: Status indicators, uptime, port.
  - **Log Viewer**: Virtualized list or careful DOM management for performance.
  - **Controls**: Start/Stop/Restart buttons with loading states.

### 3. Log Streaming (Backend)
- **Broadcast Channel**: Use `tokio::sync::broadcast` to distribute log lines from the `ProcessManager` to active WebSocket clients.
- **Buffering**: Keep a small ring buffer of recent logs so new clients see immediate context.

### 4. CLI `logs` Command
- Implement `locald logs <service>` (or all services).
- Connects to the daemon via the same mechanism (or IPC) to stream logs to stdout.

## Step-by-Step Plan

### Step 1: Log Broadcasting Infrastructure
- [ ] Add `broadcast` channel to `ProcessManager`.
- [ ] Implement a `LogBuffer` to store the last N lines.
- [ ] Update `ProcessManager` to send stdout/stderr lines to the broadcaster.

### Step 2: WebSocket & API Endpoints
- [ ] Add `axum` (or `hyper-tungstenite` if we are raw hyper) for WebSocket support. *Note: We are using Hyper directly in `proxy.rs`. We might want to bring in `axum` for the `locald.local` internal router to simplify API and WS handling, while keeping the proxy raw or integrated.*
- [ ] Implement `GET /api/state` to return the `ServiceRegistry` state.
- [ ] Implement `GET /api/logs` WebSocket handler.

### Step 3: Frontend Development
- [ ] Create `locald-server/src/assets/` for HTML/CSS/JS.
- [ ] Implement the "Connection Manager" (robust WS handling).
- [ ] Implement the Service List view.
- [ ] Implement the Log Viewer.
- [ ] Add `rust-embed` to serve these assets from `locald.local`.

### Step 4: CLI Logs
- [ ] Implement `locald-cli logs` subcommand.
- [ ] Connect to the daemon and print logs.

### Step 5: Verification & Robustness Polish
- [ ] Test killing the daemon and seeing the UI react.
- [ ] Test restarting the daemon and seeing the UI reconnect.
- [ ] Test high log volume.
