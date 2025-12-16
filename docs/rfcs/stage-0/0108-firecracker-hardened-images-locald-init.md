---
title: "Firecracker Hardened Images via locald-init (Template + Inject)"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: VMM
---

# RFC 0108: Firecracker Hardened Images via locald-init (Template + Inject)

## 1. Summary

This RFC proposes an architectural extension that allows `locald` to run hardened OCI images (distroless/scratch, “no shell, no init”) as MicroVMs.

The design replaces “boot the image” with **fabricate + inject**:

- **Fabricator (host-side)**: materialize an OCI image into a bootable root filesystem by cloning a preformatted **ext4 template**, mounting it, extracting layers (including OCI whiteouts), and injecting a minimal PID1.
- **Sentinel (guest-side)**: a new `locald-init` static Rust binary installed as `/sbin/init`, which reads a boot contract (`/etc/locald_boot.json`) and `exec()`s the intended entrypoint.
- **Privilege bridge**: extend `locald-shim` with a narrow syscall-based `mount-loop` + `umount` protocol, using FD-based path containment and mount-namespace isolation.
- **Asset manager**: introduce `locald-utils::assets` to download, verify, cache, and serve large immutable inputs (templates, kernels, etc.). Default upstream starts as GitHub Releases, with a deliberate migration plan to an R2-backed HTTPS mirror.

**Cross-platform direction (near-term, capability-gated)**: while fabrication is Linux-filesystem oriented, VM hosting is expected to become host-native relatively soon (fast follow; not required for the initial implementation):

- Linux: KVM backend (baseline)
- macOS: Virtualization.framework (VZ) backend (near-term)
- Windows: WHP backend (preferred; near-term)

Therefore, the RFC makes **“VMM backend”** an explicit capability layer with pluggable implementations.

## 2. Motivation

OCI-to-VM workflows often assume a “fat” base image with a shell or init system. Hardened images intentionally omit these to reduce attack surface and CVE exposure.

Without a valid PID1 (`/sbin/init`) or a fallback shell (`/bin/sh`), Linux boots fail early. To support hardened agents (“Exosuit”-style workloads), locald must:

- supply a minimal, deterministic init contract
- materialize the OCI filesystem without requiring host tools like `mkfs.ext4`
- keep privileged operations behind a minimal and auditable boundary
- preserve cross-platform ambitions without baking in “nested virtualization is fine”

## 3. Goals

- Boot distroless/scratch images as MicroVMs in a deterministic way.
- Avoid a hard runtime dependency on `e2fsprogs` / `mkfs.ext4`.
- Make privileged filesystem operations syscall-driven and narrowly scoped.
- Provide an explicit, testable contract between:
  - `locald` (orchestrator)
  - `locald-shim` (privileged host operations)
  - the guest (`locald-init`)
- Define an asset system that supports:
  - download-on-first-use
  - sha256 verification
  - offline/airgapped usage (preseeded caches)
  - migration from GitHub Releases to R2 without changing consumer APIs.
- Architect for multiple guest architectures (Phase 1: `x86_64`, Phase 2: `aarch64`) without redesign.
- Make host-native VM backends (macOS VZ, Windows WHP) a near-term first-class direction.

## 4. Non-Goals

- Provide a general-purpose init system inside the guest. `locald-init` is a minimal PID1, not systemd.
- Provide interactive debugging tools in the guest (no shell requirement).
- Implement VZ/Hyper-V/WHP backends in this RFC (Stage 0); the RFC specifies contracts and acceptance criteria.
- Replace the existing container runtime strategy. This adds/extends VM execution, not containers.

## 5. Terminology

- **User Host**: the OS where the user runs `locald` (Linux/macOS/Windows).
- **Execution Runtime**: the Linux environment where Linux-specific operations occur (native Linux; Lima on macOS; WSL2 on Windows). See RFC 0061.
- **VMM Host**: the environment and API used to boot the MicroVM. This may be:
  - Linux+KVM
  - macOS+VZ
  - Windows+WHP (optionally Hyper-V managed as fallback)
