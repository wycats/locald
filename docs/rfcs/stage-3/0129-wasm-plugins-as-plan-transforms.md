<!-- exo:129 ulid:01krkpxdvcjazwz86ytz2tn03k -->

# RFC 129: WASM Plugins as Plan Transforms


# RFC 0129: WASM Plugins as Plan Transforms

## 1. Summary

Define a **WASM plugin** mechanism for `locald` where plugins perform **data-to-data** transformation:

- Input: `WorkspaceContext` + `HostCapabilities` + user `ServiceSpec`
- Output: a **`ServicePlan` DAG** composed of small, host-executed primitives

The `ServicePlan` is the stable extensibility seam: the host owns execution, safety, portability, and runtime semantics; the plugin owns intent and translation.

**Canonical contract format:** WIT (WebAssembly Interface Types). The host may provide a JSON debug serialization for inspection/logs/tests, but JSON is not the canonical interface.

This RFC is intended to be the concrete mechanism behind the umbrella goal in [docs/rfcs/0038-extensibility.md](docs/rfcs/0038-extensibility.md).

## 2. Motivation

We want extensibility without reintroducing the “closed set” problem:

- Adding new service types should not require changing `locald` core.
- Plugins should be distributable and sandboxable.
- Portability should be host-owned (avoid implicit dependence on `/bin/sh`, `awk`, etc.).
- The “service contract” must remain coherent as `locald` evolves.

A key tension: a powerful plugin system can accidentally become a second programming/config language.

This proposal’s response:

- WASM is already a programming language.
- The host/plugin interface should be a **small, typed plan IR** (not a second language).

## 3. Detailed Design

### 3.1 Terminology

- **Plugin**: a WASM component that produces a `ServicePlan` from context + a user `ServiceSpec`.
- **ServiceSpec**: user-facing service input provided to the plugin.
- **ServicePlan**: a host-executed DAG of steps; steps are primitive operations (allocate port, pull OCI, declare service, etc.).
- **Capabilities**: explicit host features granted to the plugin (and optionally requested by the plugin).
- **WIT Canonical Contract**: authoritative schema + ABI for host-plugin communication.
- **Debug View**: host-generated JSON representation of a plan for inspection only.

### 3.2 Prior Art

- Cloud Native Buildpacks: detect/apply phases; caching as an opt-in capability.
- Terraform plan: plan as stable IR; apply as host execution.
- WASI/WIT: versioned typed boundaries.
- Dataflow DAGs: explicit dependencies and deterministic scheduling.

### 3.3 Non-Goals

- Implementing the full `ServiceController` trait inside WASM.
- Allowing plugins to spawn arbitrary host processes directly.
- Building a general-purpose IR-side scripting/templating language.
- Nailing a perfect plugin authoring SDK in Phase 29.

### 3.4 Shape of the System

At a high level:

1. `locald` discovers and loads WASM plugins.
2. For a service declaration, the host calls the plugin.
3. The plugin returns a fully-resolved plan (or diagnostics).
4. The host validates capabilities + plan structure and executes deterministically.
5. The host materializes services using host runtimes (e.g. `process`, `container`, `postgres`).

Important: the `container` runtime refers to `locald`’s host-owned OCI execution path. Plugins MUST NOT assume the user has installed Docker or that `locald` will use the user’s Docker daemon.

### 3.5 Plan Model: DAG with Deterministic Linearization

We represent a plan as a set of steps with explicit dependencies.

- Each step has a unique `id`.
- Each step may declare `needs: list<string>` referencing other step IDs.
- The host executes a deterministic topological sort (stable tie-break by lexical `id`).

#### 3.5.1 Deriving the DAG from User Dependencies

Users already express “B depends on A.” This is preserved by including dependency information in the input (`ServiceSpec.depends_on`). Plugins can propagate those edges into the plan by adding `needs` edges between the relevant steps.

Constraint: the plan must be fully resolved (no runtime branching/graph mutation by the host).

### 3.6 Keep the IR Small: “No Second Language”

Conditionals and loops live in plugin code (WASM). The IR stays small:

- primitive ops
- typed literals
- explicit references to earlier outputs

If a plugin needs to “loop,” it emits repeated steps.

### 3.7 Capability Model

Baseline:

- Default sandbox: WASI-only (no host filesystem)
- Optional capability: `cache_dir`
  - Scoped to plugin id + workspace id
  - Host may delete contents at any time
  - Best-effort cache only (no secrets, no correctness dependence)
- Optional capability: `state_dir`
  - Scoped to plugin id + workspace id
  - Host should attempt to preserve contents, but may enforce quotas and may clear state
  - Plugins must handle missing/cleared state gracefully

Other potential capabilities:

