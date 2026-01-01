---
title: "macOS Platform Support Strategy"
stage: 1 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Cross-Platform
---

# RFC 0104: macOS Platform Support Strategy

## 1. Summary

This RFC defines the phased strategy for macOS support in `locald`. It reconciles
the Lima-based virtualization approach (RFC 0047, RFC 0061) with a practical
implementation plan that delivers value incrementally.

**Key Insight**: Most developers don't need containers for local development.
The core `locald` experience—process supervision, port management, HTTPS, and
the dashboard—can work natively on macOS without virtualization.

## 2. Motivation

macOS represents ~50% of the developer market. Without macOS support, `locald`
cannot achieve its vision of being the universal local development platform.

The original approach (RFC 0047, RFC 0061) proposed Lima VMs for all execution,
but this:

1. Adds significant complexity and latency
2. Requires large VM downloads (~500MB+)
3. Creates friction for the "Clone & Go" promise

This RFC takes a **progressive enhancement** approach: start with native macOS
support for exec services, then add Lima for container workloads.

## 3. Phased Implementation

### Phase 1: M2.0 — CI Foundation (✅ COMPLETE)

**Status**: Implemented in `.github/workflows/ci.yml#L292-L358`

- macOS GitHub Actions runner on every PR with Rust changes
- Builds release binary on macOS ARM64 (Apple Silicon)
- Runs unit tests (excluding Linux-only crates)
- Smoke tests: `--version`, `--help`, `doctor`

### Phase 2: M2.2 — Native Exec Services (🚧 IN PROGRESS)

**Goal**: `locald up` works on macOS for exec services (native processes).

#### 2.1 What Must Work (P0 — Critical Path)

| Feature                   | Linux Implementation  | macOS Adaptation            | Code Location                            |
| ------------------------- | --------------------- | --------------------------- | ---------------------------------------- |
| Daemon startup            | Via systemd or direct | Direct fork/exec            | `locald-server/src/main.rs`              |
| Exec service spawn        | `fork()`/`exec()`     | Same (POSIX)                | `locald-server/src/runtime/process.rs`   |
| Port allocation           | Ephemeral ports       | Same                        | `locald-utils/src/port.rs`               |
| HTTP/HTTPS proxy          | Axum + rustls         | Same                        | `locald-server/src/proxy.rs`             |
| Dashboard serving         | Embedded assets       | Same                        | `locald-server/src/proxy.rs`             |
| Log streaming (CLI)       | IPC over Unix socket  | Same (POSIX)                | `locald-cli/src/handlers.rs`             |
| Log streaming (SSE)       | Server-Sent Events    | Same                        | `locald-server/src/sse.rs`               |
| Health checks             | TCP/HTTP/command      | Same                        | `locald-server/src/health.rs`            |
| `locald status/logs/stop` | IPC commands          | Same                        | `locald-cli/src/handlers.rs`             |
| `locald try`/`locald run` | Process spawn + env   | Same                        | `locald-cli/src/handlers.rs`             |
| Managed Postgres          | `postgresql_embedded` | Same (cross-platform crate) | `locald-server/src/services/postgres.rs` |
| Project registry          | XDG data dir          | Same (dirs crate)           | `locald-core/src/registry.rs`            |

#### 2.2 Privileged Operations (P1)

**Implementation Decision**: The setuid shim architecture works on **both Linux and macOS** using the same pattern:

- **FD Passing via SCM_RIGHTS**: Unix domain sockets with `SCM_RIGHTS` ancillary data work identically on Linux and macOS.
- **Setuid Bit**: macOS supports setuid binaries with the same semantics as Linux.
- **Result**: Clean UX parity for privileged port binding (80/443) across platforms.

