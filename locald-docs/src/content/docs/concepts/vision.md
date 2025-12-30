---
title: "Vision: Local Development as a Platform"
---


**"Treat the local machine as a first-class platform."**

`locald` bridges the gap between the **immediacy** of local coding and the **rigor** of cloud deployment. It provides a structured, robust, and interactive environment where services are not just processes, but managed citizens of a cohesive workspace.

## Core Philosophy

We are not building a task runner (like `make`) or a process manager (like `supervisord`). We are building a **Platform for Local Development**.

### 1. The Immediacy of Local

Local development must be fast. Friction kills flow. `locald` prioritizes **Zero-Friction Start**: clone a repo, run `locald up`, and it works. No complex manifests, no container builds (unless you want them), no manual port management.

### 2. The Rigor of Cloud

Local development shouldn't be a "wild west" of unmanaged processes and random ports. `locald` brings cloud-native discipline to the desktop:

- **Service Discovery**: `app.localhost` instead of `localhost:3456`.
- **Isolation**: Processes run in managed groups with injected environments.
- **Observability**: Logs are captured, buffered, and streamed, not lost in a terminal scrollback.

### 3. The Workspace

The dashboard is not a passive monitor; it is an active **Workspace**. It persists context, remembers your preferences, and allows you to interact with your services as if they were local terminals.

## Relationship to Axioms

This vision is realized through our [Design Axioms](axioms.md), which define the specific architectural and experience constraints we accept to achieve this goal.

