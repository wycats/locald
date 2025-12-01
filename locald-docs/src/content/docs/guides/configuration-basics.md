---
title: Basic Configuration
description: Common configuration patterns for everyday use.
---

This guide covers the most common configuration scenarios you'll encounter when setting up `locald` for your applications.

## The Minimal Config

At its core, `locald` just needs to know what command to run.

```toml
[project]
name = "my-website"

[services]
# The 'web' service runs your server
web = { command = "python3 -m http.server $PORT" }
```

**Key Concept**: `locald` assigns a random free port to your service and passes it via the `$PORT` environment variable. Your app **must** listen on this port.

## Adding a Local Domain

Accessing your app via `localhost:45123` is tedious. You can assign a `.local` domain to make it easier to remember.

```toml
[project]
name = "my-website"
domain = "my-website.local"

[services]
web = { command = "python3 -m http.server $PORT" }
```

After adding this, you'll need to run:
1.  `locald stop` and `locald start` to reload the config.
2.  `sudo locald admin sync-hosts` to update your `/etc/hosts` file (only needed once per new domain).

Now you can visit `http://my-website.local`!

## Setting Environment Variables

You often need to pass configuration or secrets to your app.

```toml
[project]
name = "backend-api"

[services]
api = { command = "npm start" }

# Add environment variables to a specific service
[services.api.env]
NODE_ENV = "development"
DATABASE_URL = "postgres://localhost:5432/mydb"
```

## Running in a Subdirectory

If your `locald.toml` is in the root, but your code is in a `backend/` folder, use `workdir`.

```toml
[project]
name = "fullstack-app"

[services]
# Runs 'npm start' inside the 'backend' directory
api = { command = "npm start", workdir = "./backend" }
```
