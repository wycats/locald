# Redis Plugin Example

This is an example locald plugin that provides Redis service configuration.

## Building

```bash
# Add wasm32-wasip1 target if not already installed
rustup target add wasm32-wasip1

# Build the plugin
cargo build --release --target wasm32-wasip1
```

The built plugin will be at `target/wasm32-wasip1/release/redis_plugin.wasm`.

## Installing

```bash
# Install to current project
locald plugin install target/wasm32-wasip1/release/redis_plugin.wasm --project

# Or install globally
locald plugin install target/wasm32-wasip1/release/redis_plugin.wasm
```

## Usage

Once installed, any service with `kind = "redis"` in your `locald.toml` will be handled by this plugin:

```toml
[services.cache]
kind = "redis"
version = "7"  # Optional, defaults to "7"
```

The plugin will:
1. Declare a container service using the `redis` image
2. Allocate a port for the Redis server
3. Set the `REDIS_PORT` environment variable for dependent services

## How It Works

The plugin implements two functions:

- **detect**: Returns the Redis version if the service kind is "redis"
- **apply**: Creates a plan with steps to:
  1. Allocate a port for Redis
  2. Declare a container service with the Redis image

## Development

This plugin demonstrates the minimal locald plugin contract:

```rust
// Implement the exported `plugin` interface
impl Guest for Component {
    fn detect(ctx: WorkspaceContext, spec: ServiceSpec) -> Option<String> { ... }
    fn apply(ctx: WorkspaceContext, caps: HostCapabilities, spec: ServiceSpec) 
        -> Result<Plan, Diagnostics> { ... }
}
```