- `oci_pull`
- `read_workspace`
- `write_workspace` (expected denied by default)

### 3.8 Evolution and Compatibility

Two evolving surfaces:

1. Plugin ABI (WIT)
2. Plan IR schema (ops/types)

#### Version Negotiation Algorithm (Normative)

- Host provides `supported_ir_versions`.
- Plugin MUST choose an `ir_version` contained in `supported_ir_versions`.
- Host MUST reject plans whose `ir_version` is not supported.

### 3.9 Debug View (JSON)

The host should provide `locald plugin inspect <plugin>` that shows metadata, capabilities, and a normalized plan debug JSON.

JSON is not used as an input contract; it is a view.

### 3.9.1 Conformance Fixtures

Treat WIT + IR version as a conformance surface.

Maintain golden fixtures:

- Inputs: `(workspace-context, host-capabilities, service-spec)`
- Expected outputs:
  - normalized plan debug JSON snapshots and/or
  - expected validation outcomes (capability mismatch, version mismatch, etc.)

#### Debug JSON Normalization Guidelines

- Sort `steps` by `id`.
- Emit fields in stable order.
- Redact host-local paths and machine-specific identifiers.
- Exclude execution artifacts (timestamps/durations) from fixture debug view.

#### Example Fixture

Input (conceptual):

- `workspace-context.workspace_id = "demo"`
- `host-capabilities.supported_ir_versions = [1]`
- `host-capabilities.granted = ["oci_pull"]`
- `service-spec.kind = "redis"`
- `service-spec.config.image = "redis:7"`

Expected normalized plan debug JSON (illustrative):

```json
{
  "ir_version": 1,
  "requested_capabilities": ["oci_pull"],
  "steps": [
    {
      "id": "port",
      "needs": [],
      "op": { "allocate_port": { "name": "redis" } }
    },
    {
      "id": "pull",
      "needs": [],
      "op": { "oci_pull": { "image": "redis:7" } }
    },
    {
      "id": "service",
      "needs": ["port", "pull"],
      "op": {
        "declare_service": {
          "name": "redis",
          "runtime": "container",
          "settings": [
            ["image", { "lit": { "string": "redis:7" } }],
            [
              "port",
              { "get": { "step_id": "port", "path": [{ "field": "port" }] } }
            ]
          ]
        }
      }
    }
  ]
}
```

### 3.9.2 Initial Operations and Contracts (Stage 1 Acceptance Criteria)

Stage 1 is “ready” when implementers can build the host executor and a minimal plugin without guessing.

#### `allocate-port`

- Inputs: `name: string`
- Capabilities: none
- Outputs: `port: u16`

#### `oci-pull`

- Inputs: `image: string`
- Capabilities: `oci_pull`
- Outputs: none (initially)

#### `declare-service`

- Inputs:
  - `name: string`
  - `runtime: string`
  - `settings: list<tuple<string, expr>>`
- Capabilities: none (initially)
- Validation:
  - `runtime` must be host-supported (`container`, `process`, `postgres`)
  - unknown `settings` keys rejected with actionable diagnostics
  - `expr.get` must reference existing steps and valid output paths

##### Runtime identifiers and `settings` keys (v1)

General rule: unknown keys MUST be rejected.

`runtime = "container"`

- `image: string` (required)
- `port: expr` (optional)
- `command: list<string>` (optional)
- `env: record<string, string>` (optional)

`runtime = "process"`

- `command: list<string>` (required)
- `port: expr` (optional)
- `env: record<string, string>` (optional)

`runtime = "postgres"`

- (none in v1; host-owned defaults)

#### `write-file`

- Inputs: `path: path`, `contents: string`
- Capabilities: `write_workspace` (recommended denied by default)
- Validation: resolve relative paths against workspace root; reject path escapes.

#### `render-template`

- Inputs: `template: string` (structured inputs TBD)
- Outputs: `rendered: string`

### 3.10 WIT Sketch (Canonical)

This is a sketch, not finalized WIT.

