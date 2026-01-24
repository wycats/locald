# Embedded Static Server

## Context

We currently rely on `python3 -m http.server` for serving static files (like documentation). This introduces a Python dependency and provides a very basic, unstyled directory listing.

## Goal

Embed a high-quality static file server directly into the `locald` binary. This server should:

1.  Be available via a CLI command (e.g., `locald serve`).
2.  Provide a modern, clean UI for directory listings.
3.  Support standard static file serving features (MIME types, etc.).
4.  Remove the need for external tools like Python or `miniserve` for basic workflows.

## Design

### CLI Interface

```bash
locald serve [PATH] [--port <PORT>] [--bind <HOST>]
```

- `PATH`: Directory to serve (default: current directory).
- `--port`: Port to listen on (default: 8080 or random?).
- `--bind`: Interface to bind to (default: 0.0.0.0).

### Implementation

We will use `axum` and `tower-http` (which are already dependencies of `locald-server`) to implement the server.

#### Directory Listing

`tower-http`'s `ServeDir` provides basic directory listing but the styling is minimal/non-existent (or maybe it doesn't do listing by default? It does if enabled, but it's basic).

To achieve a "decent UI", we might need to:

1.  Intercept requests for directories.
2.  Generate a custom HTML page listing the files.
3.  Use a simple embedded CSS for styling.

Alternatively, we can check if `tower-http` allows customizing the listing renderer. (It usually generates simple HTML).

If `tower-http`'s listing is too basic, we can implement a custom handler for directories that reads the directory and renders a template.

### "In-Line" Implementation Plan

1.  Add `serve` subcommand to `locald-cli`.
2.  Implement the server logic in `locald-server` (or a new module in `locald-cli` if we want to keep it lightweight, but `locald-server` has the deps).
3.  Since `locald-cli` depends on `locald-server`, we can expose a `run_static_server` function in `locald-server`.

## Strawman UI

The directory listing should look like a modern file browser:

- Header with current path.
- List of files/folders with icons (SVG).
- File sizes and modification times.
- "Parent Directory" link.
- Dark/Light mode support (system preference).

## Integration

Update `locald.toml` to use:

```toml
command = "cargo doc ... && locald serve target/doc --port $PORT"
```
