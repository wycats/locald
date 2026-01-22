# Architecture Vision

> **Core focus**: The core loop is `locald up → HTTPS → monitor`. Everything else supports that loop.

This document outlines the architectural intent behind `locald` so implementation details remain aligned with the core value proposition.

## 1) Execution Pipeline

- **Source of truth**: `locald.toml` defines services and their commands.
- **Supervisor**: The daemon ensures processes match the desired state (restart on crash, track health).
- **Environment injection**: Each service receives `$PORT` and other runtime variables so it can bind predictably.

## 2) Networking & HTTPS

- **Stable domains**: Each project gets a consistent `*.localhost` domain.
- **HTTPS by default**: `locald` issues TLS certs for `.localhost` and serves on ports 80/443, so local networking mirrors production expectations.
- **Disabled services still resolve**: the proxy returns an enable page instead of a dead link.

## 3) Observability (Logs & Status)

- **Unified logs**: All services stream into the dashboard and CLI.
- **Health model**: Services are tracked as starting/healthy/degraded to support orchestration and UX clarity.

## 4) Built-in Services

- **Managed databases**: First-class services like Postgres are provided as local managed resources.
- **Lifecycle control**: Managed services follow the same supervision model as user commands.

## 5) Containers & Builds (Optional)

- **Container execution**: Optional container-based services to mirror production.
- **CNB builds**: A zero-config path to produce OCI images when needed.

These are **experimental** and should not block the default host execution story.

## 6) Plugins & Extensibility

- **Plugin contract**: Plugins should be able to declare services and resources without bloating the core.
- **Distribution support**: Specialized builds can pre-bundle plugins or defaults for specific teams.

## 7) The Shim

- **Privileged boundary**: `locald-shim` handles tasks requiring elevated permissions or container runtime access.
- **Safety**: Keep the daemon unprivileged whenever possible.

## 8) File System Watchers (Future)

- **Intent**: Restart services or refresh state on config/source changes.
- **Scope**: Watchers should be minimal and predictable—no surprise rebuilds.
