---
title: "Configuration Hierarchy & Workspaces"
stage: 3
feature: Configuration
---

# RFC: Configuration Hierarchy & Workspaces

## 1. Terminology

We standardize on the term **Workspace**.

- **Definition**: A collection of related projects, typically within a single git repository (monorepo).
- **Scope**: While multi-repo workspaces are possible (via submodules or explicit config), our primary focus is the single-repo use case (e.g., `frontend/` + `backend/`).

## 2. The Hierarchy & Provenance

Configuration is resolved in this order (highest priority last). Crucially, `locald` must be able to explain _where_ a value came from.

1.  **Global**: `~/.config/locald/config.toml`
    - _Scope_: User session.
    - _Use Case_: Defaults (theme, log level), global API keys.
2.  **Context** (Recursive): `.locald.toml` in parent directories.
    - _Discovery_: Walk up from project root. **Stop** at `$HOME`, `.git` root, or filesystem root.
    - _Use Case_: Directory-specific defaults (e.g., `~/Code/work` vs `~/Code/personal`).
3.  **Workspace**: `locald.workspace.toml` (Explicit) or Git Root (Implicit).
    - _Scope_: The repository root.
    - _Use Case_: Shared resources (DB), shared env vars, service relationships.
4.  **Project**: `locald.toml`
    - _Scope_: The service itself.
    - _Use Case_: Command, port, specific overrides.

### Provenance & Visibility

To avoid "magic config" confusion, we must provide tooling to inspect the layering.

- **CLI**: `locald config show --provenance` (or `--verbose`)

  ```text
  [service]
  command = "npm run dev"  (from ./locald.toml)
  port = 3000              (from ./locald.toml)

  [env]
  DATABASE_URL = "..."     (from ../locald.workspace.toml)
  RUST_LOG = "debug"       (from ~/.config/locald/config.toml)
  ```

- **Dashboard**: The "Configuration" tab for a service should visually show the cascade, perhaps with color-coding or a "Resolved from X" tooltip.

## 3. Merging Strategy (The "Crisp" Rules)

We avoid arbitrary deep merging. Merging behavior is defined by the _type_ of configuration.

### A. Environment Variables (`[env]`)

**Scope**: **Configuration Only**.

- **Use for**: Feature flags (`FEATURE_NEW_UI=true`), Modes (`RUST_LOG=debug`), Public config.
- **Do NOT use for**: Secrets, API Keys, Database URLs.
- **Behavior**: **Merge**. Child keys override Parent keys.

### B. Secrets (`[secrets]`)

**Scope**: **Credentials & Sensitive Data**.

- **Goal**: Never check secrets into git.
- **Mechanism**: Keys map to a _provider_ resolution, not a literal value.
- **Behavior**: **Overlay**. Secrets are resolved at runtime and injected into the environment, overriding any `[env]` defaults.
- **Examples**:

  ```toml
  [secrets]
  # Pass-through from the host environment (developer's shell)
  AWS_PROFILE = { source = "env", key = "AWS_PROFILE" }

  # Fetch from a secret manager (e.g., 1Password)
  OPENAI_KEY = { source = "command", cmd = "op read op://dev/openai/key" }
  ```

- _Future Direction (Workload Identity)_: Long-term, we aim to support identity projection, where `locald` can mint short-lived OIDC tokens for services or securely project cloud credentials (e.g., AWS temporary creds) without managing long-lived static secrets.

### C. Resources & Injection

**Scope**: **Infrastructure Dependencies**.

- **Behavior**: **Injection**.
- Resources defined in the Workspace (e.g., a Postgres DB) are **not** manually wired in `[env]`.
- Instead, when a project `depends_on` a resource, `locald` **automatically injects** the connection details (e.g., `DATABASE_URL`, `REDIS_HOST`) into the process environment.
- This ensures configuration remains portable and dynamic (ports can change without breaking config).

### D. Dependencies (`depends_on`)

**Behavior**: **Replace**.

- _Rationale_: Merging dependencies is confusing. If a project defines dependencies, it likely knows exactly what it needs. Inheriting a dependency on "auth-service" from a parent config is implicit magic that breaks easily.

### E. Scalars (Port, Command, Workdir)

**Behavior**: **Replace**.

- The most specific config wins.

## 4. Variable Interpolation & Namespace

To keep configuration DRY and portable, we support variable expansion.

### A. Syntax

- **Format**: `${VAR}` or `${NAMESPACE.VAR}`.
- **Escaping**: `\${VAR}` treats it as a literal.

