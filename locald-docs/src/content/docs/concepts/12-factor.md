---
title: 12-Factor Philosophy
description: Why locald works the way it does.
---

`locald` is opinionated. It is built on the [12-Factor App](https://12factor.net/) methodology, specifically regarding **Port Binding** and **Concurrency**.

## Port Binding (Factor VII)

> The twelve-factor app is completely self-contained and does not rely on runtime injection of a webserver into the execution environment to create a web-facing service. The web app exports HTTP as a service by binding to a port, and listening to requests coming in on that port.

In `locald`, your app is the **Service**. It must listen on a port provided by the environment (`$PORT`).

- **Your App**: "I listen on `$PORT`."
- **locald**: "I assign you a random port (e.g., 45231) and route traffic to you."

This separation of concerns allows `locald` to act as the **Platform**. The Platform handles routing, domains, and SSL (eventually), while your app remains simple and portable.

## Domains vs. Ports

Because of this philosophy, we distinguish between **Service Ports** and **Public Domains**.

- **Service Port**: The ephemeral, random port your app binds to. You rarely need to know this number.
- **Public Domain**: The stable, semantic URL (`http://my-app.local`) that you use to access the app.

`locald`'s reverse proxy bridges the gap. It listens on the standard HTTP port (80) and forwards requests to the correct Service Port based on the `Host` header.

## Process Model (Factor VIII)

> In the twelve-factor app, processes are a first-class citizen. Processes take strong cues from the unix process model for running service daemons.

`locald` treats every service as a process. It manages their lifecycle (start, stop, restart) and captures their output (stdout/stderr). This ensures that your development environment mirrors a production process manager (like systemd or supervisord).
