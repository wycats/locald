---
title: Remove Container Workflow Support
stage: 1
feature: Engineering Excellence
exo:
    tool: exo rfc create
    protocol: 1
---


# RFC 0138: Remove Container Workflow Support


## Summary

Remove the "run locald from inside containers" workflow, including the `host-spawn` crate and socket-based privilege delegation to a host shim daemon.

This RFC does **not** require eliminating all container detection. Minimal container detection may remain (or be reintroduced) solely to provide **helpful error messages** when `locald` is invoked inside a container.

This simplifies the codebase by ~2,000 lines and eliminates a complex architectural pattern that proved impractical even for the maintainer.

## Motivation

### The Feature Never Worked Well

The container workflow was designed to support development environments where:

1. The user runs VS Code attached to a Toolbx or Distrobox container
2. `locald` runs inside the container
3. Privileged operations are delegated to the host via a socket-based shim daemon

This architecture required:

- A separate daemon (`locald-shim serve`) running on the host
- Socket-based IPC between container and host
- Complex container environment detection (Toolbx, Distrobox, Flatpak, Docker, WSL2)
- Auto-start logic to spawn the host daemon from within the container
- Multiple privilege escalation paths (`flatpak-spawn`, `distrobox-host-exec`, etc.)

**The maintainer, who designed this feature for their own use, has abandoned it.** After extensive use, the complexity proved unworkable:

- Container detection heuristics were fragile
- Auto-start logic had race conditions
- Two-daemon architecture doubled failure modes
- VS Code remote development introduced its own complications

### Complexity Cost

The container workflow adds ~2,000 lines of code:

| Component | Lines |
|-----------|-------|
| `crates/host-spawn/` crate | ~400-600 |
| `crates/locald-utils/src/container.rs` (host-spawn re-export + container helpers) | ~240 |
| `crates/locald-utils/src/shim_client.rs` (socket client) | ~693 |
| `crates/locald-utils/src/privileged.rs` (socket mode + auto-start) | ~150-250 |
| `crates/locald-shim/src/daemon.rs` + `crates/locald-shim/src/protocol.rs` (socket daemon/protocol) | ~300-600 |
| `crates/locald-core/src/config/global.rs` (`ContainerConfig`) | ~20-40 |
| **Total** | **~1,800-2,400** |

This code:

- Adds platform-specific detection heuristics that require ongoing maintenance
- Complicates the privilege acquisition model with conditional paths
- Requires a separate daemon with its own lifecycle management
- Creates testing surface area across multiple container runtimes

### The Alternative is Better

**The pattern of building inside a container and running on the host works fine.** You can absolutely:

- Use a containerized toolchain (Rust, Node, etc.) to build locald
- Run the resulting binary on the host
- Develop inside a container while locald runs on the host

What *doesn't* work well is the **inverse**: running locald inside a container and having it reach out to the host for privileged operations. This requires:

- Precise container environment detection heuristics
- Socket-based IPC that must work across container boundaries  
- Auto-start coordination between two daemons
- Platform-specific host-exec mechanisms (`flatpak-spawn`, `distrobox-host-exec`, etc.)

The Toolbx/Distrobox host-exec pattern is fragile because it requires conditions to be exactly right. Despite significant investment, the maintainer never got it working reliably without constant workflow friction.

**For users who need `locald` CLI access inside containers**, `distrobox-export` works:

```bash
# From inside distrobox
distrobox-export --bin $(which locald)
```

This **inverts the architecture**: instead of locald reaching out to the host, the host's locald binary is exposed into the container. Benefits:

- Zero locald-side code required
- Works with any container runtime distrobox supports
- No socket protocol or daemon coordination
- User controls exactly what's exported
- Standard pattern that distrobox users already understand

### Alignment with Host-First Strategy

RFC 0069 already established host-first execution as the default. This RFC completes that direction by removing the container-first alternative entirely.

## Scope of Removal

### Crates to Delete

- **`crates/host-spawn/`** - Entire crate (container detection, host execution mechanisms)

### Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Remove `host-spawn` member |
| `crates/locald-utils/Cargo.toml` | Remove `host-spawn` dependency |
| `crates/locald-utils/src/lib.rs` | Remove `container` module re-export (and any socket-related exports) |
| `crates/locald-utils/src/container.rs` | Remove host-spawn re-exports and host-exec helpers (or delete entire file) |
| `crates/locald-utils/src/shim_client.rs` | Remove socket client (only used for container workflow) |
| `crates/locald-utils/src/privileged.rs` | Remove socket mode and daemon auto-start path; setuid-only |
| `crates/locald-shim/src/daemon.rs` | Remove shim daemon (socket server) implementation |
| `crates/locald-shim/src/protocol.rs` | Remove socket protocol definitions |
| `crates/locald-core/src/config/global.rs` | Remove `ContainerConfig` struct |
| `crates/locald-cli/src/utils.rs` | Remove/adjust host-exec template references tied to host-spawn |

### Documentation to Remove/Update

| Document | Action |
|----------|--------|
| `docs/manual/development/container-environments.md` | Update to remove "run locald inside containers" workflow; replace with "run on host" + export/host-usage patterns |
| `docs/manual/features/doctor.md` | Update container-specific checks and guidance |
| `docs/research/host-container-privilege-split.md` | Archive or update to reflect removal decision |
| `docs/rfcs/stage-1/0130-host-shim-daemon.md` | Mark as withdrawn (see RFC Withdrawal Process) |
| `docs/rfcs/stage-2/0130-host-shim-daemon.md` | Mark as withdrawn (see RFC Withdrawal Process) |
| `docs/rfcs/stage-2/0134-host-spawn-crate-guest-to-host-execution.md` | Mark as withdrawn (see RFC Withdrawal Process) |
| README and other docs | Remove container workflow references |

### Simplifications Enabled

**Privilege Acquisition** simplifies from:

```rust
// Before: Socket-first with container detection
if in_container {
    try_socket() -> auto_start_daemon() -> retry_socket()
} else {
    use_setuid_shim()
}
```

To:

```rust
// After: Setuid-only (plus optional container detection for friendly errors)
if is_containerized() {
    return Err("locald does not support running inside containers; run on the host or export the host binary into your container");
}
find_and_verify_setuid_shim()
```

**Daemon Architecture** simplifies from two daemons (server + shim daemon) to one daemon (server only). The shim remains a setuid helper for privilege escalation, not a long-running daemon.

## Daemon Removal Decision

This RFC proposes removing the socket daemon entirely:

- Remove `locald-shim serve` (or equivalent shim daemon entrypoint)
- Remove the socket protocol and the socket client
- Keep (and simplify) the setuid shim path used for host privilege escalation

If `locald-shim` currently serves additional purposes beyond the socket daemon, those should remain; only the socket daemon/protocol responsibilities are removed.

## Affected Callsites Audit (Must Be Completed in Implementation)

- `crates/locald-utils/src/privileged.rs`: remove `PrivilegeMode::Socket` and any `.shim_client()` paths
- `crates/locald-utils/src/shim_client.rs`: delete and update all callsites
- `crates/locald-utils/src/container.rs`: remove `host-spawn` re-exports and host-exec helpers
- `crates/locald-cli/src/doctor.rs` and `docs/manual/features/doctor.md`: update container behavior and guidance
- Audit all references to `ShimClient`, `PrivilegeMode::Socket`, `host_spawn`, `TOOLBOX_PATH`, `DISTROBOX_ENTER_PATH`, `FLATPAK_ID`

## Migration Path

### For Existing Container Workflow Users

1. **Stop using `locald` from inside containers**
2. **Run `locald` on the host directly**
3. **If you need `locald` CLI inside containers**, use `distrobox-export`:
   ```bash
   distrobox-export --bin $(which locald)
   ```

### Building in a Container, Running on the Host

This RFC explicitly supports the common pattern:

1. Build locald using a containerized toolchain (Rust, Node, etc.)
2. Run the resulting `locald` binary on the host
3. Optionally expose that host binary into containers for convenience (e.g. distrobox export)

This avoids the fragile "host-exec" inversion (locald-in-container delegating to host via daemon/socket).

## Deprecation Strategy

This is a pre-1.0 breaking change. If we choose a staged rollout instead of immediate removal:

1. **Deprecation warnings (optional)**: detect container environments and emit a warning that running `locald` inside containers is deprecated, with guidance.
2. **Removal**: remove socket mode and host-exec integration; keep container detection only for error messages.

If we choose immediate removal, we should still keep the same error messaging and migration guidance.

### For Immutable Distro Users (Silverblue, Kinoite)

