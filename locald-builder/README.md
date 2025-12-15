# locald-builder

**Vision**: The factory that turns source code into runnable artifacts.

## Purpose

`locald-builder` is responsible for the "Build" phase of the development loop.

Today it is focused on Cloud Native Buildpacks (CNB) and OCI image/bundle plumbing used by `locald`.

## Key Components

- **Lifecycle**: Manages the CNB lifecycle (detect, build, export) to create images.
- **Image**: Handles pulling and caching of OCI images.
- **BundleSource**: A trait for preparing filesystem bundles for execution.

## Interaction

- **`locald-server`**: Calls into `locald-builder` to prepare services before execution.
- **`locald-oci`**: Used for low-level OCI operations.

## Standalone Usage

While primarily a library for the server, the logic here could be exposed as a standalone CLI tool (e.g., `locald-build`) to build images from a local directory without running them.