- **Fabrication**: transforming OCI image layers into a concrete root filesystem image.
- **Template**: a preformatted sparse ext4 image used as an immutable clone source.
- **Run directory**: per-instance ephemeral workspace for creating a VM.

## 6. User Experience (UX)

### 6.1 CLI

Primary user entrypoint:

```bash
locald run --vm \
  --cpus 2 --memory 512 \
  docker.io/my-org/hardened-agent:latest
```

Optional flags (strawman; exact shape TBD but must be stable enough for implementation):

- `--vm-backend=auto|kvm|vz|hyperv` (default `auto`). On Windows, this selects the _Windows_ backend, but does not yet lock in the exact hypervisor integration strategy.
- `--disk-size=1g|5g|20g` (maps to template SKUs; see assets)
- `--guest-arch=auto|x86_64|aarch64` (default `auto`)
- `--assets-dir <path>` (overrides asset root)
- `--runtime-dir <path>` (overrides runtime root)
- `--no-download` (fail if any required asset is missing)
- `--print-plan` (emit resolved plan: backend chosen, assets resolved, paths)

UX invariants:

- `locald` must show a **small number** of high-level progress steps (not per-layer spam).
- On failure, errors must identify:
  - which capability failed (assets / fabrication / boot)
  - the resolved paths involved
  - the remediation (preseed assets, change disk size, change backend)

### 6.2 Dashboard

VM services should appear like any service:

- status (starting/running/failed)
- logs (console + init logs)
- ports (forwarded)
- ability to stop/restart

Dashboard-specific detail is out-of-scope for Stage 0 but required to be compatible with the existing mental model.

## 7. Artifact Inventory (inputs vs runtime outputs)

### 7.1 Inputs (immutable, cached “assets”)

These are versioned and sha256-verified.

- **ext4 template images** (new)

  - `empty-1gb.ext4` (default)
  - `empty-5gb.ext4` (optional)
  - `empty-20gb.ext4` (optional)

- **kernel images** (existing precedent in tooling)

  - per guest arch (Phase 1: x86_64)

- **`locald-init` guest binary** (new)

  - delivered as **bundled** artifact if at all possible (see §10.1)
  - still treated as “an asset” conceptually for placement and verification purposes

- **VMM binaries** (TBD)
  - If `locald-vmm` is embedded as a library: no separate binary.
  - If we run a separate VMM process: it becomes a “tool” artifact.

### 7.2 Runtime outputs (ephemeral per run)

- cloned and mutated rootfs image: `vm_rootfs.ext4`
- mountpoints for fabrication
- run metadata + logs

### 7.3 Content cache (OCI)

OCI layers/manifests are not “assets” we publish; they are fetched from registries and cached content-addressably.

## 8. Canonical On-Disk Layout (Normative)

This RFC defines a canonical layout under the **Execution Runtime’s** XDG data directory. On non-Linux hosts, this is inside Lima/WSL2 unless explicitly configured otherwise.

### 8.1 Root selection

Let `LOCALD_DATA_ROOT` be:

1. `LOCALD_DATA_DIR` if set
2. else `$XDG_DATA_HOME/locald`
3. else `~/.local/share/locald`

Optional overrides:

- `LOCALD_ASSETS_DIR` overrides only the assets root
- `LOCALD_TOOLS_DIR` overrides only the tools root
- `LOCALD_RUNTIME_DIR` overrides only the runtime root

### 8.2 Layout

Under `LOCALD_DATA_ROOT`:

- `state/` (global state)
- `tools/<tool>/<version>/<platform-arch>/...` (downloaded executables; e.g., Lima)
- `assets/<asset-name>/<asset-version>/payload/<filename>` (immutable blobs)
- `oci/` (content-addressed cache)
- `runtime/runs/<run-id>/...` (ephemeral per-run output)

Asset installation invariants:

- downloads are staged in `.tmp/`, verified, then atomically renamed into `payload/`
- assets are immutable once installed
- concurrency is controlled via a per-asset-version `.lock` (flock or equivalent)

## 9. Architecture

### 9.1 High-level pipeline

1. **Ingest**: pull OCI image, resolve config/entrypoint.