| Feature                  | Linux                      | macOS                         | Behavior                                        |
| ------------------------ | -------------------------- | ----------------------------- | ----------------------------------------------- |
| Privileged ports (<1024) | Via `locald-shim` (setuid) | Via `locald-shim` (setuid)    | Same pattern: FD passing over Unix socket       |
| `/etc/hosts` automation  | Via `locald-shim` (setuid) | Via `locald-shim` (setuid)    | Same pattern: shim writes to `/etc/hosts`       |
| `locald doctor`          | Full system check          | macOS-specific checks         | Check shim, check Keychain trust                |
| HTTPS cert trust         | `update-ca-certificates`   | `security-framework` crate    | Native Keychain API (not CLI)                   |

#### 2.3 Compile-Time Gated Features (Linux-Only)

**Implementation Decision**: Container-related shim commands are gated at **compile time** using Rust `#[cfg(target_os = "linux")]` attributes. This is NOT a separate binary—it's the same `locald-shim` with conditional compilation.

| Feature                           | Gating Strategy                        | macOS Path                          |
| --------------------------------- | -------------------------------------- | ----------------------------------- |
| `locald admin setup`              | Available on macOS                     | Installs setuid shim (same as Linux)|
| Cgroup resource cleanup           | `#[cfg(target_os = "linux")]`         | Use process groups (SIGKILL)        |
| Container services (libcontainer) | `#[cfg(target_os = "linux")]`         | Lima integration (future M2.1)      |
| Namespace creation                | `#[cfg(target_os = "linux")]`         | Lima integration (future M2.1)      |
| VMM (locald-vmm)                  | `#[cfg(target_os = "linux")]` (KVM)   | Hypervisor.framework (future)       |

**Rationale for Compile-Time Gating**:
- Single binary distribution (no "macOS edition" vs "Linux edition")
- Clear compile errors if Linux-only code is accidentally used on macOS
- Future Lima support can be added alongside native Linux paths
- Matches existing Rust ecosystem patterns (e.g., `std::os::unix` vs `std::os::linux`)

#### 2.4 Code Changes Required

##### 2.4.1 Platform Detection

```rust
// crates/locald-utils/src/platform.rs (new file)
pub enum Platform {
    Linux,
    MacOS,
    // Windows (future)
}

impl Platform {
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        return Platform::Linux;
        #[cfg(target_os = "macos")]
        return Platform::MacOS;
    }

    /// The setuid shim works on both Linux and macOS.
    /// Uses SCM_RIGHTS for FD passing on both platforms.
    pub fn supports_shim(&self) -> bool {
        matches!(self, Platform::Linux | Platform::MacOS)
    }

    /// Cgroups are Linux-only. macOS uses process groups for cleanup.
    pub fn supports_cgroups(&self) -> bool {
        matches!(self, Platform::Linux)
    }

    /// Container isolation via libcontainer requires Linux namespaces.
    /// macOS will use Lima for container workloads (future).
    pub fn supports_containers(&self) -> bool {
        matches!(self, Platform::Linux)
    }
}
```

##### 2.4.2 Conditional Compilation Strategy

**Shim availability**: The shim works on both Linux and macOS. Container-specific commands are compile-time gated.

Files requiring `#[cfg(target_os)]` guards for **container features only**:

| File                                     | Change                                              |
| ---------------------------------------- | --------------------------------------------------- |
| `locald-shim/src/commands/container.rs`  | `#[cfg(target_os = "linux")]` for libcontainer code |
| `locald-shim/src/commands/cgroup.rs`     | `#[cfg(target_os = "linux")]` for cgroup management |
| `locald-server/src/runtime/container.rs` | Gate container service spawning on Linux            |
| `locald-utils/src/privileged.rs`         | macOS-specific doctor checks (cgroups N/A)          |

Files that work **identically on both platforms**:

| File                                     | Behavior                                            |
| ---------------------------------------- | --------------------------------------------------- |
| `locald-shim/src/commands/bind.rs`       | SCM_RIGHTS FD passing (POSIX)                       |
| `locald-shim/src/commands/hosts.rs`      | `/etc/hosts` modification                           |
| `locald-cli/src/handlers.rs:admin_setup` | Shim installation (works on macOS)                  |
| `locald-utils/src/shim.rs`               | Shim discovery (same path on both platforms)        |

##### 2.4.3 Process Cleanup Without Cgroups

