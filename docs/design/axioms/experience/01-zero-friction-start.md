# Axiom 1: Decentralized Configuration (In-Repo)

**The source of truth for a project's configuration lives in the project's repository (e.g., `locald.toml`), not in a central registry.**

## Rationale

Centralized configuration (like `/etc/hosts` or a global `nginx.conf`) is brittle and hard to share. By keeping config in the repo:

1.  **Portability**: The config travels with the code. A new developer just clones and runs `locald up`.
2.  **Versioning**: Configuration changes are tracked in git alongside code changes.
3.  **Isolation**: Projects don't accidentally step on each other's global config (mostly).

## Implications

- **Discovery**: `locald` must be able to "discover" a project. This usually happens when the user runs a command _inside_ the repo.
- **Registration**: When a user runs `locald up` in a new repo, `locald` "registers" it. This registration is essentially a pointer: `Project Name -> /path/to/repo`.
- **Staleness**: If the repo is moved or deleted, the registration becomes stale. `locald` needs a way to prune these (e.g., on startup or via `locald prune`).
- **Hot Reloading**: If `locald.toml` changes, the daemon should ideally detect it (file watcher) and reload the service.

## Sensible Defaults

To truly achieve "Zero Friction", configuration must be optional where reasonable defaults exist.

- **Project Name**: Defaults to the directory name.
- **Domain**: Defaults to `{project_name}.localhost`.
- **Command**: Defaults to `Procfile` entry or Buildpack metadata.

This allows a user to simply run `locald up` in a fresh repo and get a working environment with valid URLs (`http://my-repo.localhost`) without creating a config file.