The recommended workflow becomes:

1. Install locald on the host (it's a single binary + shim)
2. Run `locald up` on the host
3. Access services via `*.localhost` domains from anywhere (host or containers)

The `*.localhost` domains and HTTPS certificates work the same regardless of where the client runs.

## Risks and Mitigations

### Risk: Breaking Existing Users

**Mitigation**: This is a pre-1.0 project with no stable API guarantees. The changelog will clearly document this as a breaking change. The alternative (`distrobox-export`) is documented as the supported path.

### Risk: Losing Immutable Distro Market

**Mitigation**: Immutable distro users are a niche audience. The distrobox-export approach actually works better for them since it doesn't require running a socket daemon on the host.

### Risk: Incomplete Removal

**Mitigation**: The feature is well-isolated. The `host-spawn` crate has clear boundaries, and container detection is localized to specific files.

## Implementation Plan

### Phase 1: Remove Code

1. Delete `crates/host-spawn/` entirely
2. Remove workspace member from root `Cargo.toml`
3. Remove `host-spawn` re-exports and helpers from `crates/locald-utils/src/container.rs`
4. Remove `crates/locald-utils/src/shim_client.rs` and update callsites
5. Simplify `crates/locald-utils/src/privileged.rs` to setuid-only
6. Remove socket daemon/protocol from `crates/locald-shim/src/daemon.rs` and `crates/locald-shim/src/protocol.rs`
7. Remove `ContainerConfig` from `crates/locald-core/src/config/global.rs`

### Phase 2: Simplify Privilege Model

1. Remove socket-based privilege acquisition path
2. Simplify `Privileged::acquire()` to setuid-only
3. Remove `PrivilegeMode::Socket` variant
4. Update `locald doctor` to:
    - Detect container environments (optional)
    - Provide a clear "not supported" message
    - Suggest host execution and/or exporting the host binary

### Phase 2.5: Tests and CI

1. Remove or update any unit tests targeting socket mode and host-exec templates
2. Remove or update any tests that validate container workflow behavior
3. Update CI jobs if any container-workflow-specific checks exist

### Phase 3: Update Documentation

1. Update `docs/manual/development/container-environments.md` to reflect the supported patterns
2. Archive withdrawn RFCs (0130, 0134)
3. Add "Distrobox Export" section to docs as alternative
4. Update README to remove container workflow references

### Phase 4: Verify

1. Run full test suite
2. Verify `locald doctor` works correctly
3. Verify privilege acquisition works on fresh install
4. Update CI if any container-specific tests exist

## Alternatives Considered

### 1. Keep but Deprecate

Mark container workflow as deprecated, remove in v1.0.

**Rejected**: Adds maintenance burden for a feature that doesn't work well. Pre-1.0 status allows breaking changes.

### 2. Keep but Simplify

Remove auto-start logic, require manual daemon management.

**Rejected**: Still requires ~1,500 LOC of container detection and socket protocol code.

### 3. Document distrobox-export as Primary

Keep container workflow but document distrobox-export as preferred.

**Rejected**: Confusing to have two ways to do the same thing, one of which is complex and unsupported.

## Decision

**Proposed**: Complete removal of container workflow support, with `distrobox-export` documented as the alternative for users who need locald CLI access inside containers.

## RFC Withdrawal Process

If this RFC is accepted, the following RFCs should be marked **WITHDRAWN** with a short notice at the top of the document pointing to RFC 0138:

- `docs/rfcs/stage-1/0130-host-shim-daemon.md`
- `docs/rfcs/stage-2/0130-host-shim-daemon.md`
- `docs/rfcs/stage-2/0134-host-spawn-crate-guest-to-host-execution.md`

Note: there is also an unrelated RFC numbered 0134 at `docs/rfcs/stage-0/0134-macos-domain-cert-requirements.md`; it is not affected.

## References

- RFC 0069: Host-First Execution Strategy (`docs/rfcs/0069-host-first-execution.md`)
- RFC 0110: Privileged Capability Acquisition (`docs/rfcs/stage-1/0110-privileged-capability-acquisition.md`)
- RFC 0130: Host Shim Daemon (to be withdrawn)
- RFC 0134: Host-Spawn Crate: Guest-to-Host Execution (to be withdrawn)
- [Distrobox documentation on exporting binaries](https://distrobox.it/usage/distrobox-export/)
