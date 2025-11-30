---
title: Getting Started
description: Install and run your first service with locald.
---

## Installation

Currently, `locald` is built from source.

```bash
git clone https://github.com/ykatz/dotlocal.git
cd dotlocal
cargo install --path locald-cli
cargo install --path locald-server
```

## Running the Daemon

Start the daemon in the background:

```bash
locald server
```

You can verify it's running:

```bash
locald ping
# Output: pong
```

## Your First Service

1.  Create a new directory for your project.
2.  Create a `locald.toml` file:

```toml
[project]
name = "my-app"

[services]
web = { command = "python3 -m http.server $PORT" }
```

3.  Start the service:

```bash
locald start
```

4.  Check the status:

```bash
locald status
```

You should see your app running on a dynamically assigned port!

## Next Steps: Go Public (Locally)

Accessing `localhost:34123` is annoying. Let's give it a real domain.

1.  Update `locald.toml` to add a domain:

```toml
[project]
name = "my-app"
domain = "my-app.local"

[services]
web = { command = "python3 -m http.server $PORT" }
```

2.  Restart the service:

```bash
locald stop
locald start
```

3.  Configure your system (one-time setup):

```bash
# Allow locald to bind port 80
sudo locald admin setup

# Point my-app.local to 127.0.0.1
sudo locald admin sync-hosts
```

4.  Visit `http://my-app.local` in your browser!
