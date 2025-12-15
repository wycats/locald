# Phase 24: Dashboard Ergonomics

## 1. Dashboard Alias (Prerequisite)

- [x] Implement `locald.localhost` routing in `proxy.rs`
- [x] Add regression test in `proxy_test.rs`
- [x] Ensure fallback logic supports both running service (dev) and embedded assets (prod)

## 2. Global Controls

- [x] Add "Stop All" button to dashboard header
- [x] Add "Restart All" button to dashboard header
- [x] Implement backend handlers for global actions in `manager.rs` and `ipc.rs`

## 3. Event Stream

- [ ] Create `Event` struct for dashboard updates
- [ ] Implement SSE (Server-Sent Events) endpoint in `locald-server`
- [ ] Update dashboard frontend to consume SSE and display event log

## 4. Visual Polish

- [ ] Improve service list layout
- [ ] Add status indicators (green/red dots)
- [ ] Ensure mobile responsiveness
