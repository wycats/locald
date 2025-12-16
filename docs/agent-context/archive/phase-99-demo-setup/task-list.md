# Task List - Phase 99: Demo Repository Setup

## 1. Color System (Design System)

- [x] **Scaffold**: Create `examples/color-system`.
- [x] **Config**: Create `locald.toml` with:
  - `web` (Frontend): Static site or simple server.
  - `api` (Backend): Simple API returning colors.
  - `db` (Postgres): Database for colors.
- [x] **Implementation**: Create minimal dummy implementations (e.g., Python `http.server` or simple Rust binaries).

## 2. Hydra (Identity Provider)

- [x] **Scaffold**: Create `examples/hydra`.
- [x] **Config**: Create `locald.toml` with:
  - `hydra` (Main Service): The identity provider.
  - `consent` (Consent App): UI for login/consent.
  - `db` (Postgres): Database for Hydra.
- [x] **Implementation**: Mock implementations or use real binaries if easy (but mocks are safer for demo stability).

## 3. Verification

- [x] **Boot**: Verify `locald up` works in both directories.
- [x] **Routing**: Verify `color-system.localhost` and `hydra.localhost` resolve.
- [x] **Inter-Service**: Verify they can talk to each other (if needed).

## 4. Dashboard Polish (Phase 99b)

- [x] **Research**: Look for an RFC describing the "building" dashboard screen and verify log visibility.
- [x] **Feature**: Implement "High-Fidelity Build Logs" on the loading screen (RFC 0093).
- [x] **Visual Check**: Verify the dashboard looks perfect with these services running.
