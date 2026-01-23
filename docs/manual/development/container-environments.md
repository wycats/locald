# Container Development Environments

`locald` is **host-first**. It does not support running inside containers. This keeps privilege acquisition reliable and the workflow predictable.

That said, **building inside a container and running on the host works great**. If you prefer containerized toolchains (Rust, Node, etc.), you can still use them for builds while running `locald` on the host.

## Supported Patterns

### 1) Run `locald` on the Host (Recommended)

```bash
# On the host
sudo locald admin setup
locald up
```

### 2) Build in a Container, Run on the Host

```bash
# Inside a container
cargo build --release

# On the host
./target/release/locald up
```

### 3) Optional: Call `locald` from Inside a Container

If you need CLI access inside a container, **expose the host binary into the container** using your container tooling (for example, a bind mount or export step). Then run `locald` as usual from inside the container.

This keeps the architecture simple: `locald` runs on the host, but you can invoke it from inside your container.

## Why We Don’t Support “Run locald in a Container”

The inverse workflow (locald inside a container delegating to the host) proved too fragile:

- Requires brittle environment detection heuristics
- Relies on IPC across container boundaries
- Adds a second daemon with its own lifecycle and failure modes

We removed this entire stack to keep the core workflow reliable.

## Troubleshooting

### "environment.container" Warning

If you see a container warning, run `locald` on the host OS.

### "shim not found" or "not setuid"

Run the setup step on the host:

```bash
sudo locald admin setup
```
