# RFC 0079: Unified Service Trait

## Status

- **Status**: Recommended
- **Date**: 2025-12-08

## Context

Currently, `locald` manages services using a monolithic `ProcessManager` and a `ServiceRuntime` enum. This enum explicitly lists every supported runtime type:

```rust
pub enum ServiceRuntime {
    Process { ... },
    Docker { ... },
    Postgres { ... },
    None,
}
```

The `ProcessManager` contains large `match` blocks for every operation (`start`, `stop`, `status`, `inspect`, `shutdown`, `restore`). While enums are excellent for closed sets of variants, they become cumbersome when the set of variants needs to be extensible. Adding a new service type (e.g., Redis, MinIO, or a Static File Server) currently requires modifying the core manager in multiple places, creating high coupling between the manager and every specific service implementation.

As we plan to add more "managed" services (like the recently added `PostgresRunner`), this pattern will become unsustainable and error-prone. We need to move from a closed set of types (Enum) to an open set of behaviors (Trait).

## Proposal

We propose unifying all service types under a shared trait system, similar to how Cargo abstracts dependency sources (`Source` trait) or how web frameworks abstract middleware.

We will introduce two primary traits:

1.  **`ServiceFactory`**: Responsible for validating configuration and creating service instances.
2.  **`ServiceController`**: Responsible for the runtime lifecycle of a single service instance.

## Detailed Design

### 1. The `ServiceController` Trait

This trait defines the interface for interacting with a running (or stopped) service.

We split the lifecycle into **Preparation** (Build/Install) and **Execution** (Run), similar to Cargo's distinction between compiling and running.

```rust
#[async_trait]
pub trait ServiceController: Send + Sync + std::fmt::Debug {
    /// Unique identifier for this service instance (e.g., "postgres:15").
    fn id(&self) -> &str;

    /// Prepare the service for execution.
    /// This handles heavy lifting: downloading binaries, pulling Docker images,
    /// compiling code, or initializing data directories.
    ///
    /// This step is distinct from `start` to allow the UI to show "Building..."
    /// or "Downloading..." states separately from "Starting...".
    async fn prepare(&mut self) -> Result<()>;

    /// Start the service.
    /// This should be fast and idempotent. It assumes `prepare` has succeeded.
    async fn start(&mut self) -> Result<()>;

    /// Stop the service.
    async fn stop(&mut self) -> Result<()>;

    /// Get the current runtime state of the service.
    /// This returns the dynamic parts of the status (PID, Port, State).
    /// The Manager combines this with static config (Name, Domain) to form the full `ServiceStatus`.
    async fn read_state(&self) -> RuntimeState;

    /// Get a stream of logs from the service.
    async fn logs(&self) -> BoxStream<LogEntry>;

    /// Get metadata about the service (e.g., "port", "url", "connection_string").
    fn get_metadata(&self, key: &str) -> Option<String>;

    /// Execute a specific command on the service.
    /// This provides an escape hatch for capabilities like "reset", "snapshot", etc.
    /// Returns `NotSupported` if the service doesn't handle the command.
    async fn execute_command(&mut self, cmd: ServiceCommand) -> Result<()>;

    /// Serialize the runtime state for persistence.
    fn snapshot(&self) -> serde_json::Value;

    /// Restore runtime state from a snapshot.
    async fn restore(&mut self, state: serde_json::Value) -> Result<()>;
}

/// The dynamic runtime state of a service.
#[derive(Debug, Clone)]
pub struct RuntimeState {
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub status: ServiceState,
    pub health_status: HealthStatus,
}

#[derive(Debug, Clone)]
pub enum ServiceCommand {
    /// Reset the service to its initial state (e.g., wipe data).
    Reset,
    /// Custom command (e.g., "run-migration").
    Custom(String, Vec<String>),
}
```

### 2. The `ServiceFactory` Trait

This trait allows the system to pick the right implementation for a given configuration.
It acts as the dependency injection point, receiving the global context and passing relevant parts to the controller.

```rust
pub trait ServiceFactory: Send + Sync {
    /// Returns true if this factory can handle the given configuration.
    fn can_handle(&self, config: &ServiceConfig) -> bool;

    /// Creates a new controller for the given configuration.
    /// The `ServiceContext` is injected here, allowing the Factory to pass
    /// necessary dependencies (Docker, StateManager) to the Controller.
    fn create(&self, name: String, config: &ServiceConfig, ctx: &ServiceContext) -> Box<dyn ServiceController>;
}
```

### 3. `ServiceContext`

The context is passed to the Factory, not the Controller's methods. This aligns with the "Constructor Injection" pattern, keeping the runtime methods clean.

```rust
pub struct ServiceContext {
    pub docker: Arc<Docker>,
    pub state_manager: Arc<StateManager>,
    pub project_root: PathBuf,
    // ... other shared resources
}
```

### 4. Refactoring `ProcessManager`

The `ProcessManager` will no longer hold a `ServiceRuntime` enum. Instead, it will hold a collection of `Box<dyn ServiceController>`.

```rust
pub struct Service {
    pub config: LocaldConfig,
    pub controller: Box<dyn ServiceController>, // <--- Polymorphic
    // ...
}
```

When `start` is called, the Manager iterates through registered `ServiceFactory` implementations to find one that matches the config, creates the controller, and calls `controller.start()`.

## Proposed Implementations

We will refactor existing logic into the following implementations:

1.  **`ProcessController`**: Handles `ServiceConfig::Exec` (Host) and `ServiceConfig::Container` (runc/shim).
    - _Logic_: Spawns processes, manages PTYs, handles `locald-shim`.
2.  **`DockerController`**: Handles `ServiceConfig::Container` (Docker) and `ServiceConfig::Legacy`.
    - _Logic_: Talks to Docker daemon, manages container lifecycle.
3.  **`PostgresController`**: Handles `TypedServiceConfig::Postgres`.
    - _Logic_: Uses `locald-utils::postgres::PostgresRunner`.

## Future Possibilities

This architecture paves the way for:

- **`RedisController`**: Managed Redis instances (using `embedded-redis` or Docker).
- **`StaticSiteController`**: A simple internal file server for static assets.
- **`ProxyController`**: If we want to manage the proxy itself as a service.
- **`ComposeController`**: A meta-controller that manages a group of services? (Maybe too complex for now).

## Migration Strategy

1.  **Define Traits**: Create the traits in `locald-core`.
2.  **Implement One**: Refactor `PostgresRunner` to implement `ServiceController` first (it's the cleanest).
3.  **Hybrid Manager**: Update `ProcessManager` to support both `ServiceRuntime` (legacy) and `Box<dyn ServiceController>`.
4.  **Migrate Rest**: Port `Process` and `Docker` logic to controllers.
5.  **Cleanup**: Remove `ServiceRuntime` and the hybrid logic.

## Open Questions

1.  **Health Checks**: Should health checks be part of the trait (`check_health()`) or remain in a separate `HealthMonitor`?
    - _Opinion_: Keep `HealthMonitor` separate but allow the controller to provide a "health probe" strategy.
2.  **Configuration Updates**: How to handle config changes? `controller.update(new_config)`? Or destroy and recreate?
    - _Opinion_: Destroy and recreate is safer and simpler (Immutable Infrastructure principle).
