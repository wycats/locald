# Task List - Phase 97: Remove `runc` (Libcontainer “Fat Shim”)

## 0. Planning / Review

- [x] **Confirm shim interface** (Option A: `bundle run` only; rely on existing privileged cleanup/state conventions)
- [x] **Define success matrix** (Linux dev flow + sandboxed e2e; prove `runc` is not required)

**Success matrix (current)**:

- [x] `locald --sandbox=<name> container run alpine echo hello` works after `sudo locald admin setup`.
- [x] `cargo test -p locald-cli --test e2e` passes.
- [x] `cargo test -p locald-cli --test tui_test` passes.
- [x] E2E asserts stderr does not mention `runc`.

## 1. Inventory

- [x] **Enumerate `runc` call sites** (daemon, `cnb-client`, examples, tests)
- [x] **Classify usage**: runtime dependency vs docs/examples vs legacy compatibility

**Current status**:

- **Runtime**: no remaining code paths should shell out to an external `runc` binary.
- **Docs**: `locald-docs/` documents that `runc` is not required; rebuild generated docs artifacts if they still contain stale `runc` wording.
- **Legacy**: some RFCs/specs intentionally discuss `runc` historically; leave as-is unless they claim it is still required.

## 2. `locald-shim`: `libcontainer` runtime

- [x] **Add libcontainer dependency** with conservative feature flags
- [x] **Implement bundle run** (`bundle run --bundle <path> --id <id>`)
- [x] **Bundle cleanup semantics** (Option A: no `bundle delete` subcommand; rely on state-dir conventions + existing privileged cleanup)
- [x] **Harden error reporting** (actionable caller errors, preserve backtraces in debug)

## 3. Caller migrations

- [x] **Daemon/runtime**: replace `runc` invocation path with new shim bundle path
- [x] **CNB runtime (`cnb-client`)**: replace `runc` invocation path with new shim bundle path
- [x] **Remove/retire legacy helpers** (`spawn_runc_process`, `ShimRuntime::*runc*`)

## 4. Verification

- [x] **E2E suite green**
- [x] **Regression**: host exec services unaffected (no behavior change expected; existing exec-based examples continue to work)
- [x] **No-`runc` test**: prove container execution works when `runc` is missing

Notes:

- The key invariant is: container execution does not shell out to `runc` and does not require `runc` to be installed.

## 5. Cleanup

- [x] **Remove `runc` examples/tests** or migrate them to libcontainer terminology
- [x] **Docs refresh**: update any references implying `runc` is required