### B. Allowed Scopes

Variables can be used in the following fields:

- `[service]`: `command`, `workdir`.
- `[env]`: Values can reference other variables.
- `[resources]`: Connection strings.

### C. The Namespace

The following variables are available for interpolation:

1.  **Configuration Env** (`${VAR}`):

    - Refers to the **final merged** set of environment variables from the `[env]` section.
    - _Example_: `BASE_URL = "http://${HOST}:${PORT}"`

2.  **Project Context** (`${project.*}`):

    - `${project.root}`: Absolute path to the project directory.
    - `${project.name}`: Name of the project (folder name or explicit).

3.  **Workspace Context** (`${workspace.*}`):

    - `${workspace.root}`: Absolute path to the workspace root.

4.  **Host Environment** (`${host.*}`):
    - Access to the shell environment where `locald` is running.
    - _Example_: `${host.USER}`.

### D. Resolution Order

1.  Merge all `[env]` layers.
2.  Resolve `${host.*}`, `${project.*}`, `${workspace.*}`.
3.  Resolve `${VAR}` references within `[env]` (topological sort to handle dependencies).
    - _Cycle Detection_: Circular references (A->B->A) result in an error.

## 5. Workspace Definition

How do we identify a Workspace?

1.  **Explicit**: Presence of `locald.workspace.toml`.
2.  **Implicit**: The root of the git repository (`.git/` folder).
    - If multiple `locald.toml` files are found within a git repo, the git root is implicitly the Workspace root.
    - We should **encourage** users to create `locald.workspace.toml` if we detect this structure, to make it explicit.

## 6. The Registry & "Always Up"

The **Registry** tracks known projects to enable "Always Up" functionality.

- **Location**: `~/.local/share/locald/registry.json`
- **CLI Design**:
  - `locald registry list`: Show all known projects, their paths, and status.
  - `locald registry clean`: Remove projects that no longer exist on disk.
  - `locald pin <path|name>`: Mark a project as `always_up`.
  - `locald unpin <path|name>`: Unmark.
- **Dashboard Design**:
  - A "Registry" or "All Projects" view.
  - "Pin" icon on every service card to toggle `always_up`.
  - Visual indicator for "Offline" projects (registered but not running).

## 7. Implementation Strategy

1.  **Global Config**: Implement `GlobalConfig` loading and the `locald config show` command (with provenance support structure).
2.  **Registry**: Implement the JSON store and the `pin`/`unpin` CLI commands.
3.  **Workspace & Merging**:
    - Implement the discovery logic (Git root / `locald.workspace.toml`).
    - Implement the "Crisp" merging logic for Env and Resources.
    - Update `ServiceConfig` to reflect the merged state.

## 8. Context Updates (Stage 3)

List the changes required to `docs/agent-context/` to reflect this feature as "current reality".

- [ ] Create `docs/agent-context/features/configuration-hierarchy.md`
- [ ] Update `docs/agent-context/architecture/configuration.md` (if exists) or create it.
- [ ] Update `docs/agent-context/plan-outline.md` to mark Phase 26/27 as complete.

## 9. Advanced Patterns & Future Considerations

To further rationalize the configuration space, we are considering these patterns found in advanced tooling (Terraform, Bazel, Kustomize):

### A. Execution Modes (Overlays)

- **Problem**: Running the same service in "Dev" vs "Test" vs "Simulated Prod" modes requires changing multiple env vars.
- **Solution**: Support mode-specific overrides.
- **Design**:

  ```toml
  [env]
  RUST_LOG = "info"

  [env.debug]
  RUST_LOG = "trace"
  ```

- **Usage**: `locald up --mode debug`

### B. Workspace Profiles (Stacks)

- **Problem**: In a large monorepo (50+ services), `locald up` is too heavy.
- **Solution**: Define named subsets of services.
- **Design**:
  ```toml
  # locald.workspace.toml
  [profiles.checkout]
  services = ["frontend", "cart-service", "payment-service"]
  ```
- **Usage**: `locald up --profile checkout`

### C. Configuration Contracts

- **Problem**: A service fails silently or confusingly if a required env var is missing.
- **Solution**: Explicitly declare _required_ configuration.
- **Design**:
  ```toml
  [contract]
  required_env = ["DATABASE_URL", "STRIPE_KEY"]
  ```
- **Behavior**: `locald` refuses to start the service if these are not resolved (from any source), providing a clear error message.
