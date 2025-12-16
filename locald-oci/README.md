# locald-oci

**Vision**: The standards bearer for container compliance.

## Purpose

`locald-oci` encapsulates the low-level details of the Open Container Initiative (OCI) specifications. It handles the generation of OCI Runtime Specifications (`config.json`) and the manipulation of OCI Image Layouts.

## Key Components

- **Runtime Spec**: Builders for generating OCI-compliant runtime configurations (namespaces, mounts, capabilities).
- **Runtime Wrapper**: A thin runtime interface that executes OCI bundles via `locald-shim` (the “fat shim” / `bundle run` command), handling output streaming and lifecycle plumbing.
- **OCI Layout**: Tools for reading, writing, and unpacking OCI image layouts on the filesystem.

## Interaction

- **`locald-builder`**: Uses this crate to package built artifacts into OCI layouts.
- **`locald-server`**: Uses this crate to generate runtime specs and execute containers via the shim.

## Standalone Usage

This library can be used by any Rust application that needs to work with OCI images, generate runtime specifications, or orchestrate container execution.
