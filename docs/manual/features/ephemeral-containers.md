# Ephemeral Containers

`locald` provides support for running ephemeral OCI containers, similar to `docker run`. This is useful for ad-hoc tasks, debugging, or running tools that are packaged as containers.

## Usage

The `locald container` command group manages ephemeral containers.

### Running a Container

To run a container, use `locald container run`:

```bash
locald container run <image> [command]...
```

**Examples:**

```bash
# Run a simple command
locald container run alpine echo "Hello World"

# Run an interactive shell (not yet fully supported)
locald container run -it ubuntu bash
```

### Options

- `--sandbox <name>`: Run in a specific sandbox environment.

## Architecture

Ephemeral containers are managed by the daemon (server mode) and executed via `locald-shim` using the embedded `libcontainer` runtime.

1.  **Pull**: The image is pulled from the registry (if not present) to the local OCI layout.
2.  **Unpack**: The image is unpacked into a bundle directory.
3.  **Spec Generation**: A runtime specification (`config.json`) is generated based on the image config and user arguments.
4.  **Execution**: `locald-shim bundle run --bundle <bundle-path> --id <id>` executes the bundle.
5.  **Cleanup**: The container and its bundle are removed after execution.

## Limitations

- **Networking**: Containers currently run in a separate network namespace but without full bridge networking. Port mapping is not yet fully implemented for ephemeral containers.
- **Interactive/Detached Modes**: Flags exist, but full TTY support and detached execution are not yet implemented end-to-end.
