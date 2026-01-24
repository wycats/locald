# Explicit Container Service Type

- Feature Name: `container_service_type`
- Start Date: 2025-12-08
- RFC PR: (leave empty)
- Issue: (leave empty)

## Summary

Introduce a dedicated `type = "container"` for services that run as OCI containers (Docker), separating them from the default `type = "exec"` which should be reserved for host-level processes.

## Motivation

Currently, the `exec` service type is overloaded. It behaves as a host process runner if `command` is present, but switches to a container runner if `image` is present. This creates ambiguity and makes schema validation looser than necessary.

By introducing an explicit `container` type, we can enforce stricter validation:

- `exec` services _must_ have a `command` and _cannot_ have an `image`.
- `container` services _must_ have an `image`.

## Design

We will add a new variant to the `ServiceConfig` enum.

### Configuration Schema

```toml
# New "container" type
[services.redis]
type = "container"
image = "redis:7"
container_port = 6379

# Existing "exec" type (Strict)
[services.web]
type = "exec" # Optional, implied if no type
command = "npm start"
# image = "..." # Error: 'image' not allowed for type 'exec'
```

### `ContainerServiceConfig` Struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ContainerServiceConfig {
    #[serde(flatten)]
    pub common: CommonServiceConfig,

    /// The OCI image to run.
    pub image: String,

    /// Arguments to pass to the container entrypoint.
    pub command: Option<String>,

    /// The port exposed by the container.
    pub container_port: Option<u16>,

    // Future: volumes, network_mode, etc.
}
```

### Backward Compatibility

To maintain backward compatibility, the `Legacy` variant of `ServiceConfig` (or the loose `ExecServiceConfig`) will continue to accept `image`. However, we will mark this usage as deprecated in documentation and potentially emit a warning in the CLI.

## Migration Strategy

1.  **Implement**: Add `Container` variant to `TypedServiceConfig`.
2.  **Support**: Update `locald-server` to handle `TypedServiceConfig::Container` by reusing the existing Docker runner logic.
3.  **Document**: Update `configuration.md` to recommend `type = "container"` for Docker services.
4.  **Deprecate**: Add a warning when `type = "exec"` (or implicit type) is used with `image`.

## Unresolved Questions

- Should `container_port` be required? Yes, if the service needs to be proxied. If it's a worker container, it might not need it.