```wit
package locald:plugins@0.1.0;

interface types {
  record path { value: string }
  record url { value: string }
  record datetime { value: string }

  variant value {
    null,
    bool(bool),
    string(string),
    s64(s64),
    u64(u64),
    f64(f64),
    bytes(list<u8>),
    list(list<value>),
    record(list<tuple<string, value>>),
    path(path),
    url(url),
    datetime(datetime),
  }

  record output-ref {
    step_id: string,
    path: list<selector>,
  }

  variant selector { field(string), index(u32) }

  variant expr { lit(value), get(output-ref) }

  record diagnostics {
    warnings: list<string>,
    errors: list<string>,
  }

  record host-capabilities {
    supported_ir_versions: list<u32>,
    granted: list<string>,
  }

  record workspace-context {
    workspace_id: string,
    root: string,
  }

  record service-spec {
    name: string,
    kind: string,
    depends_on: list<string>,
    config: list<tuple<string, value>>,
  }

  record plan {
    ir_version: u32,
    requested_capabilities: list<string>,
    steps: list<step>,
  }

  record step {
    id: string,
    needs: list<string>,
    op: op,
  }

  variant op {
    declare-service(declare-service-op),
    allocate-port(allocate-port-op),
    oci-pull(oci-pull-op),
    render-template(render-template-op),
    write-file(write-file-op),
  }

  record declare-service-op {
    name: string,
    runtime: string, // "process" | "container" | ... (host-defined)
    settings: list<tuple<string, expr>>,
  }

  record allocate-port-op { name: string }
  record oci-pull-op { image: string }
  record render-template-op { template: string }
  record write-file-op { path: path, contents: string }
}

interface plugin {
  use types.{workspace-context, host-capabilities, service-spec, plan, diagnostics};

  detect: func(ctx: workspace-context, spec: service-spec) -> option<string>;
  apply: func(ctx: workspace-context, caps: host-capabilities, spec: service-spec) -> result<plan, diagnostics>;
}

world locald-plugin { export plugin; }
```

### 3.10.1 WIT Constraints (Implementation Notes)

During initial host implementation we learned a few WIT/Component Model constraints that are important for plugin authors and for keeping the contract stable:

- **Identifiers**: WIT identifiers do not allow underscores. Use kebab-case in WIT (e.g. `workspace-id`, `requested-capabilities`); bindings will map these to idiomatic Rust field names.
- **Keywords**: some tokens that look natural in IRs (e.g. `string`, `bool`) are not valid as variant case names. Use non-keyword names such as `text`/`boolean`.
- **Recursion**: WIT rejects self-referential type definitions. A recursively-structured `value` (JSON-like `list<value>` / `record<value>`) is not representable directly.

Phase 29 guidance:

- Treat `types.value` as **non-recursive** in the initial ABI. If a plugin needs richer structure, it should encode it explicitly (e.g. as `bytes` or `text`) and rely on host ops (`render-template`, `write-file`, etc.) rather than smuggling a second language into the IR.

### 3.11 Example: “redis” plugin

Phase 29 dogfood uses `runtime = "container"`. The long-term Redis strategy is expected to be a host-managed embedded Redis runtime.

### 3.12 Dogfooding Targets (Phase 29)

- Redis (OCI/Container)
  - Proves plugin mechanism end-to-end.
  - Long-term: switch to embedded Redis runtime.
- Postgres (Host-Managed Runtime)
  - Proves plugins can target host-managed services.
- Site / Web service
  - Exercises ports/env/health checks and end-to-end UI/status/logs.

### 3.13 Package Format (`.locald-package`)

This section defines the distributable package format for plugins (Phase 29.2).

#### 3.13.1 Archive Structure

A `.locald-package` file is a **gzip-compressed tar archive** with the following internal structure:

```
redis-plugin-1.0.0.locald-package
├── manifest.toml          # Package metadata (required)
├── plugin.wasm            # WASM component (required)
└── assets/                # Optional bundled assets
    ├── templates/
    └── config/
```

**Rationale**: tar.gz is universally supported, aligns with the Kano-QFD recommendation, and enables single-file distribution.

#### 3.13.2 Manifest Schema (`manifest.toml`)

```toml
[package]
name = "redis-plugin"                    # Required: lowercase alphanumeric + hyphens
version = "1.0.0"                        # Required: semver
description = "Redis service support"   # Optional: human-readable description
license = "MIT"                          # Optional: SPDX license identifier
repository = "https://github.com/..."   # Optional: source repository URL
authors = ["Name <email>"]              # Optional: list of authors

[plugin]
component = "plugin.wasm"               # Required: path to WASM component within archive
service_kinds = ["redis"]               # Required: service kinds this plugin handles

[compatibility]
locald_min = "0.2.0"                    # Optional: minimum locald version
ir_version = 1                          # Required: IR version the plugin produces

[capabilities]
required = ["oci_pull"]                 # Capabilities plugin requires to function
optional = ["cache_dir"]                # Capabilities plugin can use if granted
```

##### Field Semantics

- **`package.name`**: Unique identifier. Must match regex `^[a-z][a-z0-9-]*$`. Maximum 64 characters.
- **`package.version`**: Semantic versioning (MAJOR.MINOR.PATCH). Breaking changes require MAJOR bump.
- **`plugin.service_kinds`**: List of service kinds this plugin can handle during `detect`. Used for matching and documentation.
- **`compatibility.locald_min`**: If present, `locald package install` will reject if current version is lower.
- **`compatibility.ir_version`**: Must be in host's `supported_ir_versions`. Installation fails if incompatible.
- **`capabilities.required`**: Plugin will not function without these. Install warns; runtime rejects if not grantable.
- **`capabilities.optional`**: Plugin works without these but may have reduced functionality.