On Linux, we use cgroups to reliably kill all descendant processes. On macOS:

```rust
// crates/locald-utils/src/process.rs
pub fn kill_process_tree(pid: Pid) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // Use cgroup freezer or SIGKILL to cgroup
        kill_via_cgroup(pid)
    }

    #[cfg(target_os = "macos")]
    {
        // Use process groups (less reliable but functional)
        // 1. Send SIGTERM to process group
        // 2. Wait with timeout
        // 3. Send SIGKILL if needed
        kill_via_process_group(pid)
    }
}
```

##### 2.4.4 Browser Open Command

Already implemented in `crates/locald-cli/src/handlers.rs#L793`:

```rust
#[cfg(target_os = "macos")]
let open_cmd = "open";
#[cfg(not(target_os = "macos"))]
let open_cmd = "xdg-open";
```

##### 2.4.5 HTTPS Certificate Trust

**Implementation Decision**: Use the `security-framework` crate for native macOS Keychain API access. Do NOT shell out to the `security` CLI.

```rust
// crates/locald-utils/src/cert.rs (new or extend existing)
pub fn trust_certificate(cert: &Certificate) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // Copy to /usr/local/share/ca-certificates/
        // Run update-ca-certificates
        install_to_system_store(cert)
    }

    #[cfg(target_os = "macos")]
    {
        // Use security-framework crate for native Keychain API access.
        // This is more reliable than shelling to `security` CLI and avoids
        // parsing CLI output or handling shell escaping issues.
        use security_framework::trust_settings::{TrustSettings, TrustSettingsForCertificate, Domain};
        use security_framework::certificate::SecCertificate;

        let sec_cert = SecCertificate::from_der(cert.to_der()?)?;
        
        // TrustSettings::set_trust_settings_always() marks the cert as
        // trusted for all purposes (SSL, code signing, etc.)
        TrustSettings::set_trust_settings_always(
            &sec_cert,
            Domain::Admin,  // Requires admin privileges (via shim)
        )?;
        
        Ok(())
    }
}
```

**Why `security-framework` over CLI**:
- **Type safety**: Native API returns structured errors, not exit codes
- **Reliability**: No shell escaping, path quoting, or output parsing issues
- **Idiomatic**: Follows Rust ecosystem patterns (cf. `native-tls` crate)
- **Testability**: Can mock the API in tests; CLI mocking is fragile
```

#### 2.5 Testing Strategy

| Tier        | What                                          | Where                | When            |
| ----------- | --------------------------------------------- | -------------------- | --------------- |
| Unit tests  | All crates except `locald-shim`, `locald-vmm` | GitHub Actions macOS | Every PR        |
| Smoke tests | `--version`, `doctor`, `init`                 | GitHub Actions macOS | Every PR        |
| Integration | `locald up` with exec service                 | GitHub Actions macOS | Every PR (new)  |
| E2E         | Full workflow (init → up → dashboard → stop)  | Manual / Pre-release | Before releases |

**New CI Job** (to add):

```yaml
macos-integration:
  name: macOS Integration Test
  needs: [macos-build]
  runs-on: macos-latest
  steps:
    - uses: actions/checkout@v4
    - name: Download binary
      uses: actions/download-artifact@v4
    - name: Test exec service lifecycle
      run: |
        mkdir -p /tmp/test-project && cd /tmp/test-project
        cat > locald.toml << 'EOF'
        [project]
        name = "macos-test"
        [services.web]
        command = "python3 -m http.server $PORT"
        EOF
        ./locald up --detach
        sleep 5
        curl -f http://localhost:8080 || exit 1
        ./locald stop
