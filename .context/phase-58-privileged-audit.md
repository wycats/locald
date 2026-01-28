# DDD Audit: Privileged Operations (Phase 58, Task 58.5)

Date: 2026-01-28  
Scope: Privileged operations, shim architecture, setup/trust flows

## 1. Privilege Model Overview

**What needs privileges**
- **Privileged ports (80/443)**: Binding is delegated to the setuid shim via `locald-shim bind-port`, which returns a bound FD to the daemon. See `crates/locald-shim/src/bind_port.rs`, `crates/locald-server/src/proxy/mod.rs`.
- **Container execution (OCI bundles)**: Shim runs containers via libcontainer as root (`locald-shim bundle run`). See `crates/locald-shim/src/bundle.rs`.
- **Cgroup v2 setup and cleanup**: Shim creates and manages `/sys/fs/cgroup` roots and can kill/prune locald cgroups. See `crates/locald-shim/src/cgroup.rs`, `crates/locald-shim/src/commands/cgroup_cleanup.rs`.
- **Hosts file sync**: Shim updates `/etc/hosts` in a bounded `# BEGIN locald` block. See `crates/locald-shim/src/hosts.rs`, `crates/locald-utils/src/hosts.rs`.
- **Trust store modification**: Shim (and CLI fallback) install the local Root CA into system trust stores. See `crates/locald-shim/src/trust.rs`, `crates/locald-cli/src/commands/trust.rs`.
- **Cleanup of root-owned artifacts**: Shim removes paths under `.locald`. See `crates/locald-shim/src/cleanup.rs`.
- **Debug port inspection**: Shim inspects system listeners (root-required for full visibility). See `crates/locald-shim/src/port.rs`.

**How privilege is acquired**
- **Primary mechanism**: **Setuid root shim** (`locald-shim`), installed via `locald admin setup`. See `crates/locald-shim/src/lib.rs`, `crates/locald-cli/src/commands/admin.rs`.
- **Interactive escalation**: `locald admin setup` re-execs via **pkexec** (polkit) if available, otherwise **sudo**. Non‑TTY fails with an explicit message. See `crates/locald-cli/src/commands/admin.rs`, `crates/locald-utils/src/privileged.rs`.
- **Polkit policy**: Optional policy installation performed by shim during admin setup. See `crates/locald-shim/src/policy.rs`.

**Security boundary**
- `locald` daemon runs unprivileged; `locald-shim` is the privileged boundary. It exposes a strict, typed CLI surface and does not execute the `locald` binary while root. See `crates/locald-shim/src/lib.rs`, `crates/locald-shim/src/main.rs`, RFC 0096.
- Shim discovery is restricted to a sibling/parent binary, per RFC 0097. See `crates/locald-utils/src/shim.rs`, `docs/rfcs/0097-strict-shim-discovery.md`.

---

## 2. Shim Inventory (Commands & Capabilities)

**Binary:** `locald-shim` (setuid root)