#### 3.13.3 Versioning and Compatibility

**Package versioning** follows semver independently of IR version:

| Package Change              | Version Bump        |
| --------------------------- | ------------------- |
| New service kind supported  | MINOR               |
| Bug fix in plan generation  | PATCH               |
| Breaking change to behavior | MAJOR               |
| IR version bump required    | MAJOR (if breaking) |

**Compatibility checking** occurs at install time:

```
$ locald plugin install redis-plugin-1.0.0.locald-package
→ Checking compatibility...
   ✓ locald version 0.2.1 meets minimum 0.2.0
   ✓ IR version 1 supported
   ⚠ Requires capability: oci_pull (will be requested at runtime)
→ Installing to .locald/plugins/redis-plugin.wasm
→ Done.
```

#### 3.13.4 Installation and Discovery

**Installation targets**:

| Scope   | Flag                  | Target Directory                 |
| ------- | --------------------- | -------------------------------- |
| Project | `--project` (default) | `.locald/plugins/`               |
| User    | `--user`              | `$XDG_DATA_HOME/locald/plugins/` |

**Installation process**:

1. Extract archive to temp directory
2. Parse and validate `manifest.toml`
3. Check compatibility (locald version, IR version)
4. Warn about required capabilities
5. Copy `plugin.wasm` to target directory (renamed to `{package.name}.wasm`)
6. Copy `assets/` to `{target}/assets/{package.name}/` if present
7. Clean up temp directory

**Discovery** remains unchanged: host scans `.locald/plugins/*.wasm` and `$XDG_DATA_HOME/locald/plugins/*.wasm`.

#### 3.13.5 Security Considerations

**Phase 29.2 scope**: Packages are **unsigned**. Installation implies trust.

```
$ locald plugin install https://example.com/plugin.locald-package
⚠ Warning: Installing unsigned package from remote URL.
  Packages can request capabilities that affect your system.
  Only install packages from sources you trust.
Continue? [y/N]
```

**Future considerations** (not in Phase 29.2):

- Optional GPG detached signatures (`manifest.toml.sig`)
- Sigstore integration for keyless signing
- Package registry with verified publishers

#### 3.13.6 Package Creation

`locald package create` bundles a plugin into a distributable package.

##### CLI Interface

```
locald package create [SOURCE] [OPTIONS]

Arguments:
  [SOURCE]  Source directory containing manifest.toml [default: .]

Options:
  -o, --output <FILE>     Output package path [default: {name}-{version}.locald-package]
  -m, --manifest <FILE>   Manifest file path relative to SOURCE [default: manifest.toml]
      --dry-run           Show what would be packaged without creating archive
      --force             Overwrite existing output file
  -v, --verbose           Show detailed packaging steps
```

##### Validation Pipeline

Package creation performs validation in four phases:

**Phase 1 - Manifest Validation**:

1. Read and parse `manifest.toml` using `PackageManifest::from_toml()`
2. Validate schema (name regex, semver format, IR version)
3. Verify `plugin.component` file exists at expected path

**Phase 2 - WASM Verification**:

1. Read component file bytes
2. Verify WASM magic bytes (`\0asm` = `[0x00, 0x61, 0x73, 0x6D]`)
3. Optionally verify WASM component structure (deferred to runtime for MVP)

**Phase 3 - Asset Collection**:

1. If `assets/` directory exists, recursively collect all files
2. Validate no path traversal escapes (no `..` segments)
3. Preserve directory structure within `assets/`

**Phase 4 - Archive Creation**:

1. Create tar archive with entries: `manifest.toml`, component file, `assets/**`
2. Gzip compress the archive
3. Write atomically (temp file + rename) to output path

##### Error Handling

| Scenario              | Error Message                                                          |
| --------------------- | ---------------------------------------------------------------------- |
| Source path not found | `Error: Source directory '{path}' not found`                           |
| Manifest not found    | `Error: manifest.toml not found in '{path}'`                           |
| Manifest parse error  | `Error: Invalid manifest: {parse_error}`                               |
| Component not found   | `Error: Plugin component '{name}' specified in manifest not found`     |
| Invalid WASM format   | `Error: '{name}' is not a valid WASM file (invalid magic bytes)`       |
| Output exists         | `Error: Output file already exists: {path} (use --force to overwrite)` |
| Write permission      | `Error: Cannot write to output path: {path}`                           |

##### Output Format

