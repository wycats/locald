# Phase 58.2 DDD Audit — Configuration Parsing

Date: 2026-01-28  
Scope: `locald` configuration system (`crates/locald-core/src/config/`)

## 1. Schema Overview

- **Top-level config (`locald.toml`)**
  - `LocaldConfig` includes `project`, `services`, and `plugins`. `project` is required. `services` and `plugins` default to empty maps.
  - Source: `crates/locald-core/src/config/mod.rs`

- **Project**
  - `ProjectConfig` fields: `name` (required), `domain`, `start`, `build`.
  - `domain` default is _runtime-derived_ as `${project}.localhost` when missing (not in parsing defaults).
  - Sources: `crates/locald-core/src/config/project.rs`, `crates/locald-core/src/config/mod.rs`

- **Services**
  - `ServiceConfig` is untagged: either `ServiceKind::Procfile` (`file`) or legacy exec-style.
  - Sources: `crates/locald-core/src/config/service.rs`, `crates/locald-core/src/config/mod.rs`
  - Common fields: `env`, `cwd`, `depends_on`, `health_check`, `stop_signal`.
  - Source: `crates/locald-core/src/config/service.rs`

- **Plugins**
  - `PluginRef` map supports remote URLs (optional checksum), local path, or installed plugin reference.
  - Source: `crates/locald-core/src/config/plugin.rs`

- **Build config**
  - `BuildConfig.builder` defaults to `heroku/builder:22`.
  - Source: `crates/locald-core/src/config/build.rs`

- **Health checks**
  - `HealthCheck` supports either a command string or a probe with `type`, `path`, `interval`, `timeout`, `retries`.
  - Source: `crates/locald-core/src/config/health.rs`
  - Runtime uses fixed polling delays (250ms) and ignores `interval`/`timeout`.
  - Source: `crates/locald-server/src/health.rs`

- **Global config (separate from `locald.toml`)**
  - `GlobalConfig` includes `privileged_ports` (default `true`) and `updates.auto_check` (default `false`).
  - Source: `crates/locald-core/src/config/global.rs`

- **JSON Schema**
  - `LocaldConfig` derives `JsonSchema`, and `locald schema` outputs JSON Schema using `schemars`.
  - Sources: `crates/locald-core/src/config/mod.rs`, `crates/locald-cli/src/commands/schema.rs`

## 2. Friction Points (by Persona)

### New Hire

- **Reference docs incomplete**: Missing `plugins`, `Procfile` service type, `stop_signal`, `depends_on`, and config layering files (`context.toml`, `workspace.toml`).
  - Docs: `docs/manual/reference/configuration.md`
  - Schema: `crates/locald-core/src/config/mod.rs`, `crates/locald-core/src/config/service.rs`
- **`domain` optional in schema but required in docs**: The `domain` field is `Option<String>`, but docs claim it is required.
  - Schema: `crates/locald-core/src/config/project.rs`, Docs: `docs/manual/reference/configuration.md`
- **Health check interval/timeout documented but not applied**: Docs list `interval`/`timeout`, but runtime ignores them.
  - Docs: `docs/manual/reference/configuration.md`, Runtime: `crates/locald-server/src/health.rs`
- **Config hierarchy mismatch**: Docs list Global → Context → Workspace → Project, but code merges Context after Workspace (Context overrides Workspace).
  - Docs: `docs/manual/reference/configuration.md`
  - Code: `crates/locald-core/src/config/loader.rs`

### Security Auditor

- **Config contents logged**: Full `LocaldConfig` content is logged on parse, which can leak secrets (e.g., `env` values).
  - Source: `crates/locald-core/src/config/loader.rs`
- **Privileged ports default to true**: Default attempts to bind 80/443, increasing privileged surface.
  - Source: `crates/locald-core/src/config/global.rs`
- **Remote plugin URLs allow unchecked downloads**: Checksums are optional, enabling unsigned remote plugin use.
  - Source: `crates/locald-core/src/config/plugin.rs`

### Test Engineer

- **Upstream config parse failures are silent**: Invalid `context.toml` or `workspace.toml` are ignored without diagnostics.
  - Source: `crates/locald-core/src/config/loader.rs`
- **No explicit CLI override for config path**: Config is bound to CWD; no `--config` or override entry point observed in loader.
  - Source: `crates/locald-core/src/config/loader.rs`
- **Unknown fields ignored**: No `deny_unknown_fields`; typos are silently accepted, complicating tests.
  - Source: `crates/locald-core/src/config/mod.rs`

### SRE

- **Programmatic management only via JSON schema + file I/O**: `locald schema` exists, but there is no formal API for layered config or global config schema.
  - Source: `crates/locald-cli/src/commands/schema.rs`
- **Layered env is file-driven only**: `context.toml`, `workspace.toml`, `locald.toml` are required for overrides; no environment variable overlay for services.
  - Source: `crates/locald-core/src/config/loader.rs`

## 3. Recommendations (Prioritized)

1. **Doc + schema alignment (High)**
   - Update reference docs to include `plugins`, `Procfile`, `stop_signal`, `depends_on`, `build`, `context.toml`, and Procfile fallback.
   - Clarify `domain` optionality or make it required in schema.
   - Fix the config hierarchy description or adjust merge order to match docs.

2. **Error handling & diagnostics (High)**
   - Emit actionable errors for invalid upstream configs instead of silent ignore. Include path and line/column for TOML errors.
   - Add optional unknown-field warnings (or `deny_unknown_fields`) for `locald.toml` to catch typos.

3. **Security posture (High)**
   - Avoid logging raw config content; redact or hash `env` values.
   - Consider making `privileged_ports` opt-in or guard with explicit user confirmation.
   - Encourage checksum usage for remote plugin URLs or add warnings when missing.

4. **Health-check correctness (Medium)**
   - Either implement `interval`/`timeout` in health monitors or remove them from schema/docs to prevent false expectations.

5. **Testing & automation ergonomics (Medium)**
   - Add a `--config` override (or `LOCALD_CONFIG`) to make configuration deterministic in test harnesses.

## 4. Code References

- Schema definitions:
  - `crates/locald-core/src/config/mod.rs`
  - `crates/locald-core/src/config/*.rs`
- Config discovery & layering:
  - `crates/locald-core/src/config/loader.rs`
  - `crates/locald-core/src/config/global.rs`
- Logging of raw config:
  - `crates/locald-core/src/config/loader.rs`
- Schema output (`locald schema`):
  - `crates/locald-cli/src/commands/schema.rs`
  - `crates/locald-core/src/config/mod.rs`
- Health check runtime behavior:
  - `crates/locald-server/src/health.rs`
- Docs (reference + hierarchy):
  - `docs/manual/reference/configuration.md`
  - `docs/manual/concepts/config-hierarchy.md`
