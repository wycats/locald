# RFC 0075: Runc Container Runtime & Unified Execution

## Summary

This RFC proposes establishing `runc` as the primary container runtime for `locald`, replacing the dependency on the Docker daemon for containerized services. It also outlines a refactoring plan to unify the execution logic for both Cloud Native Buildpacks (CNB) and generic OCI containers (`type = "container"`).

## Motivation

### 1. Daemonless Architecture

The current `DockerRuntime` relies on a long-running Docker daemon. This introduces:

- **Complexity**: Users must install and manage Docker Desktop or Engine.
- **Privilege Issues**: Docker often requires root or special group membership.
- **Resource Overhead**: The daemon consumes resources even when idle.

By using `runc` directly, `locald` becomes the container manager, spawning containers as child processes. This aligns with our "Daemon-First" axiom but keeps the runtime ephemeral.

### 2. Cross-Platform Strategy

To clarify the distinction between the _infrastructure_ (VM) and the _runtime_ (Process Spawner):

- **Linux**: `locald` runs natively. `runc` spawns containers directly on the host kernel.
- **Windows (WSL2)**: `locald` runs **inside** the WSL2 Linux environment. `runc` spawns containers using the WSL2 Linux kernel.
- **macOS (Lima)**: `locald` runs **inside** a Lima Linux VM. `runc` spawns containers using the Lima Linux kernel.

In all cases, `locald` and `runc` operate within a Linux environment. We do not use WSL or Lima as the "runtime" itself; rather, they provide the Linux kernel required by `runc`.

### 3. Unified Codebase

Currently, `locald` has separate code paths for:

- Host processes (`start_host_process`)
- CNB Containers (`start_cnb_container`)
- Docker Containers (`DockerRuntime`)
- Generic Containers (`start_container` - recently added)

There is significant duplication between the CNB and Generic container paths. Both involve:

1.  Preparing a rootfs.
2.  Generating an OCI `config.json`.
3.  Setting up a PTY.
4.  Invoking `locald-shim` to run `runc`.

## Design

### 1. The `ContainerBundle` Abstraction

We will introduce a `ContainerBundle` abstraction in `locald-builder` (or a new `locald-runtime` crate). This struct represents a ready-to-run OCI bundle.

```rust
pub struct ContainerBundle {
    pub dir: PathBuf,
    pub config: oci::Spec,
}

impl ContainerBundle {
    /// Creates a new bundle directory and populates it with rootfs and config.json
    pub async fn create(path: &Path, rootfs_source: &Path, config: oci::Spec) -> Result<Self>;

    /// Returns the path to the bundle directory
    pub fn path(&self) -> &Path;
}
```

### 2. `BundleSource` Trait

We need a way to produce a bundle from different sources.

```rust
#[async_trait]
pub trait BundleSource {
    /// Prepares the rootfs and returns the OCI configuration
    async fn prepare(&self, destination: &Path) -> Result<ContainerBundle>;
}
```

We will implement this for:

- **`CnbSource`**: Builds the app using CNB, extracts the run image, and overlays the application layer.
- **`ImageSource`**: Pulls a generic OCI image and extracts it to a rootfs.

### 3. Unified `ProcessRuntime`

The `ProcessRuntime` in `locald-server` will be simplified. Instead of separate methods, it will have a generic `start_container` method that accepts a `BundleSource`.

```rust
impl ProcessRuntime {
    pub async fn run_container<S: BundleSource>(
        &self,
        name: String,
        source: S,
        env: HashMap<String, String>
    ) -> Result<ServiceHandle> {
        // 1. Prepare Bundle
        let bundle = source.prepare(&runtime_dir).await?;

        // 2. Setup PTY & Shim
        let pair = Self::create_pty()?;
        let shim = ShimRuntime::find_shim()?;

        // 3. Execute
        // runc run --bundle <bundle.path> <id>
        // ...
    }
}
```

## Refactoring Plan

### Phase 1: Extract OCI Logic to `locald-oci`

We will create a new crate `locald-oci` to hold the low-level OCI primitives.

1.  Create `locald-oci` crate.
2.  Move `runtime_spec.rs` and `oci_layout.rs` from `locald-builder` to `locald-oci`.
3.  Update `locald-builder` to depend on `locald-oci`.

### Phase 2: Consolidate `start_container` and `start_cnb_container`

1.  Analyze `locald-server/src/runtime/process.rs`.
2.  Identify the common "Execution Phase" (PTY + Shim + Command).
3.  Extract this into a private helper `spawn_runc_process`.

### Phase 3: Implement `BundleSource`

1.  Refactor `ContainerImage` in `locald-builder` to implement `BundleSource`.
2.  Refactor `Lifecycle` (CNB) to implement `BundleSource` (or a wrapper around it).

### Phase 4: Deprecate DockerRuntime

1.  Update `locald-server` to use the new unified path for all `type = "container"` services.
2.  Mark `DockerRuntime` as deprecated.
3.  Eventually remove `DockerRuntime` once the `runc` path is stable on all platforms.

## Future Work

- **Rootless Containers**: Investigate running `runc` in rootless mode to avoid `sudo` requirement for the shim in some cases.
- **Network Namespaces**: Currently we use host networking. We may want to introduce CNI plugins for isolated networking in the future.