**Success output**:

```
Creating package from ./my-plugin

✓ Validated manifest (redis-plugin v1.0.0)
✓ Verified WASM component (plugin.wasm, 234 KB)
✓ Collected 5 asset files (12 KB)
✓ Created archive (compressed: 189 KB)

→ Package created: redis-plugin-1.0.0.locald-package

  Install with:
    locald plugin install redis-plugin-1.0.0.locald-package
```

**Dry-run output**:

```
Would create package from ./my-plugin

  Manifest: redis-plugin v1.0.0
  Component: plugin.wasm (234 KB)
  Assets: 5 files (12 KB)

  Would write: redis-plugin-1.0.0.locald-package
```

##### Default Conventions

- **Output filename**: `{package.name}-{package.version}.locald-package`
- **Output directory**: Current working directory
- **Manifest location**: `{SOURCE}/manifest.toml`
- **Component location**: `{SOURCE}/{plugin.component}`
- **Asset location**: `{SOURCE}/assets/` (if present)

#### 3.13.7 Package Installation

`locald plugin install` handles both raw `.wasm` files and `.locald-package` archives via auto-detection.

##### CLI Interface

```
locald plugin install <SOURCE> [OPTIONS]

Arguments:
  <SOURCE>  Local path, file:// URL, or http(s):// URL to .wasm or .locald-package

Options:
  --name <NAME>     Installed name (only for raw .wasm; packages use manifest name)
  --project         Install to project scope (.locald/plugins/) [default]
  --user            Install to user scope ($XDG_DATA_HOME/locald/plugins/)
  --force           Overwrite existing plugin with same name
```

##### Format Detection

The install command auto-detects the format:

1. If source ends with `.locald-package`, treat as package archive
2. If source is a gzip file (magic bytes `1f 8b`), attempt to parse as package
3. Otherwise, treat as raw `.wasm` file

##### Package Installation Pipeline

**Phase 1 - Extraction**:

1. Create temp directory in target location
2. Extract gzip-compressed tar archive
3. Validate archive structure (must contain `manifest.toml`)

**Phase 2 - Manifest Validation**:

1. Parse `manifest.toml` using `PackageManifest::parse()`
2. Validate all fields per section 3.13.2

**Phase 3 - Compatibility Checking**:

1. If `compatibility.locald_min` is set, compare against current locald version
2. If `compatibility.ir_version` is set, verify it's in host's supported versions (currently `[1]`)
3. Fail installation if incompatible

**Phase 4 - Capability Warning**:

1. List `capabilities.required` with warning symbol
2. List `capabilities.optional` as informational

**Phase 5 - Installation**:

1. Copy component to `{target}/{package.name}.wasm`
2. If `assets/` exists, copy to `{target}/assets/{package.name}/`
3. Remove temp directory

##### Error Handling

| Scenario         | Error Message                                                         |
| ---------------- | --------------------------------------------------------------------- |
| Source not found | `Error: Package not found: '{path}'`                                  |
| Invalid archive  | `Error: '{path}' is not a valid .locald-package archive`              |
| Missing manifest | `Error: Package missing manifest.toml`                                |
| Invalid manifest | `Error: Invalid manifest: {parse_error}`                              |
| Version mismatch | `Error: Package requires locald >= {min}, current is {current}`       |
| IR incompatible  | `Error: Package uses IR version {v}, supported: {list}`               |
| Already exists   | `Error: Plugin '{name}' already installed (use --force to overwrite)` |

##### Output Format

**Success output**:

```
→ Extracting package...
→ Checking compatibility...
   ✓ locald version 0.2.1 meets minimum 0.2.0
   ✓ IR version 1 supported
   ⚠ Requires capability: oci_pull (will be requested at runtime)
→ Installing to .locald/plugins/redis-plugin.wasm
→ Done.
```

**Security warning (remote URLs)**:

```
⚠ Warning: Installing unsigned package from remote URL.
  Packages can request capabilities that affect your system.
  Only install packages from sources you trust.
Continue? [y/N]
```

##### Idempotent Installation

If a package with the same name and version is already installed, the command succeeds with a message:

```
✓ redis-plugin v1.0.0 already installed
```

### 3.14 Distribution Format (`.locald-distribution`)

This section defines the distribution format for bundling plugins with project configuration (Phase 29.3). Distributions enable "clone → locald up" workflows for teams by packaging both plugins and opinionated project setup.

#### 3.14.1 Concept: Packages vs Distributions

| Concept          | Purpose                | Contains                    | Installed To          |
| ---------------- | ---------------------- | --------------------------- | --------------------- |
| **Package**      | Single plugin delivery | 1 WASM + assets             | Plugin directory      |
| **Distribution** | Project bootstrap kit  | N packages + project config | New project directory |

