---
title: Execution Modes
---

`locald` runs services in two modes: **Host Execution** (default) and **Container Execution** (experimental).

## Host Execution (Default)

Host execution runs your commands directly on your machine. It’s the lowest-friction path for local development.

- **Environment**: Uses your local tools (`cargo`, `npm`, `python`, `go`).
- **Performance**: No container overhead.
- **Networking**: Ports are assigned and injected via `$PORT`.

```toml
[services.web]
command = "npm run dev"
```

## Container Execution (Experimental)

Container execution and CNB builds are experimental and live in the experimental docs.

- See [Experimental: Execution Modes](/experimental/execution-modes/)
