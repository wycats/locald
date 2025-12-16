# Static Sites

`locald` includes a built-in static file server optimized for local development. It can serve static assets, documentation, or simple HTML sites directly from your project directory.

## Configuration

To define a static site service, use `type = "site"` in your `locald.toml`.

```toml
[services.docs]
type = "site"
path = "docs"
```

### Options

- **`type`**: Must be `"site"`.
- **`path`**: The directory to serve, relative to the project root.
- **`build`** (Optional): A command to run before serving (and whenever files change).
- **`port`** (Optional): A specific port to bind to (default: random ephemeral port).

## Features

### 1. Directory Listing

If a directory does not contain an `index.html` file, `locald` automatically generates a directory listing. This is useful for browsing file structures or serving collections of files.

### 2. Live Reloading

`locald` watches the served directory for changes.

- If a `build` command is configured, it re-runs the build on change.
- The browser automatically refreshes when files change (via the injected toolbar).

### 3. Development Toolbar

`locald` injects a small JavaScript toolbar into HTML pages. This toolbar provides:

- **Connection Status**: Shows if the dev server is connected.
- **Logs**: Streams build logs and server errors directly to the browser console.

## Example: Built Static Site

For a site that needs a build step (like a static site generator):

```toml
[services.my-site]
type = "site"
path = "dist"
build = "npm run build"
```

`locald` will:

1.  Run `npm run build`.
2.  Serve the `dist` directory.
3.  Watch for changes in the project.
4.  Re-run `npm run build` and refresh the browser on changes.
