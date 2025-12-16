---
title: "Standard Container Environment"
stage: 3 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Core Architecture
---

# RFC 0060: Standard Container Environment

## 1. Summary

This RFC defines the precise OCI runtime configuration (`config.json`) and filesystem layout that `locald` guarantees for all containerized services. It explicitly maps each configuration decision to an upstream standard (OCI, CNB) or a de-facto industry standard (Moby/Docker), ensuring our execution environment is predictable and compliant, not "hacked together."

## 2. Motivation

As we moved from "Native" execution to "Containerized" execution (RFC 0052), we made several ad-hoc decisions to get things working (e.g., hardcoded `rsync` exclusions, specific UID mappings).

To ensure long-term stability and compatibility, we must formalize these decisions. We are not inventing a new runtime environment; we are implementing a compliant OCI runtime that hosts Cloud Native Buildpacks.

## 3. Detailed Design

### 3.1. The Filesystem Layout

**Standard**: Cloud Native Buildpacks (CNB) Platform API.

- **`/workspace`**: The application source code.
  - _Source_: CNB Spec.
  - _Implementation_: We unpack the OCI image here.
- **`/layers`**: Buildpack layers (dependencies, cache).
  - _Source_: CNB Spec.
- **`/cnb`**: Lifecycle binaries and configuration.
  - _Source_: CNB Spec.

### 3.2. Process Configuration (`config.json`)

**Standard**: OCI Runtime Specification.

#### Working Directory (`cwd`)

- **Value**: `/workspace`
- **Reason**: CNB-built images place the application in `/workspace`. The entrypoint (launcher) expects to run from this context.
- **Precedent**: `pack` and `lifecycle` default to this directory.

#### User Namespaces & ID Mapping

- **Value**: Map Host UID $\to$ Container UID 0 (Root).
- **Reason**: "Rootless" execution.
  - The process inside the container needs to _believe_ it is root (UID 0) to perform operations like `apt-get` (during build) or modifying files in `/workspace` without permission errors.
  - However, for security and file ownership on the host, these operations must map back to the unprivileged Host User.
- **Precedent**: Podman Rootless, Docker `userns-remap`.

#### Mounts

- **Standard OCI Mounts**: `/proc`, `/sys`, `/dev`, `/dev/pts`, `/tmp`.
- **Reason**: These are mandated by the **OCI Runtime Specification (config-linux.md)** for any container that intends to run standard Linux applications.
  - `/proc`: Required for process introspection and `ps`.
  - `/sys`: Required for kernel parameter discovery.
  - `/dev`: Required for standard I/O streams (`stdin`, `stdout`, `stderr`) and random number generation (`/dev/urandom`).
  - `/dev/pts`: Required for terminal emulation (PTY).
- **Precedent**: `runc spec` generates these by default. Docker's `containerd` shim enforces them.

### 3.3. Build Context Preparation (Source Filtering)

**Standard**: Docker (`.dockerignore`).

Currently, `locald` uses a hardcoded list of exclusions when preparing the build context:

- `.git/`
- `node_modules/`
- `target/`
- `.locald/`

**Why not `.gitignore`?**

- `.gitignore` is for _version control_, not _build context_.
- Users often want to include files in the build that are ignored by git (e.g., a local `.env` file, or a generated config file needed for the build).
- Conversely, users often want to exclude files from the build that _are_ in git (e.g., large documentation folders, test data).
- **Precedent**: Docker explicitly separates `.dockerignore` from `.gitignore` for this reason.

**Why not `git clone`?**

- **Uncommitted Changes**: The primary use case of `locald` is _local development_. The user wants to run the code they are currently editing, which may not be committed yet.
- **Performance**: Cloning a large repo is slower than `rsync`ing the working tree (especially with exclusions).
- **Offline**: `git clone` might require network access if submodules are involved, whereas `rsync` is purely local.

**Policy**:

