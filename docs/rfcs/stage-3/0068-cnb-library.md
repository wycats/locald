# RFC 0068: CNB Library Extraction & OCI Layering

## Status

- **Status**: Stage 2 (Available)
- **Date**: 2025-12-10

## Summary

Refactor the container build and execution stack into a strict 3-layer architecture: **Foundation** (`locald-oci`), **Platform** (`cnb-client`), and **Integration** (`locald-builder`). This RFC supersedes the initial plan by explicitly assigning "Image Fetching" responsibilities to `locald-oci` to support both CNB builds and ad-hoc container execution (`locald container run`).

## Motivation

Currently, `locald-builder` contains a mix of generic OCI logic (pulling images), CNB-specific logic (lifecycle orchestration), and `locald`-specific runtime binding. This coupling prevents code reuse for `locald container run` (Phase 76) and makes testing difficult.

We need a clean separation of concerns:

1.  **Foundation**: Tools to fetch and run _any_ OCI image.
2.  **Platform**: Tools to run the _CNB Lifecycle_ (which happens to use OCI images).
3.  **Integration**: Wiring these tools into the `locald` daemon.

## Architecture

### 1. Foundation: `locald-oci` (The Engine)

**Scope**: Pure OCI primitives. Knows about Registries, Images, and Runtimes. Knows **nothing** about Buildpacks.

- **Responsibilities**:
  - **Fetcher**: Pulling manifests and blobs from OCI registries (moved from `locald-builder`).
  - **Store**: Managing the local OCI Layout (unpacking layers, handling whiteouts).
  - **Spec**: Generating `config.json` from `OciImageConfig`.
  - **Runtime**: Wrapping `runc` (or `locald-shim`) execution.

### 2. Platform: `cnb-client` (The Orchestrator)

**Scope**: Cloud Native Buildpacks logic. Knows about Lifecycles, Stacks, and Platforms.

- **Responsibilities**:
  - **Builder Management**: Uses `locald-oci` to fetch and unpack Builder images.
  - **Metadata**: Parses `stack.toml`, `builder.toml`, and CNB labels.
  - **Lifecycle**: Orchestrates the 5 phases (Detect, Analyze, Restore, Build, Export).
  - **Platform API**: Sets up the `/platform` directory and environment variables.
- **Dependencies**: `locald-oci` (for fetching/running).

### 3. Integration: `locald-builder` (The Glue)

**Scope**: `locald` specific wiring.

- **Responsibilities**:
  - **Config Mapping**: Maps `locald.toml` to `cnb-client` configuration.
  - **Runtime Impl**: Implements `cnb_client::ContainerRuntime` using `locald-shim`.
  - **CLI**: Exposes `locald build`.

## Implementation Plan

### Phase 1: Refactor `locald-oci`

1.  Move `locald-builder/src/image.rs` logic to `locald-oci/src/fetcher.rs`.
2.  Ensure `locald-oci` can pull an image to a local path and unpack it.

### Phase 2: Scaffold `cnb-client`

1.  Create `cnb-client` crate.
2.  Move `locald-builder/src/lifecycle.rs` logic to `cnb-client`.
3.  Define `ContainerRuntime` trait in `cnb-client` to abstract the execution.

### Phase 3: Update `locald-builder`

1.  Update `locald-builder` to depend on `cnb-client` and `locald-oci`.
2.  Implement `ContainerRuntime` using `locald-shim`.

## Drawbacks

- Refactoring `image.rs` requires careful handling of dependencies (e.g., `oci-distribution`, `flate2`).

## Rationale

This layering enables `locald container run` (Phase 76) to use `locald-oci` for fetching images without pulling in the entire CNB stack. It also allows `cnb-client` to be a standalone library for other Rust tools.
