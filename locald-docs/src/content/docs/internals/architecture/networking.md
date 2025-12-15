---
title: "Architecture: Networking"
---

This document describes how `locald` handles networking, including port assignment, proxying, and DNS.

## 1. Port Assignment

`locald` aims to eliminate port conflicts.

- **Dynamic Assignment**: By default, `locald` assigns a random free port to each service.
- **Injection**: This port is injected into the service's environment as the `PORT` variable. Services are expected to bind to this port.
- **Discovery**: For services that don't respect `PORT`, `locald` can attempt to discover the port they bound to (e.g., by scanning `/proc/net/tcp`).

## 2. The Reverse Proxy

The daemon includes a built-in HTTP reverse proxy (based on `hyper` / `axum`).

- **Routing**: Requests are routed based on the **Host Header** (e.g., `my-app.localhost`).
- **Path-Based Routing**: Advanced configuration allows routing specific paths (e.g., `/api`) to different services.
- **Protocols**: Supports HTTP/1.1 and HTTP/2.

## 3. DNS & The `.localhost` Domain

`locald` uses the `.localhost` top-level domain (TLD).

- **Why `.localhost`?**: It is reserved by RFC 2606 and is treated as a **Secure Context** by modern browsers (bypassing Mixed Content warnings and enabling features like Service Workers). It resolves to `127.0.0.1` on most systems without configuration.
- **Hosts File**: For systems that don't automatically resolve subdomains of `.localhost`, `locald` manages a dedicated section in `/etc/hosts`.
  - **Safety**: It only modifies lines between `# BEGIN locald` and `# END locald` markers.

## 4. SSL / TLS

`locald` provides "Zero-Config SSL" for local development.

- **Pure Rust Stack**: Uses `rcgen` to generate certificates and `rustls` to serve them. No external `openssl` binary required.
- **Root CA**: Generates a local Root CA on first run and installs it into the system trust store (requires `sudo` once).
- **On-Demand Certs**: Leaf certificates for domains (e.g., `app.localhost`) are generated in-memory during the TLS handshake.

## 5. Notify Protocol

`locald` implements a Unix Datagram socket server compatible with the `systemd` notification protocol (`sd_notify`).

- Services can send `READY=1` to this socket to signal they are up and running.
- The `NOTIFY_SOCKET` environment variable is injected into services.
