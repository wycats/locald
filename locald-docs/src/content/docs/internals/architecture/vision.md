---
title: Architecture Vision
description: The architectural intent behind the locald core workflow.
---

> **Core focus**: The core loop is `locald up → HTTPS → monitor`. Everything else supports that loop.

This page summarizes the architectural intent behind `locald` so implementation details stay aligned with the core value proposition.

## Execution Pipeline

- **Source of truth**: `locald.toml` defines services and their commands.
- **Supervisor**: The daemon reconciles desired vs. actual state.
- **Environment injection**: Each service receives `$PORT` and related runtime variables.

## Networking & HTTPS

- **Stable domains**: Each project gets a consistent `*.localhost` domain.
- **HTTPS by default**: `locald` issues TLS certs for `.localhost` and serves on ports 80/443.

## Observability

- **Unified logs**: All services stream into the dashboard and CLI.
- **Health model**: Services are tracked as starting/healthy/degraded for orchestration clarity.

## Built-in Services

- **Managed databases**: First-class services like Postgres are provided as local managed resources.
- **Lifecycle control**: Managed services follow the same supervision model as user commands.

## Containers & Builds (Optional)

- **Container execution**: Optional container-based services for production parity.
- **CNB builds**: A zero-config path to produce OCI images.

These are **experimental** and should not block the default host execution story.

## Plugins & Extensibility

- **Plugin contract**: Plugins can declare services and resources without bloating the core.
- **Distributions**: Specialized builds can pre-bundle plugins or defaults.

## The Shim

- **Privileged boundary**: `locald-shim` handles elevated operations or container runtime access.
- **Safety**: Keep the daemon unprivileged whenever possible.

## File System Watchers (Future)

- **Intent**: Restart services or refresh state on config/source changes.
- **Scope**: Watchers should be minimal and predictable.
