# OCI Example

This example demonstrates how to use the `locald` crates (`locald-oci`, `locald-shim`) to pull an OCI image and run it as a container using the embedded `libcontainer` runtime in `locald-shim`.

## Prerequisites

- Linux
- A privileged `locald-shim` installed (setuid root). This is normally done once via `sudo locald admin setup`.
  - When developing from this repo, you can also run: `sudo target/debug/locald admin setup`

## Usage

```bash
# Build the example (and `locald` if you need admin setup)
cargo build -p oci-example --bin locald

# One-time: install/repair the privileged shim (required for container execution)
sudo target/debug/locald admin setup

# Run the example
cargo run -p oci-example -- alpine:latest echo "Hello World"
```

## How it works

1.  **Pull**: Uses `locald-oci` to pull the image from a registry to a local OCI layout.
2.  **Unpack**: Unpacks the image rootfs.
3.  **Spec**: Generates an OCI runtime spec (`config.json`) that uses user namespaces for user mapping.
4.  **Run**: Invokes a _privileged_ `locald-shim` (setuid root) which uses `libcontainer` to execute the container directly.

Note: In `locald`, “rootless” means there is no root daemon and no interactive sudo prompts in the hot path. Container execution still uses a privileged shim.
