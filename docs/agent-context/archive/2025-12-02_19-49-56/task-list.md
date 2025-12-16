# Phase 15 Task List: Zero-Config SSL & Single Binary

## 1. Merge Binaries & CLI Refactor

- [x] **Refactor Server Crate**
  - [x] Rename `locald-server` crate to `locald-server-lib` (or just expose lib).
  - [x] Move `main.rs` logic into a `run()` function in `lib.rs`.
  - [x] Ensure `locald-cli` can import `locald-server`.
- [x] **Update CLI Structure**
  - [x] Add `server` subcommand group to `locald-cli`.
  - [x] Implement `locald server start` (calls server lib `run()`).
  - [x] Implement `locald server shutdown` (sends IPC shutdown signal).
  - [x] Update `locald up` to spawn `locald server start` in background if not running.
  - [x] Remove `locald-server` binary target from workspace (or keep as thin wrapper if needed, but goal is single binary).

## 2. Ad-Hoc Workflow (`run` & `add`)

- [x] **Implement `locald run <cmd>`**
  - [x] Execute command as child process.
  - [x] Forward signals (Ctrl-C).
  - [x] On exit, prompt user to add to config.
- [x] **Implement `locald add <name> <cmd>`**
  - [x] Parse `locald.toml`.
  - [x] Add new service entry.
  - [x] Write back to file.

## 3. Zero-Config SSL

- [x] **Trust Store Management**
  - [x] Add `rcgen` and `ca_injector` dependencies (replaced `devcert`).
  - [x] Implement `locald trust` command.
  - [x] Generate/Install Root CA if missing.
- [x] **Certificate Generation**
  - [x] Add `rcgen` dependency.
  - [x] Fix compilation errors in `CertManager` (`locald-server/src/cert.rs`).
  - [x] Implement on-the-fly certificate generation for `<project>.localhost`.
- [x] **TLS Termination**
  - [x] Update `Proxy` to use `rustls` (Added `start_https`).
  - [x] Implement `ResolvesServerCert` trait to serve generated certs.
  - [x] Switch default domain to `.localhost`.

## 4. Verification & Cleanup

- [x] **Verify Workflows**
  - [x] `locald up` (starts daemon).
  - [x] `locald run` -> Add -> `locald up`.
  - [x] `locald server shutdown`.
- [x] **Verify SSL**
  - [x] `locald trust` (Verified generation, installation needs sudo).
  - [x] `curl https://app.localhost`.
  - [x] Browser check.
- [x] **Dogfooding Improvements**
  - [x] Implement seamless update: `locald up` restarts daemon if version changed.
  - [x] Add build timestamp to version for dev builds.
  - [x] Fix `locald run` Ctrl+C handling (graceful exit + prompt).
- [x] **Cleanup**
  - [x] Remove old `locald-server` binary artifacts.
  - [x] Update documentation (CLI reference).
