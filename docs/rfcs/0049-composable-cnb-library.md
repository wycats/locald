---
title: "Composable CNB Library (locald-pack)"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Build System
---

# RFC 0049: Composable CNB Library (`locald-pack`)

## 1. Summary

This RFC proposes extracting the core CNB logic from `locald` into a standalone, reusable library (and potentially a CLI tool) called `locald-pack` (or just `pack-rs`).

Unlike the official `pack` CLI, which is a monolithic "all-or-nothing" tool, this library will be **Composable**. It will expose the individual phases of the CNB lifecycle (Analyze, Detect, Restore, Build, Export) as distinct, programmable APIs, allowing users to orchestrate their own build pipelines without reimplementing the low-level OCI/Lifecycle plumbing.

## 2. Motivation

The official `pack` tool is excellent for standard use cases, but it is rigid. It assumes a specific workflow and is difficult to embed or customize.

Developers often want:
1.  **Granular Control**: "I want to run `detect` and `build`, but I want to handle the `export` phase myself (e.g., to a custom tarball instead of a registry)."
2.  **Embedded Usage**: "I want to build an image inside my Rust application without spawning a `pack` subprocess."
3.  **Alternative Runtimes**: "I want to run the lifecycle on a remote cluster, or inside a specific VM, not just local Docker."

`locald` already needs this flexibility (to support "Native" vs "Containerized" vs "Lima" runtimes). By formalizing this as a library, we solve our own problem and provide a valuable tool to the ecosystem.

## 3. Detailed Design

### 3.1 The "Kit" Philosophy

Instead of a "Framework" that calls your code, we provide a "Kit" of tools that you call.

```rust
// Conceptual Usage
let platform = Platform::new(app_dir, layers_dir);
let lifecycle = Lifecycle::new(builder_image);

// Phase 1: Analyze (Optional)
if let Some(prev_image) = registry.get_image("my-app:latest").await? {
    lifecycle.analyze(&platform, prev_image).await?;
}

// Phase 2: Detect
let group = lifecycle.detect(&platform).await?;
println!("Detected buildpacks: {:?}", group);

// Phase 3: Build (with custom progress handler)
lifecycle.build(&platform, group, |msg| {
    println!("Build output: {}", msg);
}).await?;

// Phase 4: Export (Custom)
// Instead of the standard exporter, maybe we just tar up the layers
my_custom_exporter::export(&platform.layers_dir, "output.tar").await?;
```

### 3.2 Core Components

1.  **`BuilderImage`**: Handles pulling, verifying, and extracting the CNB builder image (Lifecycle binaries + Buildpacks).
2.  **`Platform`**: Manages the directory structure (`/layers`, `/platform`, `/workspace`) and environment variables.
3.  **`Lifecycle`**: Wraps the execution of the lifecycle binaries (`detector`, `builder`, etc.).
    *   Crucially, this component is **Runtime-Agnostic**. It accepts a `Runtime` trait (from RFC 0047) so it can execute the binaries via `Command` (Native), `libcontainer` (Isolated), or `ssh` (Remote).
4.  **`Registry`**: A clean abstraction for OCI registry interactions (checking for run-images, pushing layers).

### 3.3 The "Default Path"

While we expose the low-level pieces, we also provide a high-level `Builder` struct that wires them together for the 90% use case (standard `pack build` behavior).

```rust
// The "Easy Mode"
locald_pack::Builder::new("heroku/builder:22")
    .build("my-app", ".")
    .await?;
```

## 4. Implementation Plan

### Phase 1: Internal Module (Current)
Implement this logic inside `locald-core::buildpack` to serve `locald`'s immediate needs. Focus on the `Runtime` abstraction.

### Phase 2: Extraction (Future)
Once the API stabilizes, extract `locald-core::buildpack` into a separate crate (`locald-pack`?).

### Phase 3: CLI Wrapper (Optional)
Build a lightweight CLI (`locald-pack`) that exposes these primitives to shell scripts, offering a more scriptable alternative to `pack`.

## 5. Alternatives Considered

*   **Using `pack` CLI**: Too heavy, requires Docker, hard to control output/progress.
*   **Using `libcnb` (Rust)**: This is for *writing* buildpacks, not *running* them.
*   **Reimplementing Lifecycle**: Too risky. We should orchestrate the official lifecycle binaries, not rewrite them.
