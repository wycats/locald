---
title: Configuration
description: Reference for locald.toml.
---

The `locald.toml` file defines how `locald` runs your service.

## Schema

```toml
[project]
# The name of the project. Used for namespacing services.
name = "my-project"

# Optional: The domain to serve the project on.
# If set, locald will route requests from this domain to your services.
domain = "my-app.local"

[services]
# Define services as a map. The key is the service name.
web = { command = "npm start", port = 3000 }
worker = { command = "npm run worker" }

# Extended syntax for more options
[services.api]
command = "cargo run"
# Optional: Working directory (defaults to the directory containing locald.toml)
workdir = "./backend"
# Optional: Environment variables
env = { RUST_LOG = "debug" }
```

## Environment Variables

`locald` automatically injects the following environment variables into your process:

- `PORT`: The dynamically assigned TCP port. Your service **must** listen on this port.
