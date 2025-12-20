# Execution Modes

`locald` supports two primary execution modes for your services: **Host Execution** and **Container Execution**.

## 1. Host Execution (Default)

In this mode, `locald` runs your application directly on your host machine as a standard process. This is the default behavior and provides the lowest friction for most development workflows.

### Characteristics

- **Environment**: Inherits your user's shell environment (PATH, installed tools).
- **Performance**: Zero overhead; runs at native speed.
- **Tools**: Uses the tools you already have installed (`cargo`, `npm`, `python`, `go`).
- **Networking**: Binds directly to localhost ports (managed by `locald`).

### Configuration

No special configuration is needed. Just define a command:

```toml
[services.web]
command = "npm run dev"
```

## 2. Container Execution (Opt-In)

In this mode, `locald` builds your application into an OCI container using Cloud Native Buildpacks (CNB) and runs it in an isolated environment. This mimics a production-like environment (like Heroku or Kubernetes).

### Characteristics

- **Environment**: Isolated Linux environment defined by the builder image.
- **Reproducibility**: Guarantees the same dependencies and OS versions as production.
- **Isolation**: File system and process isolation.
- **Overhead**: Requires building the container image (cached) and running via the embedded container runtime (via `locald-shim`).

### Configuration

To opt-in, add a `[service.build]` section:

```toml
[services.web]
build = { builder = "paketobuildpacks/builder:base" }
```

See [Cloud Native Builds](./builds.md) for more details.

## Choosing a Mode

| Feature           | Host Execution                          | Container Execution                                    |
| :---------------- | :-------------------------------------- | :----------------------------------------------------- |
| **Use Case**      | Rapid iteration, debugging, simple apps | Production parity, complex dependencies, CI/CD testing |
| **Dependencies**  | Must be installed on host               | Installed by Buildpack inside container                |
| **Startup Speed** | Instant                                 | Slower (build + container start)                       |
| **Isolation**     | Low (shared host)                       | High (namespaces, cgroups)                             |

We recommend starting with **Host Execution** for the fastest feedback loop and switching to **Container Execution** if you need strict isolation or are debugging production-specific issues.
