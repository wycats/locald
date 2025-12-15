# Task List - Phase 97: Libcontainer Shim (Fat Shim)

## 1. Shim Implementation (Runtime)

- [x] **CLI Update**: Modify `locald-shim` to accept a `bundle_path` argument (replacing `debug bootstrap`).
- [x] **Libcontainer Integration**: Implement logic to load `config.json` from `bundle_path` and call `libcontainer::Container::new`.
- [x] **Error Handling**: Ensure proper error reporting back to the caller.

## 2. Daemon Integration (Caller)

- [x] **OCI Spec Generation**: Update `locald-server` (or `locald-oci`) to generate a full OCI Bundle (config.json + rootfs) for services.
- [x] **Shim Invocation**: Update `ProcessManager` to call the new `locald-shim` command with the bundle path.
- [x] **Cleanup**: Ensure temporary bundles are cleaned up after execution.

## 3. Verification

- [x] **Unit Test**: Verify `locald-shim` can run a simple "hello world" bundle.
- [x] **E2E Test**: Verify `locald run` works end-to-end with the new shim.
- [x] **Regression Check**: Ensure existing `exec` services still work (if they use the shim).
