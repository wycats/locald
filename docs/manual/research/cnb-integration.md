# Research: CNB Integration

## Goal

Integrate Cloud Native Buildpacks (CNB) into `locald` to provide a zero-config build system (`locald build`).

## Findings (from Agent Research)

### 1. Platform API vs Lifecycle

- **Platform API**: The interface we must implement. Responsibilities: prepare env, orchestrate phases, handle artifacts.
- **Lifecycle**: The Go binaries (`detector`, `builder`, `creator`, etc.) that do the work.
- **`creator`**: A single binary that runs all phases in order.

### 2. Running without Docker

- **Yes**, `lifecycle` binaries can run directly on the host.
- **Risk**: "Stack" compatibility. Buildpacks expect a specific OS (e.g., Ubuntu Jammy). Running on a different distro might cause dynamic linking errors (glibc).

### 3. Implementation Options

- **Option A (Wrap `pack`)**: Requires Docker. Heavy dependency.
- **Option B (Native Platform)**: Download `lifecycle` binaries and run them directly.
  - **Pros**: No Docker required. Fast.
  - **Cons**: Stack compatibility issues.

## Decision

We will pursue **Option B (Native Platform)**.
To mitigate Stack issues, we will initially target "Meta Buildpacks" or languages that are less sensitive to the OS (or where we can configure the stack to match the host).
Longer term, we might use `bubblewrap` or similar to simulate the stack if needed.

## Prototype Plan

1.  Download `lifecycle` release (v0.20.x?).
2.  Create a simple `node-js` app.
3.  Run `creator` against it.
4.  See if it produces a runnable artifact (or OCI image).

## Open Questions

- Does `creator` _require_ exporting to a Docker daemon? Or can it export to a local directory (OCI layout)?
- If it exports to OCI layout, how do we run it? `runc`? Or just extract the rootfs and run the process?
- If we just want the binary/app, maybe we don't need the `export` phase? Just `build`?
