---
title: "Axiom 5: Interface Parity (TUI = Web)"
---

**Information and control available in the Web UI must also be available in the terminal (TUI/CLI).**

## Rationale

Developers live in the terminal. Forcing them to switch to a browser to restart a service or check a log is a context switch. However, a Web UI is superior for "at a glance" status of many services or visualizing complex data. We want the best of both.

## Implications

- **API-Driven**: The Daemon exposes a structured API (likely JSON over HTTP or WebSocket).
- **Shared Consumption**: The Web UI is a client of this API. The TUI is _also_ a client of this API.
- **Feature Parity**: If we add a "Restart" button to the Web UI, we must add `locald restart` to the CLI.
- **Streaming**: The API must support streaming (e.g., Server-Sent Events or WebSockets) for logs, so `locald logs -f` works just as well as the Web UI log viewer.
