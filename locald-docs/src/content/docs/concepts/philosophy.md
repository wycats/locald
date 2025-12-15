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

## 12-Factor Alignment

`locald` is opinionated. It is built on the [12-Factor App](https://12factor.net/) methodology, specifically regarding **Port Binding** and **Concurrency**.

### Port Binding (Factor VII)

> The twelve-factor app is completely self-contained and does not rely on runtime injection of a webserver into the execution environment to create a web-facing service. The web app exports HTTP as a service by binding to a port, and listening to requests coming in on that port.

In `locald`, your app is the **Service**. It must listen on a port provided by the environment (`$PORT`).

- **Your App**: "I listen on `$PORT`."
- **locald**: "I assign you a random port (e.g., 45231) and route traffic to you."

This separation of concerns allows `locald` to act as the **Platform**. The Platform handles routing, domains, and SSL (eventually), while your app remains simple and portable.

### Domains vs. Ports

Because of this philosophy, we distinguish between **Service Ports** and **Public Domains**.

- **Service Port**: The ephemeral, random port your app binds to. You rarely need to know this number.
- **Public Domain**: The stable, semantic URL (`https://my-app.localhost`) that you use to access the app.

`locald`'s reverse proxy bridges the gap. It listens on the standard HTTP port (80) and forwards requests to the correct Service Port based on the `Host` header.

### Process Model (Factor VIII)

> In the twelve-factor app, processes are a first-class citizen. Processes take strong cues from the unix process model for running service daemons.

`locald` treats every service as a process. It manages their lifecycle (start, stop, restart) and captures their output (stdout/stderr). This ensures that your development environment mirrors a production process manager (like systemd or supervisord).
