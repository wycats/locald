# Rustdoc Service

## Context

In the wider ecosystem, running and viewing documentation (like `cargo doc --open`) often requires running a custom server, setting up a watcher, or manually rebuilding. While tools exist, the philosophy of "it's ok, you can do it yourself" creates friction for what should be a core, always-available workflow.

## Goal

We want to package up the "build and serve docs" workflow so it "just works" and is ultimately pluggable.

For `dotlocal` specifically, we want to have the project's own documentation always running and accessible, serving as a dogfooding exercise for this feature.

## Strawman Design

### 1. The "Rustdoc" Service Type

We could introduce a specialized service type or a configuration preset that:

1.  Runs `cargo doc` (possibly with `--watch` or via a file watcher).
2.  Serves the resulting `target/doc` directory via a static file server.
3.  Exposes it on a port.

### 2. Integration

In `locald.toml`, this might look like:

```toml
[[services]]
name = "docs"
command = "cargo doc --no-deps && python3 -m http.server 8000 --directory target/doc"
# ...
```

Or eventually a first-class plugin:

```toml
[[services]]
type = "rustdoc"
```

## Immediate Implementation (Dogfooding)

For the `dotlocal` project, we will add a service to `locald.toml` that:

1.  Builds the docs.
2.  Serves them.
3.  (Optionally) Watches for changes.

Since we don't have a built-in static file server or watcher in `locald` yet (other than what we might hack together), we can use standard tools.

A simple approach for now:

- Use a script or shell command that runs `cargo doc` and then serves the directory.
- Or use `cargo-watch` if available, or just a simple build-and-serve loop.

For `dotlocal`, we probably want to serve the workspace docs.
