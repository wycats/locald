# Task List - Phase 23: Advanced Service Configuration

- [x] **Service Types**: Support `type = "worker"` for non-network services (skip port assignment/probes).
- [x] **Procfile Support**: Parse `Procfile` for drop-in compatibility; auto-generate config if missing.
- [x] **Port Discovery**: Auto-detect ports for services that ignore `$PORT` (scan `/proc/net/tcp` or `lsof`).
- [ ] **Advanced Health Checks**: Add HTTP probe and Command execution health checks (e.g., `check_command` for DB readiness).
- [ ] **Foreman Parity**: Support `.env` file loading, signal handling customization, and concurrency control.
