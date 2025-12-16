---
title: DNS and Domains
description: How to configure local domains and SSL for your services.
---

`locald` allows you to access your services via custom domains (e.g., `https://my-app.localhost`) instead of remembering port numbers.

## Configuration

To enable domain access, add a `domain` field to the `[project]` section of your `locald.toml`. If omitted, it defaults to `<project-name>.localhost`.

```toml
[project]
name = "my-app"
domain = "my-app.localhost"

[services]
web = { command = "npm start", port = 3000 }
```

## Zero-Config SSL

`locald` automatically generates valid SSL certificates for any `.localhost` domain. This allows you to develop with HTTPS enabled, mirroring production environments and enabling features like Secure Cookies and Service Workers.

### Trusting the CA

To make your browser trust these certificates, you need to install the `locald` Root CA once:

```bash
locald trust
```

This command (which may require `sudo`) generates a root certificate and adds it to your system's trust store (and Firefox's if installed).

## Setup

### 1. Port Binding

`locald` listens on ports 80 (HTTP) and 443 (HTTPS) to route traffic.
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
