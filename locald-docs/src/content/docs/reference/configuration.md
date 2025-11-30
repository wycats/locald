---
title: Configuration
description: Reference for locald.toml.
---

The `locald.toml` file defines how `locald` runs your service.

## Schema

```toml
[service]
# The name of the service. Must be unique per project.
name = "my-service"

# The command to run.
# $PORT will be replaced by the assigned port.
command = "npm start"

# Optional: Working directory (defaults to the directory containing locald.toml)
# work_dir = "./backend"

# Optional: Environment variables
[service.env]
NODE_ENV = "development"
```

## Environment Variables

`locald` automatically injects the following environment variables into your process:

- `PORT`: The dynamically assigned TCP port.
