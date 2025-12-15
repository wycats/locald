# Task List - Phase 76: Ephemeral Containers

## 1. OCI Library Enhancement (locald-oci)

- [x] **Spec Generation**: Implement `locald_oci::runtime_spec::generate(image_config) -> Spec`.
  - [x] Map `Env`, `Entrypoint`, `Cmd`, `WorkingDir`.
  - [x] Handle basic User/Group mappings.
- [x] **Runtime Interface**: Define `locald_oci::runtime` traits (superseded by direct shim integration).

## 2. Shim Integration (locald-shim)

- [x] **Refactor (RFC 0098)**: Implement "Fat Shim" architecture.
  - [x] Add `libcontainer` dependency.
  - [x] Implement `debug bootstrap` (prototype run command) to execute OCI bundles.
- [x] **Verification**: Verify shim can execute a generated OCI bundle (E2E test).

## 3. Server Orchestration (locald-server)

- [x] **ContainerService**: Create a service to manage ephemeral container lifecycles.
- [x] **Pipeline**: Implement Pull -> Unpack -> Generate Spec -> Run pipeline.

## 4. CLI Implementation (locald-cli)

- [x] **Command**: Add `locald container run <image> [command]` subcommand.
- [x] **Integration**: Wire up to `locald-server` API.

## 5. Verification

- [x] **E2E Test**: Verify `locald container run alpine echo hello` works.
