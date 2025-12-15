# Cloud Native Builds

`locald` provides an **opt-in** zero-configuration build system powered by **Cloud Native Buildpacks (CNB)**. This allows you to turn your source code into a runnable OCI image without writing a `Dockerfile`.

This feature is part of the **Container Execution** mode. See [Execution Modes](./execution-modes.md) for a comparison with the default Host Execution.

## How it Works

When you configure a service to use `build` (or run `locald build`), `locald`:

1.  **Detects**: Analyzes your source code to determine the language and framework (e.g., Rust, Node.js, Python).
2.  **Builds**: Uses a "Builder Image" (like Paketo or Heroku) to compile your code and install dependencies.
3.  **Exports**: Creates a standard OCI image that can be run by `locald` or any container runtime.

## Usage

### Basic Build

To build the project in the current directory:

```bash
locald build
```

This will:

- Pull the default builder image (if not present).
- Run the CNB lifecycle.
- Tag the resulting image with the project name.

### Configuration

You can configure the build process in your `locald.toml`:

```toml
[service.web]
build = { builder = "paketobuildpacks/builder:base" }
```

## Caching

`locald` leverages CNB's advanced caching mechanisms. Dependencies (like `node_modules` or Cargo crates) are cached in a separate Docker volume (or directory) and reused between builds, significantly speeding up subsequent builds.

## Rust Support

`locald` has first-class support for Rust. It uses standard buildpacks to handle `cargo build`, ensuring that you don't need to manage `rustup` or system dependencies on your host machine. The build runs in a consistent Linux environment, guaranteeing that your application works the same way in development as it does in production.

## Running the Image

Once built, `locald` needs to know how to start your application. It supports auto-detection of the start command from the buildpack metadata. See [Process Types & Start Commands](./process-types.md) for details.
