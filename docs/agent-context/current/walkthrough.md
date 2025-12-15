# Walkthrough - Phase 97: Remove `runc` (Libcontainer “Fat Shim”)

## Step 0: Phase Kickoff

1. **Objective**: Agree on the shim interface for bundle execution and what “done” means.
2. **Output**:
   - One-line command surface for bundle execution (approved): `locald-shim bundle run --bundle <PATH> --id <ID>`
   - A verification matrix (at minimum: e2e suite + “no runc installed”).

## Step 1: Inventory `runc`

1. **Objective**: Find every remaining `runc` invocation and classify it.
2. **Output**:
   - A checklist of call sites (daemon runtime, CNB runtime, examples/tests).
   - A delete/migrate plan for `runc-*` examples.

### Current inventory (as of 2025-12-13)

**Runtime call sites**:

- ✅ Callers have been migrated to the shim bundle interface.
- ✅ The shim implements `locald-shim bundle run --bundle <PATH> --id <ID>`.
- ✅ No runtime code paths should shell out to an external `runc` binary.

**Remaining `runc` references**:

- **Docs**: `locald-docs/` states `runc` is not required; if stale generated docs still mention `runc`, rebuild the docs pipeline (source-of-truth is `locald-docs/src/`, copied into the binary from `locald-docs/dist`).
- **Legacy examples/tests/scripts/CI**: migrated off `runc-*` naming and removed the CI `runc` install step; `verify-oci-example.sh` is the preferred example runner.
- **Historical RFCs/specs**: may mention `runc` as background; keep unless they imply it is still required.

## Step 2: Implement bundle execution in `locald-shim`

1. **Objective**: Execute OCI bundles using embedded `libcontainer`.
2. **Output**:
   - `locald-shim bundle run ...` implemented.
   - Container lifecycle semantics (IDs, state directories, cleanup/delete) defined.
   - Leaf Node constraints preserved.

## Step 3: Migrate callers

1. **Objective**: Make daemon + CNB paths call the new shim interface.
2. **Output**:
   - No code paths shell out to `runc`.

## Step 4: Verification + cleanup

1. **Objective**: Ensure user workflows remain smooth and the system is self-contained.
2. **Output**:
   - e2e suite green.
   - Proof that container execution works without `runc`.
   - Docs updated to remove `runc` as a requirement.

## Step 5: Phase verification + coherence sweep

1. **Objective**: Run the repository verification script and fix any drift or lint failures discovered during the transition.
2. **Output**:
   - ✅ `scripts/agent/verify-phase.sh` passes (build, clippy, dashboard check, IPC verification).
   - ✅ Documentation command surfaces updated to consistently prefer `locald-shim bundle run --bundle <PATH> --id <ID>` (legacy form noted only as back-compat).
   - ✅ Clippy cleanup to keep the verification pipeline green:
     - `locald-utils/src/shim.rs`: doc hygiene + logging; allowlisted the intentionally blocking `sudo` calls.
     - `examples/oci-example/src/main.rs`: switched blocking process spawning to `tokio::process::Command`.
     - `cnb-client/src/lifecycle.rs`: removed `let`+return wrapper flagged by clippy.
     - `locald-server/src/runtime/process.rs`: removed unused `async` wrappers around synchronous helpers.
