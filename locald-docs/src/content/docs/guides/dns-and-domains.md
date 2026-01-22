---
title: DNS and Domains
description: How to configure local domains and SSL for your services.
---

`locald` gives every workspace a stable domain and HTTPS by default, so your local environment behaves like production.

> **Platform note**: The OS that runs your browser must trust the local CA. On WSL, that usually means using a browser inside WSL (WSLg). End-to-end Windows browser support is tracked as a roadmap item.

## Configuration

To enable domain access, add a `domain` field to the `[project]` section of your `locald.toml`. If omitted, it defaults to `<project-name>.localhost`.

```toml
[project]
name = "my-app"
domain = "my-app.localhost"

[services]
web = { command = "npm start", port = 3000 }
```

## Zero-Config HTTPS

`locald` automatically generates valid SSL certificates for any `.localhost` domain. This lets you develop with HTTPS on day one, enabling features like Secure Cookies and Service Workers without extra setup.

### Trusting the CA

To make your browser trust these certificates, install the `locald` Root CA once:

```bash
locald trust
```

This command (which may require `sudo`) generates a root certificate and adds it to your system's trust store (and Firefox's if installed).

For the Windows/WSL story, see [Windows & WSL (Roadmap)](/roadmap/windows-and-wsl/).

## Setup

### 1. Port Binding

`locald` listens on ports 80 (HTTP) and 443 (HTTPS) to route traffic using the same network shape you’ll use in production.
On Linux, binding these low ports requires special permissions.

To allow this without running `locald` as root, run:

```bash
sudo locald admin setup
```

This grants the `cap_net_bind_service` capability to the `locald` binary.

### 2. Hosts File

While `.localhost` is technically a reserved TLD that should resolve to loopback, some browsers and tools still rely on `/etc/hosts`.
`locald` can manage this for you to ensure maximum compatibility.

After starting your services, run:

```bash
sudo locald admin sync-hosts
```

This will safely add the necessary entries to your `/etc/hosts` file (or Windows equivalent).
`locald` uses a marked section (`# BEGIN locald`) to ensure it doesn't mess up your existing configuration.

## Usage

Once configured, you can access your service at:

```
https://my-app.localhost
```

Check `locald status` to see the active URL for your services.

If a service is disabled, the domain still resolves and shows a short enable page instead of a dead link.
