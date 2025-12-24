---
title: "Axiom 12: The Source of Truth"
---


**"We discover, we don't invent."**

`locald` is a guest in the user's workspace. We derive our configuration and behavior from the existing artifacts in the project. We respect the decisions made by the user and the tools they use.

## 1. The Workspace is Authoritative

The user's source code and configuration files (`Cargo.toml`, `package.json`, `Procfile`, `run.toml`) are the primary source of truth.

- **Discovery over Configuration**: We prefer inferring configuration from standard files over requiring proprietary `locald.toml` entries.
- **No Shadow Configuration**: We do not maintain a hidden database of configuration that overrides the workspace files. If it's not in the file, it doesn't exist.

## 2. The Runtime Contract

The "Contract" depends on the execution mode:

### Host Execution (Default)

When running on the host, the **Shell Environment** is the contract.

- **Respect the User's Shell**: We inherit the user's `PATH` and environment variables (unless explicitly isolated).
- **Augment, Don't Replace**: We inject service-specific variables (`PORT`, `DATABASE_URL`) but rely on the user's installed tools (`cargo`, `npm`, `python`) to be present and correct.

### Container Execution (Opt-In)

When `locald` runs a container (via `[service.build]`), the **OCI Image Config** is the definitive contract.

- **Respect the Environment**: We preserve the `Env` defined in the image (e.g., `PATH`, `LD_LIBRARY_PATH`). We append to it, but we never blindly overwrite it.
- **Respect the User**: We run as the user defined in the image (or the buildpack lifecycle's requirement), not an arbitrary default.

## 3. State vs. Configuration

We strictly distinguish between **Configuration** (what the user wants) and **State** (what is currently happening).

- **Configuration**: Lives in the repository (Git). Immutable during a run.
- **State**: Lives in `.locald/` or memory. Ephemeral.
- **No Bleed**: We never store configuration in state files. We never rely on state to determine the desired configuration.

