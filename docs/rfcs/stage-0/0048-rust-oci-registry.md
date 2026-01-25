---
title: "Rust-based OCI Registry (Strawman)"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Distribution
---

# RFC 0048: Rust-based OCI Registry (Strawman)

## 1. Summary

This is a "harebrained" proposal to implement a custom, lightweight OCI Container Registry in Rust.

Inspired by the architecture of the **Cargo Registry** (crates.io), this project aims to simplify container distribution by decoupling the **Index** (metadata) from the **Storage** (blobs), leveraging modern cloud infrastructure (S3/R2 + CDNs) directly.

## 2. Motivation

The current state of OCI registries is frustrating:

- **Complexity**: Running a private registry (like Harbor or even the reference `distribution/distribution`) is heavy.
- **Opaqueness**: The protocols often feel like black boxes.
- **Missed Opportunity**: The Cargo ecosystem proved that a static index + content-addressable storage (CAS) scales infinitely and is easy to mirror/cache.

The OCI Distribution Spec and Image Format are well-defined. There is no technical reason we cannot build a "Cargo-for-Containers" that is:

1.  **Simple**: "It's just blobs and JSON."
2.  **Fast**: Rust-based, highly concurrent.
3.  **Cheap**: Runs on S3/R2 with Cloudflare caching.

## 3. Conceptual Design

### 3.1 The Architecture

We separate the system into two distinct parts (like Cargo):

1.  **The Storage (The "Crate Files")**:

    - OCI Layers (blobs) are immutable.
    - They are stored in an S3 bucket (or compatible object storage).
    - Keyed by SHA256 digest.
    - Served via CDN (Cloudfront/Cloudflare).

2.  **The Index (The "Git Repo")**:
    - Stores the Manifests and Tags.
    - Could be a Git repository (like the old Cargo index) or a Sparse HTTP Index (like the new Cargo index).
    - Allows for fast lookups of "What is `latest`?" without scanning S3.

### 3.2 The Protocol

The registry will implement the **OCI Distribution Specification** API.

- `GET /v2/`: API Version Check.
- `GET /v2/<name>/manifests/<reference>`: Pull manifest.
- `GET /v2/<name>/blobs/<digest>`: Pull layer (redirects to S3/CDN).
- `POST /v2/<name>/blobs/uploads/`: Push layer.

### 3.3 Why Rust?

- **Performance**: High-throughput async I/O (Tokio/Axum).
- **Safety**: Memory safety for parsing untrusted manifests.
- **Ecosystem**: We can reuse `oci-spec-rs` (from the Youki project) for type definitions.

## 4. Strawman Implementation Plan

This is likely a separate project, but `locald` could be the first consumer/provider.

1.  **Prototype**: Build a simple Axum server that implements the `GET` endpoints of the OCI spec, serving files from a local disk.
2.  **S3 Integration**: Move blob storage to S3. Implement presigned URLs for uploads/downloads to offload traffic.
3.  **The Index**: Design the metadata storage format.
4.  **Push Support**: Implement the complex chunked upload protocol.

## 5. "Harebrained" Ideas

- **P2P Distribution**: Since layers are CAS blobs, could we distribute them via BitTorrent or IPFS?
- **Local Caching Proxy**: `locald` could act as a transparent pull-through cache for this registry, speeding up local builds.
- **AI-Assisted**: The spec is rigid. We can use AI agents to generate the boilerplate for the API endpoints and validation logic.
