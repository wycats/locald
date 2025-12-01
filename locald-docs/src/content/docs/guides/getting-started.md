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

1.  Create a new directory for your project:

    ```bash
    mkdir my-app
    cd my-app
    ```

2.  Initialize the project:

    ```bash
    locald init
    ```

    Follow the interactive prompts to set up your project name and first service. For this example, you can use:

    - **Project Name**: `my-app`
    - **Service Name**: `web`
    - **Command**: `python3 -m http.server $PORT`

    This will generate a `locald.toml` file for you.

3.  Start the service:

    ```bash
    locald start
    ```

## Monitoring Your App

`locald` provides a real-time dashboard to see your running services and logs.

```bash
locald monitor
```

This opens a TUI (Text User Interface) where you can:

- See the status of all services.
- View real-time logs (stdout/stderr).
- See the assigned ports and URLs.

Press `q` or `Ctrl+C` to exit the monitor (your services will keep running!).

You can also check the status via the CLI:

```bash
locald status
```

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
