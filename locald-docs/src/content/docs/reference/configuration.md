---
title: Configuration Reference
description: Complete specification for locald.toml.
---

The `locald.toml` file is the source of truth for your project's configuration. It uses the [TOML](https://toml.io/) format.

## `[project]` Section

Defines global settings for the project.

| Key      | Type   | Required | Description                                                                                      |
| :------- | :----- | :------- | :----------------------------------------------------------------------------------------------- |
| `name`   | String | **Yes**  | A unique identifier for the project. Used for namespacing logs and services.                     |
| `domain` | String | No       | A local domain (e.g., `app.local`) to route to this project. Requires `locald admin sync-hosts`. |

## `[services]` Section

Defines the processes to run. Keys are service names (e.g., `web`, `worker`). Values can be a simple table or an inline table.

### Service Options

| Key          | Type         | Required | Description                                                                                     |
| :----------- | :----------- | :------- | :---------------------------------------------------------------------------------------------- |
| `command`    | String       | **Yes**  | The shell command to execute. Supports environment variable expansion (e.g., `$PORT`).          |
| `workdir`    | String       | No       | The working directory for the command, relative to `locald.toml`. Defaults to the project root. |
| `env`        | Table        | No       | Key-value pairs of environment variables to inject into the process.                            |
| `depends_on` | List<String> | No       | A list of other service names that must start before this service.                              |

### Example: Full Specification

```toml
[project]
name = "complex-app"
domain = "complex.local"

[services.api]
command = "./target/debug/api"
workdir = "./backend"
depends_on = ["db"]

[services.api.env]
RUST_LOG = "debug"
DB_HOST = "localhost"

[services.worker]
command = "celery -A proj worker"
workdir = "./worker"
depends_on = ["api", "redis"]
```

## Injected Environment Variables

`locald` guarantees the following variables are present in the service environment:

- `PORT`: A dynamically assigned, free TCP port. The service **must** bind to this port to be reachable via the proxy.
- `PATH`: Inherited from the `locald` process (usually your user's shell path).
