---
title: WASM Plugins as Plan Transforms
stage: 1
feature: Extensibility
exo:
    tool: exo rfc create
    protocol: 1
---


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
      "op": {"allocate_port": {"name": "redis"}}
    },
    {
      "id": "pull",
      "needs": [],
      "op": {"oci_pull": {"image": "redis:7"}}
    },
    {
      "id": "service",
      "needs": ["port", "pull"],
      "op": {
        "declare_service": {
          "name": "redis",
          "runtime": "container",
          "settings": [
            ["image", {"lit": {"string": "redis:7"}}],
            ["port", {"get": {"step_id": "port", "path": [{"field": "port"}]}}]
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

## 4. Implementation Plan (Stage 2)

- [ ] Define initial WIT package + generate bindings.
- [ ] Implement plugin discovery/loading.
- [ ] Implement plan validation + deterministic topo sort.
- [ ] Implement minimal op set.
- [ ] Add `locald plugin inspect/validate`.
- [ ] Create example `redis` plugin.
- [ ] Add conformance fixture suite for host.

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
