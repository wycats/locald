# Phase 58.6 — DDD Audit: Doctor Command

Date: 2026-01-28  
Scope: `locald doctor` command

## Check Inventory (what actually runs)

**Core report checks (DoctorReport):**

1. **Container environment unsupported** — `container.unsupported` (critical fail) if a container is detected.  
   `crates/locald-core/src/doctor/mod.rs`

2. **Cgroup v2 available** — `cgroup.v2_unavailable` (critical fail) when `/sys/fs/cgroup` is missing.  
   `crates/locald-core/src/doctor/mod.rs`

3. **Cgroup root established** — `cgroup.root_missing` (critical fail) when expected root is missing.  
   `crates/locald-core/src/doctor/mod.rs`

4. **Shim present** — `shim.missing` (critical fail) if no `locald-shim` found.  
   `crates/locald-core/src/doctor/shim.rs`

5. **Shim permissions** — `shim.permissions` (critical fail) if shim isn't root-owned + setuid.  
   `crates/locald-core/src/doctor/shim.rs`

6. **Shim integrity or version** — `shim.integrity` or `shim.version_mismatch` (critical fail) when embedded bytes/version mismatch.  
   `crates/locald-core/src/doctor/shim.rs`  
   `crates/locald-core/src/version.rs`

7. **Shim usability self-check** — `shim.unusable` (critical fail) if shim self-test fails.  
   `crates/locald-core/src/doctor/shim.rs`

8. **HTTPS Root CA presence** — `https.root_ca` (warning fail) if Root CA cert/key missing.  
   `crates/locald-core/src/doctor/mod.rs`

9. **KVM integration (verbose only)** — `kvm.unavailable` (info skip or warning fail) if `/dev/kvm` missing/unusable; only added when `--verbose`.  
   `crates/locald-core/src/doctor/mod.rs`

10. **Non-Linux stub** — `platform.unsupported` (warning skip) on non-Linux.  
    `crates/locald-core/src/doctor/mod.rs`

**CLI-only optional integrations (human output only, not in JSON):**

- **Buildpacks (CNB)** availability via privileged shim.
- **KVM** availability summary.  
  `crates/locald-cli/src/commands/doctor.rs`

**Exit status and JSON output:**

- Exit code is `0` unless critical failures exist (returns `1` on any critical fail).  
  `crates/locald-cli/src/commands/doctor.rs`

---

## Friction Points (by persona)

### 1) New Hire

- **Docs list checks not implemented.** Manual says "Runtime Basics (daemon reachable, sandbox mode)" but there is no such check in code.  
  `docs/manual/features/doctor.md`
- **Problem IDs drift from docs.** Docs reference `SHIM_MISSING`, `CGROUP_V2`, `CERTS_MISSING`, `ROOT_CA`, etc., but code uses `shim.missing`, `cgroup.v2_unavailable`, `https.root_ca`, etc.  
  `docs/manual/features/doctor.md`
- **Docs mention Docker integration availability.** CLI docs imply Docker daemon checks, but the doctor doesn't check Docker at all.
- **JSON schema example is stale.** Example `id` values and summaries don't match real IDs/strings; could mislead newcomers.  
  `docs/manual/features/doctor.md`

### 2) Security Auditor

- **Verbose evidence leaks host details.** `--verbose` can expose shim path, uid/mode, sudo uid, certs directory.  
  `crates/locald-core/src/doctor/shim.rs`
- **Privileged self-check executes setuid shim.** This is expected but should be explicitly documented as privileged execution.

### 3) Test Engineer

- **Checks are hard to simulate in CI.** Many checks depend on real host state (`/sys/fs/cgroup`, `/dev/kvm`, setuid shim, certs directory). No injected test doubles.
- **Optional integrations are only in human output.** No JSON pathway to assert CNB/KVM availability in automation.

### 4) SRE

- **JSON output omits optional integrations.** Only DoctorReport is serialized; CNB/KVM rollups are not present.  
  `crates/locald-cli/src/commands/doctor.rs`
- **No daemon health endpoint for host readiness.** `doctor` is CLI-only; server health is service-focused, not host readiness.
- **No automatic doctor run in `locald up`.** It's only invoked in the explicit command handler.

---

## Recommendations (prioritized)

**P0 — Align docs with reality**

1. **Update problem IDs and check list in docs.** Sync IDs in manual/CLI docs to real `DoctorProblem` values (`container.unsupported`, `cgroup.v2_unavailable`, `cgroup.root_missing`, `shim.missing`, `shim.permissions`, `shim.integrity`, `shim.version_mismatch`, `shim.unusable`, `https.root_ca`, `kvm.unavailable`, `platform.unsupported`).  
   `docs/manual/features/doctor.md`
2. **Remove or implement "Runtime Basics" checks.** Either add daemon reachability/sandbox checks or remove this section from docs.
   `docs/manual/features/doctor.md`

**P1 — Improve machine-readability & SRE UX** 3. **Add optional integrations to JSON schema.** Include CNB/KVM availability in DoctorReport or add a new top-level `integrations` section.

4. **Document JSON schema in docs site.** Only the manual has a schema example, and it is stale.  
   `docs/manual/features/doctor.md`  
   `locald-docs/src/content/docs/`

**P2 — Security clarity** 5. **Explicitly document privileged self-check behavior and evidence exposure.** Make it clear that `--verbose` includes host-sensitive evidence and that the shim self-check runs with elevated privileges.

**P2 — Remediation completeness** 6. **Add actionable remediation for host policy blocks.** `shim.unusable` currently yields no commands. Provide safe guidance (e.g., check mount `nosuid`, SELinux/AppArmor, container policy).

**P3 — Testability** 7. **Introduce a test harness or injectable probe layer.** Allow simulated host probes for cgroup, shim, certs, and KVM so CI can verify expected outputs.

---

## Code References (summary)

- Doctor report checks: `crates/locald-core/src/doctor/mod.rs`
- Container unsupported: `crates/locald-core/src/doctor/mod.rs`
- Cgroup checks: `crates/locald-core/src/doctor/mod.rs`
- Shim checks: `crates/locald-core/src/doctor/shim.rs`
- HTTPS Root CA warning: `crates/locald-core/src/doctor/mod.rs`
- KVM verbose check: `crates/locald-core/src/doctor/mod.rs`
- CLI JSON output + exit code: `crates/locald-cli/src/commands/doctor.rs`
- Optional integrations rollup: `crates/locald-cli/src/commands/doctor.rs`
- CLI command definition: `crates/locald-cli/src/commands/doctor.rs`
- Handler only runs doctor on explicit command: `crates/locald-cli/src/main.rs`
- Docs (manual): `docs/manual/features/doctor.md`
- Docs (CLI reference): `docs/manual/cli/`
- Docs (integrations matrix): `docs/manual/integrations/`