A **distribution** answers: "How do I set up a new project that uses your plugin?" It bundles:

- One or more `.locald-package` files (or remote refs)
- A starter `locald.toml` with service definitions
- Optional project scaffolding (templates, example code)

#### 3.14.2 Archive Structure

A `.locald-distribution` file is a **gzip-compressed tar archive** with the following internal structure:

```
redis-stack-1.0.0.locald-distribution
├── distribution.toml         # Distribution metadata (required)
├── locald.toml               # Project config template (required)
├── packages/                 # Bundled plugin packages (optional)
│   ├── redis-plugin-1.0.0.locald-package
│   └── postgres-plugin-2.0.0.locald-package
└── scaffold/                 # Project scaffolding (optional)
    ├── .gitignore
    ├── README.md.template
    └── docker-compose.yml
```

**Rationale**: Distributions extend the package archive format, adding project-level configuration and optional bundled packages.

#### 3.14.3 Distribution Manifest (`distribution.toml`)

```toml
[distribution]
name = "redis-stack"                        # Required: lowercase alphanumeric + hyphens
version = "1.0.0"                           # Required: semver
description = "Redis + Postgres dev stack"  # Optional: human-readable description
license = "MIT"                             # Optional: SPDX license identifier
repository = "https://github.com/..."       # Optional: source repository URL
authors = ["Name <email>"]                  # Optional: list of authors

[compatibility]
locald_min = "0.2.0"                        # Optional: minimum locald version

[plugins]
# Bundled plugins (files in packages/ directory)
bundled = [
    "redis-plugin-1.0.0.locald-package",
    "postgres-plugin-2.0.0.locald-package"
]

# Remote plugins (fetched during init)
remote = [
    "https://plugins.locald.dev/caddy-plugin-1.0.0.locald-package",
    { url = "https://internal.company.com/custom-plugin.locald-package", sha256 = "abc123..." }
]

[scaffold]
# Template files to render (support variable substitution)
templates = ["README.md.template"]

# Files to copy as-is
files = [".gitignore", "docker-compose.yml"]

# Variables available in templates
[scaffold.variables]
project_name = { prompt = "Project name", default = "my-project" }
database_name = { prompt = "Database name", default = "app_development" }
```

##### Field Semantics

- **`distribution.name`**: Unique identifier. Must match regex `^[a-z][a-z0-9-]*$`. Maximum 64 characters.
- **`distribution.version`**: Semantic versioning (MAJOR.MINOR.PATCH).
- **`compatibility.locald_min`**: If present, `locald init` rejects if current version is lower.
- **`plugins.bundled`**: Relative paths to `.locald-package` files within `packages/` directory.
- **`plugins.remote`**: URLs to fetch during init. Supports string URLs or objects with `url` and optional `sha256` checksum.
- **`scaffold.templates`**: Files in `scaffold/` to process with variable substitution. Output removes `.template` suffix.
- **`scaffold.files`**: Files in `scaffold/` to copy without modification.
- **`scaffold.variables`**: User-prompted variables for template rendering.

#### 3.14.4 Project Config Template (`locald.toml`)

The `locald.toml` included in a distribution serves as the starter project configuration:

```toml
# Example: locald.toml in redis-stack distribution

[project]
name = "{{project_name}}"    # Variable substituted during init

[[services]]
name = "redis"
kind = "redis"

[[services]]
name = "db"
kind = "postgres"
config.database = "{{database_name}}"
```

**Template syntax**: Variables use `{{variable_name}}` syntax, matching keys from `scaffold.variables`.

#### 3.14.5 The `locald init --from-distribution` Command

##### CLI Interface

```
locald init --from-distribution <SOURCE> [OPTIONS]

Arguments:
  <SOURCE>  Local path or URL to .locald-distribution

Options:
  --name <NAME>           Project name (overrides prompt/default)
  --target <DIR>          Target directory [default: ./<project_name>]
  --no-scaffold           Skip scaffold files (only install plugins + locald.toml)
  --offline               Use only bundled plugins, skip remote fetches
  --yes                   Accept all defaults without prompting
  -v, --verbose           Show detailed initialization steps
```

##### Initialization Pipeline

**Phase 1 - Distribution Extraction**:

1. Download distribution (if URL) to temp directory
2. Extract gzip-compressed tar archive
3. Parse and validate `distribution.toml`

**Phase 2 - Compatibility Check**:

1. Verify locald version meets `compatibility.locald_min`
2. Fail with actionable message if incompatible

**Phase 3 - Variable Collection**:

1. For each variable in `scaffold.variables`:
   - If `--yes` flag, use default value
   - If variable provided via CLI (e.g., `--name`), use that
   - Otherwise, prompt user with configured prompt text
