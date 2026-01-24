---
title: "Strawman: Standalone OCI Fetcher Crate"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Architecture
---

# RFC 0046: Strawman: Standalone OCI Fetcher Crate

## 1. Summary

This RFC proposes extracting the OCI image fetching and extraction logic, currently embedded in `locald-builder`, into a standalone, reusable Rust crate.

This crate would provide a high-level API to:

1.  Connect to an OCI registry (with authentication).
2.  Fetch an image manifest.
3.  Download layers.
4.  Extract layers (tar/gzip) to a local directory or export them to an OCI Layout structure.

## 2. Motivation

During the implementation of the CNB integration (RFC 0043), we discovered that the combination of `oci-distribution`, `tar`, and `flate2` provides a powerful primitive: the ability to consume OCI artifacts without a full container runtime (like Docker or Podman).

This primitive has uses beyond just CNB builders:

- **WASM Modules**: Fetching WASM binaries distributed as OCI artifacts.
- **CLI Tools**: Distributing CLI tools or assets via registries.
- **Plugin Systems**: Fetching plugins.
- **System Roots**: Fetching root filesystems for lightweight VMs or sandboxes.

Currently, this logic is tightly coupled to the `locald-builder` crate. Extracting it would:

1.  **Clean up `locald-builder`**: Allow it to focus purely on CNB lifecycle orchestration.
2.  **Enable Reuse**: Other parts of `locald` (or other projects) could use it.
3.  **Isolate Complexity**: OCI authentication and layer handling is complex; isolating it makes it easier to test and maintain.

## 3. Design Ideas

### 3.1 Potential Crate Names

- `oci-fetch`
- `oci-unpack`
- `container-image-fetcher`
- `oci-artifact-client`

### 3.2 API Sketch

```rust
use oci_fetcher::{Client, Reference, Destination};

async fn example() -> Result<()> {
    let client = Client::new(auth_config);
    let image_ref: Reference = "heroku/builder:22".parse()?;

    // Fetch and extract to a directory
    client.pull(
        &image_ref,
        Destination::Directory("/tmp/my-image")
    ).await?;

    // Fetch and export to OCI Layout (for tools that consume it)
    client.pull(
        &image_ref,
        Destination::OciLayout("/tmp/my-layout")
    ).await?;

    Ok(())
}
```

### 3.3 Key Features

- **Authentication**: Docker config support, credential helpers.
- **Caching**: Intelligent layer caching (don't re-download if sha256 matches).
- **Formats**: Support Docker V2 Schema 2 and OCI Image Manifests.
- **Extraction**: Safe extraction (prevent path traversal) with `tar`.

## 4. Implementation Plan (Strawman)

This is a placeholder for future work. The immediate priority is getting Rust buildpacks working in `locald`. Once that is stable, we can refactor the OCI logic into this crate.

1.  **Audit**: Review `locald-builder/src/oci_layout.rs` and `builder.rs`.
2.  **Scaffold**: Create the new crate in the workspace.
3.  **Migrate**: Move the logic, generalizing it where necessary (removing CNB-specific assumptions).
4.  **Integrate**: Update `locald-builder` to depend on the new crate.
