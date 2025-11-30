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
[service]
name = "my-app"
command = "python3 -m http.server $PORT"
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
