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
| `domain` | String | No       | A local domain (e.g., `app.localhost`) to route to this project. Defaults to `<name>.localhost`. |

## `[services]` Section

Defines the processes to run. Keys are service names (e.g., `web`, `worker`).

### Common Options

These options apply to all service types.

| Key            | Type         | Default   | Description                                                                                                                                                                |
| :------------- | :----------- | :-------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `port`         | Integer      | Auto      | The port the service listens on. If omitted, `locald` assigns a random port and injects it via `$PORT`.                                                                    |
| `env`          | Table        | `{}`      | Key-value pairs of environment variables to inject into the process.                                                                                                       |
| `depends_on`   | List<String> | `[]`      | A list of other service names that must start before this service. `locald` waits for dependencies to be [Healthy](/concepts/health-checks) before starting the dependent. |
| `health_check` | Table/String | Auto      | Configuration for checking if the service is ready. See [Health Checks](#health-checks).                                                                                   |
| `stop_signal`  | String       | `SIGTERM` | The signal to send to stop the service.                                                                                                                                    |

### Service Types

`locald` supports different service types via the `type` field.

#### `exec` (Default)

Runs a standard shell command.

| Key              | Type    | Required | Description                                                                                     |
| :--------------- | :------ | :------- | :---------------------------------------------------------------------------------------------- |
| `command`        | String  | **Yes**  | The shell command to execute.                                                                   |
| `workdir`        | String  | No       | The working directory for the command, relative to `locald.toml`. Defaults to the project root. |
| `image`          | String  | No       | **Deprecated**. Use `type = "container"` instead.                                               |
| `container_port` | Integer | No       | **Deprecated**. Use `type = "container"` instead.                                               |

```toml
[services.web]
command = "npm start"
workdir = "./frontend"
```

#### `container`

Runs a service using a Docker container. This is preferred over `exec` for containerized services.

| Key              | Type    | Required | Description                                                                |
| :--------------- | :------ | :------- | :------------------------------------------------------------------------- |
| `image`          | String  | **Yes**  | The Docker image to run (e.g., `redis:7`).                                 |
| `command`        | String  | No       | Arguments to pass to the container entrypoint.                             |
| `container_port` | Integer | No       | The port exposed by the container. If omitted, no port mapping is created. |
| `workdir`        | String  | No       | The working directory inside the container.                                |

```toml
[services.redis]
type = "container"
image = "redis:7"
container_port = 6379
```

#### `postgres`

Runs a managed Postgres instance. `locald` handles downloading the binary, initializing the data directory, and managing the process.

| Key       | Type   | Default  | Description                     |
| :-------- | :----- | :------- | :------------------------------ |
| `version` | String | "stable" | The version of Postgres to use. |

```toml
[services.db]
type = "postgres"
version = "15"
```

#### `worker`

Runs a background worker process. Workers are not expected to bind to a port.

| Key       | Type   | Required | Description            |
| :-------- | :----- | :------- | :--------------------- |
| `command` | String | **Yes**  | The command to run.    |
| `workdir` | String | No       | The working directory. |

```toml
[services.worker]
type = "worker"
command = "bundle exec sidekiq"
```

### Health Checks

You can explicitly configure how `locald` determines if a service is ready.

| Key        | Type    | Default | Description                                     |
| :--------- | :------ | :------ | :---------------------------------------------- |
| `type`     | String  | -       | The type of check: `http`, `tcp`, or `command`. |
| `path`     | String  | `/`     | The path to check (for `http` type).            |
| `interval` | Integer | `1`     | Seconds between checks.                         |
| `timeout`  | Integer | `1`     | Seconds to wait for a response.                 |
| `command`  | String  | -       | The command to run (for `command` type).        |

**Shorthand**: You can provide a string to run a command check.

```toml
# HTTP Check
health_check = { type = "http", path = "/healthz" }

# Command Check (Shorthand)
health_check = "curl -f http://localhost:$PORT/health"
```

## Injected Environment Variables

`locald` guarantees the following variables are present in the service environment:

- `PORT`: A dynamically assigned, free TCP port. The service **must** bind to this port to be reachable via the proxy.
- `NOTIFY_SOCKET`: The path to the Unix socket for `sd_notify` readiness checks. See [Smart Health Checks](/concepts/health-checks) for details.
- `PATH`: Inherited from the `locald` process (usually your user's shell path).

## Experimental Features

### Build Configuration (CNB)

**Status**: Experimental / Active Development.

To run your service in a container using Cloud Native Buildpacks (CNB), add a `build` section. See [Cloud Native Builds](/reference/builds) for details.

| Key          | Type         | Default             | Description                |
| :----------- | :----------- | :------------------ | :------------------------- |
| `builder`    | String       | `heroku/builder:22` | The builder image to use.  |
| `buildpacks` | List<String> | `[]`                | List of buildpacks to use. |

```toml
[services.web.build]
builder = "heroku/builder:22"
buildpacks = ["heroku/nodejs"]
```