**Commands**
- `bundle run`: Run OCI bundle (root). See `crates/locald-shim/src/bundle.rs`.
- `bind-port`: Bind privileged port and pass FD via SCM_RIGHTS. See `crates/locald-shim/src/bind_port.rs`, `crates/locald-server/src/proxy/mod.rs`.
- `hosts sync`: Update `/etc/hosts` in bounded block. See `crates/locald-shim/src/hosts.rs`.
- `self-check`: Non-destructive probe to detect setuid viability. See `crates/locald-shim/src/self_check.rs`, `crates/locald-core/src/doctor/shim.rs`.
- `cleanup`: Remove `.locald` directories only. See `crates/locald-shim/src/cleanup.rs`.
- `cgroup root`: Create cgroup root (systemd or direct). See `crates/locald-shim/src/cgroup.rs`.
- `cgroup cleanup`: Kill & prune locald cgroups. See `crates/locald-shim/src/commands/cgroup_cleanup.rs`.
- `trust install`: Install Root CA into system trust store (invoking user's home). See `crates/locald-shim/src/trust.rs`.
- `policy install`: Install polkit policy (with immutable-distro fallback). See `crates/locald-shim/src/policy.rs`.
- `port`: Report listeners on a port. See `crates/locald-shim/src/port.rs`.

---

## 3. Friction Points by Persona

### New Hire
- **Doc drift on shim discovery**: Internal docs claim `LOCALD_SHIM_PATH` and PATH lookup; code only checks sibling/parent. Conflicting guidance in `docs/manual/security/` vs `crates/locald-utils/src/shim.rs` and RFC 0097.
- **Security doc references non-existent shim subcommand**: Security doc mentions `locald-shim cap` and setcap flow; shim has no `cap` command. See `docs/manual/security/`, `crates/locald-shim/src/main.rs`.
- **Where is shim installed?** Docs say `~/.cargo/bin`, but admin setup installs alongside the CLI binary. See `docs/manual/getting-started/installation.md`, `crates/locald-cli/src/commands/admin.rs`.

### Security Auditor
- **Setuid surface area**: Shim runs `bundle run` (libcontainer) as root; it's large enough to justify a clear threat model and limits. See `crates/locald-shim/src/bundle.rs`.
- **Self-check runs privileged**: `locald doctor` executes `locald-shim self-check` (setuid). This is expected but not called out in docs. See `crates/locald-core/src/doctor/shim.rs`.
- **Trust store writes**: Root CA installation touches system trust stores; fallback paths are distro-specific. See `crates/locald-shim/src/trust.rs`, `crates/locald-cli/src/commands/trust.rs`.

### Test Engineer
- **Difficult to test in CI**: Privileged ops require setuid shim and host-level cgroup/trust store access; tests rely on sudo in CI. See `crates/locald-shim/tests/`, `.github/workflows/`.
- **No mockable probe layer**: `DoctorReport` reads real host state and invokes shim directly. See `crates/locald-core/src/doctor/mod.rs`.

### SRE
- **Automation needs non-interactive path**: `locald admin setup` requires root (pkexec/sudo). Non‑TTY execution fails unless already privileged. See `crates/locald-cli/src/commands/admin.rs`.
- **Degraded mode exists but not centralized**: Some paths fall back (high ports, skip host sync), but the operational policy is spread across components. See `crates/locald-server/src/proxy/mod.rs`, `crates/locald-server/src/hosts.rs`, `crates/locald-core/src/doctor/mod.rs`.

---

## 4. Recommendations (Prioritized)

**P0 — Documentation correctness**
1. **Fix shim discovery docs** to match sibling/parent-only discovery and remove `LOCALD_SHIM_PATH`/PATH lookup references. Update `docs/manual/security/` and align with `docs/rfcs/0097-strict-shim-discovery.md`.
2. **Update security architecture doc** to match actual shim commands and binding model (no `locald-shim cap`, binding via `bind-port`, CA install via `trust install`). See `docs/manual/security/` and `crates/locald-shim/src/main.rs`.

**P1 — Security & clarity**
3. **Explicitly document privileged self-check** behavior and evidence exposure for `locald doctor`. See `docs/manual/features/doctor.md`.
4. **Clarify trust-store changes** (where CA is stored, how to remove/rotate, and distro-specific tooling). See `docs/manual/features/https.md`, `crates/locald-shim/src/trust.rs`, `crates/locald-server/src/certs.rs`.

**P2 — Testability & operational ergonomics**
5. **Add a probe abstraction** for privileged readiness so tests can simulate missing shim, cgroup, and trust store without sudo. See `crates/locald-core/src/doctor/mod.rs`.
6. **Centralize privilege policy** into a documented matrix in the manual (what requires shim, what degrades, what is optional).

---

## 5. Code References

- `crates/locald-shim/src/lib.rs`
- `crates/locald-shim/src/main.rs`
- `crates/locald-shim/src/bind_port.rs`
- `crates/locald-shim/src/bundle.rs`
- `crates/locald-shim/src/cgroup.rs`
- `crates/locald-shim/src/hosts.rs`
- `crates/locald-shim/src/trust.rs`
- `crates/locald-shim/src/policy.rs`
- `crates/locald-shim/src/self_check.rs`
- `crates/locald-utils/src/privileged.rs`
- `crates/locald-utils/src/shim.rs`
- `crates/locald-core/src/doctor/shim.rs`
- `crates/locald-cli/src/commands/admin.rs`
- `crates/locald-cli/src/commands/trust.rs`
- `docs/rfcs/0096-shim-execution-safety.md`
- `docs/rfcs/0097-strict-shim-discovery.md`