```

### Phase 3: M2.1 — Container Services via Lima (Future)

**Goal**: Support `container` service type on macOS using Lima VMs.

**Prerequisites**:

- M2.2 complete (native exec services work)
- RFC 0047 / RFC 0061 Lima strategy finalized

**Scope**:

- Download/manage Lima binary
- Create `locald-vm` with VirtioFS mounts
- Proxy container commands through Lima
- Port forwarding from VM to host

**Not in Scope for M2.1**:

- Native Hypervisor.framework integration (future M2.x)
- Windows/WSL2 support (separate milestone)

## 4. Known Limitations

### 4.1 Privilege Separation on macOS (Resolved)

**Update**: The setuid shim architecture works on macOS with full UX parity for core features:

| Feature               | Linux                 | macOS                 | Status      |
| --------------------- | --------------------- | --------------------- | ----------- |
| Privileged ports      | ✅ via shim           | ✅ via shim           | **Parity**  |
| `/etc/hosts`          | ✅ via shim           | ✅ via shim           | **Parity**  |
| HTTPS cert trust      | ✅ system store       | ✅ Keychain API       | **Parity**  |
| Cgroup management     | ✅ via shim           | ❌ N/A                | Linux-only  |
| Container isolation   | ✅ via libcontainer   | ❌ N/A (Lima future)  | Linux-only  |

**Key Insight**: The core shim operations (FD passing via `SCM_RIGHTS`, setuid execution) are POSIX-standard and work identically on both platforms. Only Linux-specific kernel features (cgroups, namespaces) require platform gating.

### 4.2 No Native Container Isolation

Until M2.1 (Lima integration), container services will error on macOS:

```
Error: Container services require Lima (not yet implemented on macOS).
Hint: Use `command = "..."` for native process execution, or wait for locald v1.1.
```

### 4.3 File System Differences

- macOS uses case-insensitive FS by default (APFS)
- Some path lengths may differ
- `/tmp` behavior differs (macOS: `/private/tmp`)

## 5. Success Criteria

### M2.2 Complete When:

- [ ] `locald up` starts daemon on macOS (no errors)
- [ ] Exec services spawn and receive `$PORT`
- [ ] HTTP proxy routes to services correctly
- [ ] Dashboard accessible at `localhost:8080`
- [ ] `locald logs <service>` streams output
- [ ] `locald stop` cleanly terminates services
- [ ] `locald doctor` reports macOS-specific status
- [ ] CI runs macOS integration test on every PR
- [ ] Managed Postgres works on macOS

### Quality Gates:

- No `#[cfg(target_os = "linux")]` code that breaks compilation on macOS
- All unit tests pass on macOS runner
- `cargo clippy` clean on macOS target

## 6. Migration Notes

For users upgrading from Linux-only versions:

1. **No action required** for exec service users
2. **Container users** must wait for M2.1 or use Docker Desktop
3. **Custom shim configurations** do not apply on macOS

## 7. Related RFCs

- **RFC 0047**: Cross-Platform Container Runtime (Lima strategy)
- **RFC 0061**: Cross-Platform Host Architecture (VirtioFS, platform matrix)
- **RFC 0101**: Arc 2 Runtime Isolation (Driver pattern alternative)

## 8. Appendix: macOS-Specific Code Audit

Files with existing macOS-specific code:

| File                              | Purpose                | Status                |
| --------------------------------- | ---------------------- | --------------------- |
| `locald-cli/src/handlers.rs#L793` | Dashboard open command | ✅ Complete           |
| `locald-vmm/src/macos.rs`         | VMM stub (empty)       | Placeholder for M2.3+ |
| `xtask/src/coverage.rs#L18`       | Coverage tool paths    | ✅ Complete           |

Files requiring M2.2 changes:

| File                                  | Change Required                                     |
| ------------------------------------- | --------------------------------------------------- |
| `locald-shim/src/commands/container.rs` | `#[cfg(target_os = "linux")]` for libcontainer    |
| `locald-shim/src/commands/cgroup.rs`  | `#[cfg(target_os = "linux")]` for cgroup ops       |
| `locald-utils/src/cert.rs`            | Add `security-framework` for macOS Keychain API    |
| `locald-utils/src/privileged.rs`      | macOS doctor checks (skip cgroup checks)           |
| `locald-server/src/runtime/container.rs` | Gate container services on Linux                |
| `Cargo.toml` (locald-utils)           | Add `security-framework` as macOS dependency       |
