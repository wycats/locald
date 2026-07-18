# Axiom 12: The Source of Truth

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

## 3. Configuration, Identity, and Runtime State

We keep configuration, identity, and runtime authorities distinct and enforce the following boundaries:

- **Configuration**: Lives in the workspace and describes what the user wants to run.
- **Identity Catalog**: Lives in locald's platform data directory and records opaque project identities, current and historical path locators, and display metadata. It never shadows service configuration.
- **Single Writer**: The daemon holds an exclusive catalog writer lock for its complete stateful lifetime, including startup import and restart reconciliation.
- **Availability and Runtime State**: Lives in separate daemon-owned stores or memory and describes current demand, processes, and health.
- **No Bleed**: The daemon rereads workspace configuration when converging services. Catalog and runtime records identify the project and explain current state; they do not replace the project's configuration.
