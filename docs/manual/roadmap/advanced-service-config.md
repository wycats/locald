# Design: Advanced Service Config (Phase 30)

> **Roadmap**: This is a proposal for post-MVP service orchestration refinements.

## The Problem

`locald.toml` is intentionally minimal. Some users will eventually need more advanced service behaviors without importing a full orchestrator.

## Proposed Additions

### Service Templates

Allow defining a template once and reusing it across services with small overrides.

```toml
[templates.web]
image = "ghcr.io/acme/web:latest"
healthcheck = "/health"
ports = ["3000"]

[services.api]
use = "templates.web"
command = "./bin/api"
```

### Startup and Dependency Ordering

Express lightweight dependencies without complex graphs:

```toml
[services.db]
healthcheck = "pg_isready"

[services.api]
wait_for = ["db"]
```

### Lifecycle Hooks

Provide optional hooks for pre-start and post-start behaviors.

```toml
[services.api]
pre_start = "./bin/migrate"
```

## Principles

- Keep the surface minimal.
- Opt into complexity explicitly.
- Make advanced features composable but not required.
