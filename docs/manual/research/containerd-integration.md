# Research: Containerd Integration

**Question**: Could we interop with `containerd` (or another container system with good Rust integration) to run containers ourselves without Docker? How crazy would that be?

## Context

Currently, `locald` integrates with Docker via the `bollard` crate, which talks to the Docker Daemon API. This requires the user to have Docker Desktop or Docker Engine installed and running.

The user is asking if we can bypass the Docker Daemon and interface directly with a lower-level container runtime like `containerd` to run containers. This would potentially allow `locald` to run containers without a full Docker installation, or at least without the heavy Docker Daemon dependency.

## Feasibility Analysis

### 1. Containerd vs. Docker

- **Docker**: A high-level platform that includes a daemon, CLI, image builder, and registry client. It uses `containerd` under the hood.
- **Containerd**: An industry-standard container runtime with an emphasis on simplicity, robustness and portability. It manages the complete container lifecycle of its host system: image transfer and storage, container execution and supervision, low-level storage and network attachments, etc.

### 2. Rust Integration

- **`containerd-client`**: There are Rust crates for interfacing with `containerd`'s GRPC API (e.g., `containerd-client` or direct GRPC generation).
- **Complexity**: Interfacing with `containerd` is significantly lower-level than Docker. You have to manage:
  - **Namespaces**: Creating and managing Linux namespaces.
  - **Rootfs**: Setting up the root filesystem (overlayfs, snapshots).
  - **CNI**: Configuring networking (Container Network Interface) manually. Docker does this for you (bridge networks, port mapping). With `containerd`, you often need CNI plugins.
  - **Images**: Pulling and unpacking images (containerd handles this, but you invoke it).

### 3. "How Crazy Would That Be?"

- **Moderate to High Craziness**: It is not "crazy" in the sense of impossible—Kubernetes does it (via CRI). But for a local development tool, it adds a massive amount of responsibility.
- **Networking**: The hardest part is networking. Docker provides a very convenient bridge network and DNS resolution. Replicating that with raw CNI plugins in Rust is non-trivial.
- **Cross-Platform**: `containerd` works on Linux. On macOS/Windows, Docker uses a VM (LinuxKit). If we used `containerd` directly, we would still need a Linux VM on non-Linux platforms. We'd essentially be rebuilding Docker Desktop.

## Alternatives

1.  **Podman**: Podman is daemonless and rootless. It has a Docker-compatible API (socket) that `bollard` can already talk to. This might be a better "lightweight" alternative than raw `containerd`.
2.  **Wasm**: For some services, compiling to WebAssembly (Wasm/WASI) might be a future path, but databases like Postgres aren't quite there yet.
3.  **Embedded Postgres**: For the specific use case of "just add postgres", we could use a wrapper around a native postgres binary (downloaded on the fly) rather than a container.

## Conclusion

Direct `containerd` integration is likely **too heavy** for `locald` at this stage, primarily due to the cross-platform VM requirement (on Mac/Windows) and networking complexity.

**Recommendation**:

- Stick with Docker API (`bollard`) for now.
- Investigate **Podman** support as a lighter alternative.
- For "Managed Services" (like "just give me Postgres"), we can abstract away the Docker details so the user doesn't _know_ they are using Docker, even if we use it under the hood.