- `locald` **MUST** implement support for `.dockerignore` (or `.localdignore`) to allow users to control this.
- Until then, the hardcoded list serves as a "Default Ignore Policy" to prevent common failures.

### 3.4. Future Architecture: Zero-Copy Build Context

**Observation**: "It's 2025." Copying files (even with `rsync`) is inefficient and outdated for local development.

**Target Architecture**: Rootless OverlayFS.

Instead of copying files to a temporary directory, `locald` should compose the build context using a filesystem overlay:

1.  **LowerDir**: The user's source directory (Read-Only).
2.  **UpperDir**: A temporary scratch directory (Read-Write).
3.  **Whiteouts**: Files listed in `.dockerignore` are "masked" in the UpperDir using whiteout nodes (char 0/0) _before_ the build starts.

**Benefits**:

- **Zero Copy**: Instant startup, regardless of repo size.
- **Isolation**: Writes during the build (e.g., `npm install`) happen in the UpperDir, leaving the user's source pristine.
- **Safety**: The source is mounted Read-Only, preventing accidental modification by the build process.

**Do we still need `.dockerignore`?**

**Yes.** In a Zero-Copy model, `.dockerignore` defines the _negative space_ of the overlay.

- We scan the source directory.
- For every file matching `.dockerignore` (e.g., `node_modules`, `.env`), we create a **whiteout node** (character device 0/0) in the `UpperDir`.
- This effectively "deletes" the file from the container's view without touching the source or copying data.

**Implementation Path**:

1.  **Primary (Embedded)**: Use the **`fuse-backend-rs`** crate (from Kata Containers). This allows us to embed the OverlayFS logic directly into the `locald` binary, avoiding the need for an external `fuse-overlayfs` executable.
2.  **Fallback (Implemented)**: Use `reflink` crate (CoW) on supported filesystems (btrfs, xfs, modern ext4).
3.  **Legacy**: Fallback to `rsync` (Removed).

### 3.5. Cross-Platform Strategy

The "Zero-Copy" architecture adapts to macOS and Windows using our standard virtualization strategy (Lima and WSL2).

#### macOS (Lima)

- **Architecture**: `locald` manages a lightweight Linux VM via **Lima**.
- **Mount**: The project directory is mounted into the VM (via `virtiofs`) as Read-Only.
- **Overlay**: Inside the VM, we create the OverlayFS:
  - **LowerDir**: The `virtiofs` mount of the host source.
  - **UpperDir**: A directory inside the VM's ephemeral storage.
- **Result**: The build runs inside the Linux VM. It reads source files from the host on-demand (no full copy) and writes build artifacts to the VM's disk.

#### Windows (WSL2)

- **Architecture**: `locald` runs inside the **WSL2** utility VM.
- **Overlay**:
  - If the source is in the WSL filesystem (Recommended): Works exactly like native Linux.
  - If the source is in the Windows filesystem (`/mnt/c`): We use the `/mnt/c/...` path as the **LowerDir**.
- **Result**: Zero-copy build context even for files on the Windows host.

### 3.6. Environment Variables

**Standard**: CNB Platform API.

- `CNB_PLATFORM_API`: Specifies the API version.
- `CNB_EXPERIMENTAL_MODE`: Controls experimental features.

## 4. Implementation Plan (Stage 2)

- [x] **Implement OCI Spec Generation**: `locald-builder/src/runtime_spec.rs` implements the OCI config generation.
- [x] **Implement Workspace Prep**: `locald-builder/src/lifecycle.rs` implements the CoW copy logic (replacing `rsync`).
- [ ] **Formalize Ignore Logic**: Replace hardcoded overrides with a parser for `.dockerignore`.

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/architecture/container-runtime.md` with these guarantees.

## 6. Drawbacks

- **Rigidity**: Adhering strictly to CNB paths (`/workspace`) might confuse users expecting standard Linux paths (`/usr/src/app`), though CNB is the chosen standard for `locald`.