2. **Fabricate rootfs**:

   - resolve ext4 template (by disk size SKU)
   - clone template → `vm_rootfs.ext4` (fast reflink if possible)
   - mount via privileged shim (loop + mount)
   - extract layers with whiteout semantics
   - inject `locald-init` and boot contract
   - unmount

3. **Boot**:
   - select VMM backend (`auto`)
   - attach kernel + rootfs
   - boot with `init=/sbin/init`

### 9.2 Capability model: VMM backend

The system must treat “boot a MicroVM” as a capability with multiple backend implementations.

Strawman trait (conceptual):

```rust
trait VmmBackend {
  fn name(&self) -> &'static str;
  fn detect(&self) -> Capability;

  fn boot(&self, spec: BootSpec) -> Result<VmHandle>;
}
```

`BootSpec` (conceptual) includes:

- guest arch
- kernel path
- rootfs image path
- vCPU count, memory
- kernel cmdline (includes `init=/sbin/init`)
- console/log routing
- port forwarding / networking attachment

Backends:

- **Linux/KVM backend**: baseline.
- **macOS/VZ backend**: fast follow (not required for initial implementation).
- **Windows/Hyper-V/WHP backend**: fast follow (not required for initial implementation).

Relationship to `locald-vmm`:

- Today, `locald-vmm` is Linux/KVM-shaped.
- This RFC assumes we will refactor the VMM layer so that `locald` targets a **backend-neutral interface**.
  - Option 1: `locald-vmm` becomes the umbrella crate, with backend modules (`kvm`, `vz`, `hyperv`).
  - Option 2: split into `locald-vmm-core` (device model, boot flow) and `locald-vmm-backend-*` crates.
  - Option 3: keep `locald-vmm` (KVM) and add a separate host-native VM launcher for VZ/Hyper-V.

Stage-0 requirement: pick one option before Stage 2 implementation; do not grow three unrelated VMM implementations without a shared `BootSpec`/`VmHandle` contract.

Stage-0 requirement: backend selection must be explicit and loggable (`--print-plan`).

Stage-0 requirement: on Windows, `--print-plan` must also report **which Windows strategy** was chosen:

- **WHP (preferred)**: use Windows Hypervisor Platform (WHP/WHPX) as the accelerator, and `locald` is the VMM (device model in-process).
- **Hyper-V managed VM (fallback)**: `locald` asks Hyper-V to run a VM and attaches a virtual disk.

Stage-0 requirement: `--vm-backend=auto` must attempt WHP first on Windows, then fall back _only if configured_ (or error with a clear remediation message).

Stage-0 requirement: if the chosen Windows strategy is not available because a Windows optional feature is disabled, `locald` must fail with an actionable message (e.g. “enable Windows Hypervisor Platform and reboot”, or “enable Hyper-V and reboot”).

