# Phase 5 Walkthrough: Web UI & TUI Basics

## Overview
This phase focuses on providing visibility into the running system. We are moving from a "blind" daemon (controllable only via CLI status checks) to an observable system with a real-time dashboard.

## Key Decisions

### 1. `locald.local` as the Dashboard Domain
Consistent with our "12-factor" and "managed ports" philosophy, the dashboard itself is treated as a service provided by the platform. It lives at a known local domain rather than a random port.

### 2. Robustness First
The Web UI is designed to be "rock solid". This means:
- **Aggressive Reconnection**: If the daemon goes away, the UI knows and tries to reconnect.
- **Clear State**: No ambiguous "loading" states that hang forever.
- **In-Band Remediation**: If something is wrong, the UI tells you what to do.

### 3. Axum Integration
We integrated `axum` into the `locald-server` to handle the `locald.local` API and WebSocket endpoints. This provides a robust and ergonomic way to handle HTTP requests alongside the raw Hyper proxy.

### 4. Log Streaming Architecture
We implemented a `broadcast` channel in the `ProcessManager` to distribute logs to multiple consumers (Web UI and CLI). A ring buffer ensures that new clients see recent history immediately.

## Implementation Log

- **Infrastructure**: Added `tokio::sync::broadcast` and `LogBuffer` to `ProcessManager`.
- **Backend**: Integrated `axum` for `locald.local` routing, including WebSocket support for logs.
- **Frontend**: Created a robust, dependency-free SPA for the dashboard using Vanilla JS and ES Modules.
- **CLI**: Added `locald logs` command to stream logs to the terminal.
- **Robustness**: Added port fallback (80 -> 8080 -> 8081) to ensure the server can always start.