2. Validate all required variables are set

**Phase 4 - Target Directory Setup**:

1. Create target directory (default: `./<project_name>`)
2. Fail if directory exists and is non-empty (unless `--force`)
3. Initialize as locald project

**Phase 5 - Plugin Installation**:

1. For each plugin in `plugins.bundled`:
   - Extract from `packages/` and install to `.locald/plugins/`
2. For each plugin in `plugins.remote` (unless `--offline`):
   - Fetch from URL
   - Verify checksum if provided
   - Install to `.locald/plugins/`

**Phase 6 - Config Generation**:

1. Process `locald.toml` with collected variables
2. Write to `{target}/locald.toml`

**Phase 7 - Scaffold Generation**:

1. For each file in `scaffold.templates`:
   - Process with variable substitution
   - Write to target (without `.template` suffix)
2. For each file in `scaffold.files`:
   - Copy to target preserving relative path

##### Error Handling

| Scenario               | Error Message                                                               |
| ---------------------- | --------------------------------------------------------------------------- |
| Source not found       | `Error: Distribution not found: '{path}'`                                   |
| Invalid archive        | `Error: '{path}' is not a valid .locald-distribution archive`               |
| Missing manifest       | `Error: Distribution missing distribution.toml`                             |
| Version mismatch       | `Error: Distribution requires locald >= {min}, current is {current}`        |
| Directory exists       | `Error: Target directory '{dir}' already exists (use --force to overwrite)` |
| Remote fetch failed    | `Error: Failed to fetch remote plugin: {url} ({reason})`                    |
| Checksum mismatch      | `Error: Checksum mismatch for {url}: expected {expected}, got {actual}`     |
| Missing bundled plugin | `Error: Bundled plugin not found: {name}`                                   |

##### Output Format

**Interactive mode**:

```
Initializing from redis-stack v1.0.0

? Project name: my-redis-app
? Database name: [app_development]

→ Creating project directory: ./my-redis-app
→ Installing plugins...
   ✓ redis-plugin v1.0.0
   ✓ postgres-plugin v2.0.0
   ⚡ Fetching caddy-plugin v1.0.0...
   ✓ caddy-plugin v1.0.0
→ Generating configuration...
   ✓ locald.toml
   ✓ README.md
   ✓ .gitignore
→ Done!

  Next steps:
    cd my-redis-app
    locald up
```

**Non-interactive mode** (`--yes`):

```
→ Initializing from redis-stack v1.0.0 with defaults
→ Creating project directory: ./my-project
→ Installing 3 plugins...
→ Generating 3 scaffold files...
→ Done!
```

#### 3.14.6 Remote Package References in `locald.toml`

Projects can reference remote plugins directly in `locald.toml`, enabling dependency resolution without pre-installation.

##### Schema Extension

```toml
[project]
name = "my-app"

# Plugin sources (processed before service resolution)
[plugins]
# Simple URL reference
redis = "https://plugins.locald.dev/redis-plugin-1.0.0.locald-package"

# URL with checksum verification
postgres = { url = "https://plugins.locald.dev/postgres-plugin-2.0.0.locald-package", sha256 = "abc123..." }

# Local path reference (useful for development)
custom = { path = "../my-custom-plugin/target/plugin.wasm" }

# Installed plugin (explicit, optional—installed plugins are discovered automatically)
# sidekiq = { installed = "sidekiq-plugin" }

[[services]]
name = "redis"
kind = "redis"

[[services]]
name = "db"
kind = "postgres"
```

##### Resolution Order

When `locald up` starts, plugin resolution follows this order:

1. **Installed plugins**: Scan `.locald/plugins/` and user plugin directory
2. **Path references**: Load WASM from specified local path
3. **Remote references**: Fetch, verify, and cache remote packages

##### Remote Plugin Caching

Remote plugins are cached to avoid repeated downloads:

- **Cache location**: `$XDG_CACHE_HOME/locald/plugins/` (typically `~/.cache/locald/plugins/`)
- **Cache key**: URL + SHA256 (if provided) or URL hash + mtime from HTTP headers
- **Cache invalidation**: `locald plugin cache clear` or automatic on checksum mismatch

##### Security Model

Remote plugin references inherit the same security model as `locald plugin install`:

```
$ locald up
→ Fetching remote plugin: https://plugins.locald.dev/redis-plugin-1.0.0.locald-package
⚠ Warning: First-time fetch of unsigned remote plugin.
  Plugins can request capabilities that affect your system.
  Only use plugins from sources you trust.

  Add sha256 checksum to suppress this warning:
    postgres = { url = "...", sha256 = "abc123..." }

Continue? [y/N]
```

