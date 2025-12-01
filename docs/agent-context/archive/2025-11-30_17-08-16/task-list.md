# Phase 5 Task List

- [ ] **Infrastructure: Log Broadcasting**
  - [ ] Add `tokio::sync::broadcast` to `ProcessManager`.
  - [ ] Implement `LogBuffer` (Ring Buffer).
  - [ ] Wire up stdout/stderr capture to the broadcaster.

- [ ] **Backend: Internal API & WebSockets**
  - [ ] Decide on Router (Axum vs Raw Hyper) for `locald.local`. (Recommendation: Axum for ergonomics).
  - [ ] Implement `GET /api/state`.
  - [ ] Implement `GET /api/logs` (WebSocket).
  - [ ] Integrate `rust-embed` for static assets.

- [ ] **Frontend: Dashboard**
  - [ ] Create basic HTML/CSS structure.
  - [ ] Implement `ConnectionManager` (Reconnection logic, status UI).
  - [ ] Implement `ServiceList` component.
  - [ ] Implement `LogViewer` component.
  - [ ] Add Start/Stop/Restart controls.

- [ ] **CLI: Logs Command**
  - [ ] Implement `locald logs` subcommand.
  - [ ] Stream logs to terminal.

- [ ] **Verification**
  - [ ] Verify UI robustness (disconnect/reconnect).
  - [ ] Verify log streaming performance.