Policy reference: this is an instance of **capability gating** as defined by the portability axiom. See [Axiom 7: Cross-Platform Portability — Capability Gating](../../design/axioms/environment/10-portability.md#capability-gating).

#### Capability probe + fallback order (normative)

Backend selection for `--vm-backend=auto` must follow an explicit probe order and produce a stable, loggable decision.

General rules:

- Probes must be **cheap** (no downloads; no long-running work).
- Probes must be **side-effect minimal** (create ephemeral handles/objects only; do not leave VMs running).
- `--print-plan` must include: chosen backend, probe results for all attempted backends, and the precise failure reason for each rejected backend.

Probe order:

- On Linux:

  - Probe KVM by attempting to open `/dev/kvm` and querying basic capabilities.
  - If KVM probe fails: report “VM boot unsupported on this host” (do not silently fall back to a different isolation mechanism).

- On macOS:

  - Probe VZ by checking OS/platform requirements and attempting to initialize the minimal Virtualization.framework boot configuration.
  - If VZ probe fails: report “VM boot unsupported on this host” and include remediation (OS version, entitlements, permissions).

- On Windows:
  1. Probe WHP (preferred) by attempting to call into the Windows Hypervisor Platform API surface and create a minimal partition/VP configuration.
     - If the call surface is missing/disabled, classify as “feature disabled” with remediation (“enable Windows Hypervisor Platform and reboot”).
  2. If (and only if) the user has enabled fallback (exact configuration mechanism TBD), probe “Hyper-V managed VM”.
     - If unavailable, classify as “feature disabled” with remediation (“enable Hyper-V and reboot”).
  3. If neither is available: report “VM boot unsupported on this host”.

Stage-0 requirement: if a Windows feature is disabled, the error must explicitly say that this is **expected** behavior unless the user enables the feature.

### 9.3 Cross-platform split: Fabrication vs Boot

This RFC separates:

- **Fabrication**: Linux filesystem operations requiring privileged mount/loop syscalls.
- **Boot**: platform-specific virtualization APIs.

Implications:

- On Linux: both fabrication and boot can happen in one environment.
- On macOS/Windows:
  - fabrication is expected to happen in the Execution Runtime (Linux VM)
  - boot is expected to be host-native (VZ / WHP)

This forces an explicit **artifact bridge** between the execution runtime and the user host for the finalized `vm_rootfs.ext4` (and kernel) so the host-native backend can attach them.

Stage-0 requirement: **host-visible boot artifacts**.

- The boot inputs (kernel image and finalized rootfs image) must be readable from the **VMM Host** filesystem at the moment `boot()` is invoked.
- Therefore, the system must treat “where fabrication happens” and “where boot reads from” as two potentially different storage roots:
  - **Execution Runtime root**: where OCI fetch/extract and ext4 fabrication may run (native Linux, Lima, WSL2).
  - **VMM Host root**: where host-native backends (macOS VZ, Windows WHP/Hyper-V) can open files.
- `--print-plan` must report:
  - the resolved runtime-side paths (where fabrication writes),
  - the resolved host-side paths (what the backend will open), and
  - the chosen bridge mechanism (shared handoff directory vs copy/stream).

#### Disk format compatibility constraint (normative)

The fabrication output must be attachable by the chosen backend without additional host tool dependencies.

- Linux/KVM backend (locald-vmm): expects a raw disk image file and presents it to the guest as a virtio block device.
- macOS/VZ backend: must be able to attach a disk image file directly (target: raw image attachment).
- Windows/WHP backend (preferred; `locald` is the VMM): expects a raw disk image file and presents it to the guest as a virtio block device.
- Windows/Hyper-V managed VM backend (fallback): may require `vhd`/`vhdx` container formats.

OPEN: if the **Hyper-V managed VM** path requires `vhdx`, we must either:

- ship `vhdx` template assets in addition to raw ext4 templates, or
- implement a `vhdx` wrapper/encoder internally, or
- accept a Windows-specific dependency (strongly discouraged by project goals).

Stage-0 requirement: the **WHP backend** must not require `vhd`/`vhdx`.

OPEN: define the preferred “Windows raw disk” story as one of:

- virtio-mmio (matching the Linux `locald-vmm` model), or
- a Windows-specific paravirtual device model that preserves “raw backing file” semantics.

Stage-0 strawman provides two strategies; implementations may choose one per platform:

#### Strategy A: Shared “handoff directory” (preferred for simplicity)

- The host exposes a directory into the execution runtime (VirtioFS for Lima; DrvFs/Plan9 for WSL).
- Fabrication writes final `vm_rootfs.ext4` into that shared directory.
- Host-native VMM backend reads the disk image from the host filesystem.

Pros: simple, no custom transport.
Cons: cross-OS filesystem performance may be poor.

#### Strategy B: Runtime-local fabricate then copy/stream out

- Fabricate on runtime-local filesystem (fast).
- After unmount, copy the resulting sparse file to host (scp/agent channel).

Pros: good performance.
Cons: requires an explicit transport protocol.

OPEN: decide per-platform defaults and performance safeguards.

## 10. Components

### 10.1 `locald-init` (new crate)

Purpose: minimal PID1 (“Sentinel”) for hardened images.

Behavior requirements:

- Must mount pseudo-filesystems: `proc`, `sysfs`, `devtmpfs` (or minimal `/dev`), and `tmpfs` for `/run`.
- Must configure `lo` up.
- Must act as a real init:
  - reap zombies (wait loop)
  - forward signals to child
  - exit semantics must not panic-kill the guest

Boot contract:

- Reads `/etc/locald_boot.json`.
- Sets environment variables.
- Applies uid/gid mappings if specified.
- Executes the resolved entrypoint.

Failure mode:

- If exec fails, log error to console and enter a sleep loop (keeps VM alive for log retrieval).

Delivery model:

- Phase 1: bundle `locald-init` with the locald release and install it into the fabricated rootfs.
  - Bundling mechanism is an implementation detail, but the effect must be:
    - no network required once locald is installed
    - integrity check (build-time pinned sha256 or equivalent)

Guest architecture:

- Phase 1: `x86_64-unknown-linux-musl`
- Phase 2: `aarch64-unknown-linux-musl`

### 10.2 `locald-shim` extensions (mount-loop)

This RFC builds directly on RFC 0107’s “safe mount-loop contract” and makes it concrete.

#### CLI protocol

- `locald-shim admin mount-loop --image <ABS> --target <ABS> --fstype ext4 [--readonly|--readwrite]`
- `locald-shim admin umount --target <ABS>`

Exit codes (normative):

- `0`: success
- `2`: invalid arguments
- `3`: containment violation / unsafe path
- `4`: mount/loop syscall failure
- `5`: cleanup failed (best-effort attempted)

STDOUT/STDERR contract:

- STDOUT: machine-readable single-line JSON on success (includes loop dev path)
- STDERR: human-readable error context (paths, syscall name, errno)

#### Security & hardening (normative)

- Must create a private mount namespace (`unshare(CLONE_NEWNS)` or equivalent) and set mounts `MS_PRIVATE`.
- Must avoid path traversal/symlink escape:
  - use FD-based operations (`openat`/`openat2`) against an allowed root
  - require containment within a single configured locald-owned root
  - reject non-regular image files
  - reject non-directory targets
- Must set conservative mount flags:
  - always `nosuid,nodev,noexec`
  - default `rw` only for fabrication targets, never for template payload

Cleanup invariants:

- On any failure after loop attachment:
  - unmount target if mounted
  - detach loop fd
  - remove transient mount directories if created by shim

OPEN: exact “allowed root” selection for shim filesystem operations (project-local vs global runtime root).

### 10.3 `locald-utils::assets` (new module)

This module becomes the single place where locald obtains and verifies large immutable inputs.

Resolution order (normative):

1. `LOCALD_ASSETS_DIR` env var
2. locald config (`locald.toml` / yaml; exact config format TBD)
3. XDG default: `$XDG_DATA_HOME/locald/assets`

Asset identity:

- `(name, version, platform, arch)` where applicable
- ext4 templates are arch-neutral

Installation algorithm (normative):

- Acquire `.lock` (flock)
- If `payload/<file>` exists and sha256 matches: return
- Download to `.tmp/<random>`
- Verify sha256
- Atomic rename into `payload/<file>`
- Write `manifest.json`

Upstream strategy:

- Default upstream: GitHub Releases (initial)
- Mirror strategy: configurable base URL (intended for R2-backed HTTPS)

OPEN: define asset registry source-of-truth (hardcoded table vs embedded manifest file).

### 10.4 `locald-vmm` Fabricator

`fabricate_rootfs(layers, image_config) -> PathBuf` produces a finalized disk image.

Note: the fabricate output format must be compatible with the selected backend (see §9.3 disk format constraint). In practice this implies:

- Phase 1: raw ext4 image output (`.ext4`)
- Phase N: additional outputs may be required (e.g. `.vhdx`) depending on backend constraints

Key requirements:

- Template retrieval via `locald-utils::assets`.
- Clone strategy:
  - try `FICLONE` (reflink) first
  - fallback to sparse-aware copy
  - fallback to plain copy (warn)
- Concurrency safety:
  - create output in temp name, then atomic rename

Mount/extract/inject sequence:

- create mount dir under run dir
- call shim `mount-loop` with `rw`
- apply layers in order with OCI whiteout semantics
- ensure `/sbin/init` exists (create dirs)
- write `/etc/locald_boot.json`
- call shim `umount`

OCI whiteouts (normative behavior):

- `.wh.<name>` removes `<name>` in the same directory
- `.wh..wh..opq` marks a directory as opaque (removes entries from lower layers)

Implementation notes:

- Prefer a single “apply layer tarball” implementation shared with container rootfs unpacking, if present.
- Preserve file modes, ownership, symlinks, hardlinks.

## 11. Boot Contract: `/etc/locald_boot.json`

Schema (normative):

```json
{
  "version": 1,
  "argv": ["/app/bin/agent", "--flag"],
  "env": { "KEY": "VALUE" },
  "workdir": "/",
  "uid": 0,
  "gid": 0,
  "supplementary_gids": [],
  "stdio": {
    "mode": "console"
  }
}
```

Rules:

- `argv` is required and must have at least one element.
- `env` keys must be valid POSIX env names.
- `uid/gid` default to 0 unless specified.
- Future fields must be ignored by older `locald-init` versions if possible.

## 12. Security Model

Threat model focus: a privileged setuid shim is the highest risk surface.

Security invariants:

- `locald` itself remains unprivileged.
- All privileged filesystem operations are:
  - narrow
  - syscall-driven
  - path-contained
  - auditable
- Template assets are immutable and never mounted readwrite.

## 13. Observability

- Guest console logs must be captured and surfaced as service logs.
- `locald-init` must prefix logs with a stable tag (e.g., `locald-init:`) for filtering.
- Fabrication steps must emit timing metrics per stage (clone/mount/extract/inject).

## 14. Implementation Plan (Stage 2)

- [ ] Add `locald-init` workspace member; implement PID1 requirements; build musl static artifact.
- [ ] Add `locald-utils::assets` module with sha256 verification + atomic install + mirror support.
- [ ] Extend `locald-shim` with clap parsing and implement `mount-loop` + `umount` (per RFC 0107 + this RFC).
- [ ] Implement `locald-vmm` fabrication pipeline (template clone, mount, extract, inject, unmount).
- [ ] Implement backend selection and Linux/KVM boot path integration.
- [ ] Define host-native backend contracts for macOS VZ and Windows Hyper-V/WHP; implement capability detection stubs.
- [ ] Add e2e tests:
  - distroless image boots and runs entrypoint
  - whiteout correctness test image
  - failure modes (missing template, too-small disk)

## 15. Context Updates (Stage 3)

- [ ] Add `docs/manual/features/vm-hardened-images.md`
- [ ] Update `docs/manual/features/execution-modes.md` (document VM lane + hardened images)
- [ ] Add `docs/manual/architecture/assets.md` (assets vs tools vs oci vs runtime)
- [ ] Add `docs/manual/architecture/vmm-backends.md` (KVM/VZ/Hyper-V capability model)

## 16. Drawbacks

- Adds complexity: more moving parts (init binary, templates, shim mounting).
- Privileged shim expansion increases security surface.
- Cross-platform host-native boot implies nontrivial backend work (VZ/Hyper-V).

## 17. Alternatives

- Require `mkfs.ext4` / `e2fsprogs` at runtime (rejected: not baseline; conflicts with “no prereqs” vibe).
- Rootless `debugfs` modification of ext4 without mounting (optional fallback, but still `e2fsprogs`).
- Use initramfs rather than block rootfs (rejected: memory overhead; deviates from “disk-backed”).

## 18. Unresolved Questions

- What is the authoritative “shim allowed root” for mount-loop operations?
  - runtime dir only?
  - project-local dir only?
  - both, via explicit allowlist?
- How do we best bridge artifacts for host-native VZ/Hyper-V boot without killing performance?
- Should kernels be managed as assets via `locald-utils::assets` to unify the download story?
- Do we ship multiple template SKUs up front or only on demand?

## 19. Future Possibilities

- Migrate default asset upstream from GitHub Releases to R2-backed HTTPS.
- Add a remote “VMM host” capability (if required by nested-virt constraints).
- Add `aarch64` guest support.
- Add richer networking and service discovery for VM services.
