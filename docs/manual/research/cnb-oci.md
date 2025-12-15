# Research: OCI Image Extraction for CNB

## Goal

We want to implement `locald build` using Cloud Native Buildpacks. To do this without requiring a running Docker daemon for the _build_ process (or to be more lightweight), we need to:

1.  Download a CNB Builder image (e.g., `heroku/builder:22`).
2.  Extract the `/cnb` directory from the image layers.
3.  Run the CNB `lifecycle` binaries (which we can download separately or extract from the builder if they are there).

## Current State

- `examples/cnb-test/extract-builder.sh` uses `docker pull` and `docker cp`.
- `examples/cnb-test/run-creator.sh` runs the lifecycle binaries.

## Research Questions

1.  Can we use a Rust crate to pull an OCI image and extract specific files/directories without a Docker daemon?
2.  Which crates are suitable? (`oci-distribution`, `krata`, `containerd-shim`?)
3.  How do we handle authentication for registries?
4.  How do we handle multi-layer extraction (whiteouts, etc.)?

## Candidates

- **oci-distribution**: Seems to be the standard for interacting with registries.
- **crane**: Go tool, but maybe we can learn from it or use a Rust equivalent.
- **cap-std-ext** / **tar**: For extraction.

## Prototype Plan

1.  Create a Rust binary in `examples/cnb-test/rust-extractor`.
2.  Use `oci-distribution` to pull the manifest of `heroku/builder:22`.
3.  Download the layers.
4.  Walk through layers to find `/cnb` content.
5.  Extract to disk.

## Findings (2025-12-05)

- Successfully implemented a prototype in `examples/cnb-test/rust-extractor`.
- Used `oci-distribution` to pull the manifest and layers of `heroku/builder:22`.
- Found `/cnb` content distributed across multiple layers:
  - Layer 4: `/cnb/buildpacks`, `/cnb/extensions`
  - Layer 5: `/cnb/lifecycle` (Contains `creator`, `detector`, etc.)
  - Layers 6-19: Individual buildpacks (e.g., `heroku_nodejs`, `heroku_ruby`).
  - Layers 20-22: Configuration files (`order.toml`, `stack.toml`, `run.toml`).
- The paths in the tar archives are absolute (e.g., `/cnb/lifecycle/creator`).
- We can selectively extract files starting with `/cnb` from all layers to reconstruct the builder environment.

## Next Steps

1.  Implement `locald-builder` crate to encapsulate this logic.
2.  Implement `locald build` command to use this crate.
3.  The `locald build` command should:
    - Check if the builder image is cached.
    - If not, pull and extract `/cnb` to a cache directory (e.g., `~/.local/share/locald/builders/heroku-builder-22`).
    - Run the `lifecycle/creator` binary from the extracted directory against the user's app.
