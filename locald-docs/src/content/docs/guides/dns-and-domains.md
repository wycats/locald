---
title: DNS and Domains
description: How to configure local domains for your services.
---

`locald` allows you to access your services via custom domains (e.g., `http://my-app.local`) instead of remembering port numbers.

## Configuration

To enable domain access, add a `domain` field to the `[project]` section of your `locald.toml`:

```toml
[project]
name = "my-app"
domain = "my-app.local"

[services]
web = { command = "npm start", port = 3000 }
```

## Setup

### 1. Port Binding

By default, `locald` runs as your user and cannot bind to port 80 (which is required for clean URLs like `http://my-app.local`).
To allow this, run:

```bash
sudo locald admin setup
```

This grants the `cap_net_bind_service` capability to the `locald-server` binary.
If you skip this step, `locald` will fall back to port 8080 (e.g., `http://my-app.local:8080`).

### 2. Hosts File

Your computer needs to know that `my-app.local` points to your local machine (`127.0.0.1`).
`locald` can manage this for you.

After starting your services, run:

```bash
sudo locald admin sync-hosts
```

This will safely add the necessary entries to your `/etc/hosts` file (or Windows equivalent).
`locald` uses a marked section (`# BEGIN locald`) to ensure it doesn't mess up your existing configuration.

## Usage

Once configured and synced, you can access your service at:

```
http://my-app.local
```

If you didn't run `admin setup`, you might need:

```
http://my-app.local:8080
```

Check `locald status` to see the active URL for your services.