**Trust once, verify always**: After first approval, the plugin URL+checksum is recorded in `.locald/trust.toml`. Future fetches of the same URL with matching checksum proceed without prompting.

##### Offline Mode

`locald up --offline` skips remote plugin fetches and uses only cached or installed plugins:

```
$ locald up --offline
→ Using cached plugins only
   ✓ redis-plugin (cached)
   ✗ postgres-plugin (not cached, skipping)
⚠ Warning: 1 remote plugin unavailable in offline mode
```

#### 3.14.7 Distribution Creation

`locald distribution create` bundles plugins and configuration into a distributable archive.

##### CLI Interface

```
locald distribution create [SOURCE] [OPTIONS]

Arguments:
  [SOURCE]  Source directory containing distribution.toml [default: .]

Options:
  -o, --output <FILE>     Output path [default: {name}-{version}.locald-distribution]
  -m, --manifest <FILE>   Manifest path relative to SOURCE [default: distribution.toml]
      --include-remote    Fetch and bundle remote plugins instead of keeping as references
      --dry-run           Show what would be packaged without creating archive
      --force             Overwrite existing output file
  -v, --verbose           Show detailed packaging steps
```

##### Validation Pipeline

**Phase 1 - Manifest Validation**:

1. Parse and validate `distribution.toml`
2. Verify `locald.toml` exists

**Phase 2 - Plugin Resolution**:

1. For each bundled plugin: verify file exists in `packages/`
2. For each remote plugin: validate URL format
3. If `--include-remote`: fetch and add to `packages/`

**Phase 3 - Scaffold Validation**:

1. Verify all template files exist in `scaffold/`
2. Verify all static files exist in `scaffold/`
3. Validate template syntax (balanced `{{` and `}}`)

**Phase 4 - Archive Creation**:

1. Create tar archive with: `distribution.toml`, `locald.toml`, `packages/**`, `scaffold/**`
2. Gzip compress
3. Write atomically to output path

##### Output Format

```
Creating distribution from ./redis-stack

✓ Validated manifest (redis-stack v1.0.0)
✓ Found 2 bundled plugins (1.2 MB)
✓ Validated 2 remote plugin URLs
✓ Processed locald.toml (2 services)
✓ Collected 3 scaffold files
✓ Created archive (compressed: 1.1 MB)

→ Distribution created: redis-stack-1.0.0.locald-distribution

  Initialize with:
    locald init --from-distribution redis-stack-1.0.0.locald-distribution
```

## 4. Implementation Plan (Stage 2)

### Phase 29.1 (Plugin Mechanism) ✅

- [x] Define initial WIT package + generate bindings.
- [x] Implement plugin discovery/loading.
- [x] Implement plan validation + deterministic topo sort.
- [x] Implement minimal op set.
- [x] Add `locald plugin inspect/validate`.
- [x] Create example `redis` plugin.
- [x] Add conformance fixture suite for host.

### Phase 29.2 (Packaging) ✅

- [x] Define `manifest.toml` schema and parser.
- [x] Implement `locald package create` command.
- [x] Implement `locald package install` command.
- [x] Add compatibility checking (locald version, IR version).
- [x] Add capability warning at install time.
- [x] Update redis-plugin example with manifest.
- [x] Document packaging workflow for plugin authors.

### Phase 29.3 (Distributions)

- [ ] Define `distribution.toml` schema and parser (3.14.3).
- [ ] Implement distribution archive creation with `locald distribution create` (3.14.7).
- [ ] Implement `locald init --from-distribution` command (3.14.5).
- [ ] Add template variable substitution for `locald.toml` and scaffold files.
- [ ] Extend `locald.toml` schema for `[plugins]` section with remote refs (3.14.6).
- [ ] Implement remote plugin fetching and caching during `locald up`.
- [ ] Add checksum verification and trust model (`.locald/trust.toml`).
- [ ] Create example `redis-stack` distribution for dogfooding.
- [ ] Document distribution authoring workflow.

## 5. Context Updates (Stage 3)

- [ ] Add `docs/manual/` documentation for plugin architecture.

## 6. Drawbacks

- New contract surface (WIT + IR) must be versioned and tested.
- Op set growth pressure; must stay intentionally small.

## 7. Alternatives

- TOML-only presets
- Embedded scripting (Lua/Rhai/JS)
- Native dynamic libraries
- Full WASM `ServiceController`

## 8. Unresolved Questions

- Whether `detect` is mandatory or optional (proposal: optional but strongly recommended).
- Exact structured inputs for `render-template`.

## 9. Future Possibilities

- Rust SDK for plugin authors.
- Multi-plugin detect/apply composition if proven valuable.
- Published conformance suite in CI.
