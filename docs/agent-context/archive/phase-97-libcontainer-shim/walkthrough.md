# Walkthrough - Phase 97: Libcontainer Shim (Fat Shim)

**Goal**: Replace `runc` with embedded `libcontainer`.

## Changes

### Diagnosis

- Identified that `runc` dependency creates distribution friction and "rootless" execution is fragile on many systems.
- RFC 0098 proposes embedding `libcontainer` into `locald-shim`.

### Implementation

- (Pending) Refactor `locald-shim` to use `libcontainer`.
- (Pending) Update `locald-server` to generate OCI bundles.

### Verification

- (Pending) Verify end-to-end execution.
