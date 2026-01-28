# Phase 58.4 — DDD Audit: Health Checks & Service URLs

Date: 2026-01-28  
Scope: Health checks, URL/domain handling, health gates, CLI/dashboard integration.

---

## 1. Health Check Overview

### Supported Types (Runtime + Config)

- Config supports `health_check` as either a command string or a structured probe (`http`, `tcp`, `command`). See `crates/locald-core/src/config/health.rs`.
- Runtime health checks are implemented in `crates/locald-server/src/health.rs` and support:
  - HTTP probe (`HealthProbeKind::Http`)
  - TCP probe (`HealthProbeKind::Tcp`)
  - Command probe (`HealthProbeKind::Command` or string shorthand)
  - Default: if no `health_check` and a port exists, `tcp` probe is used. See `crates/locald-server/src/health.rs`.

### `health_check` Configuration

- `health_check` can be a table or string; docs specify `type`, `path`, `interval`, `timeout`, `retries`. See `docs/manual/reference/configuration.md`.
- Example usage appears in `examples/health-check-test/locald.toml`.

### Default Timeouts / Intervals (Actual)

- Health monitors poll every 250ms and stop once healthy. See `crates/locald-server/src/health.rs`.
- HTTP probes use a fixed 2s timeout; TCP and command probes have no explicit timeouts. See `crates/locald-server/src/health.rs`.
- Startup health gate waits up to 30s. See `crates/locald-server/src/manager.rs`.

### Notify Readiness (sd_notify)

- A `NotifyHealth` is created and `READY=1` marks health as healthy (`HealthStatus::Healthy`). See `crates/locald-server/src/notify.rs` and `crates/locald-server/src/health.rs`, `crates/locald-server/src/controller/exec.rs`.
- Note: The notify path updates internal health state but does not emit a `HealthUpdated` event.

### Documentation vs Runtime (Mismatch)

- Docs describe Docker healthcheck + `sd_notify` hierarchy; runtime currently implements HTTP/TCP/Command + `sd_notify` but no Docker healthcheck polling. See `docs/manual/features/health-checks.md` and `docs/manual/reference/configuration.md` vs `crates/locald-server/src/health.rs`.

### Propagation to CLI/Dashboard

- Health updates flow through `ProjectEvent::HealthUpdated` events and API `ServiceStatus`. See `crates/locald-server/src/events.rs` and `crates/locald-server/src/manager.rs`.
- Dashboard displays `HealthStatus` in the sidebar indicator. See `locald-dashboard/src/lib/components/Sidebar.svelte`.
- CLI `status` and `ps` currently display state, port, URL, warnings, but not health. See `crates/locald-cli/src/commands/ps.rs` and `crates/locald-cli/src/commands/status.rs`.

---

## 2. URL/Domain Overview

### Assignment of `*.localhost`

- Default project domain: `${project.name}.localhost`, configurable with `project.domain`. See `crates/locald-core/src/config/project.rs`.
- Service domain resolution:
  - `web` or service name == project name → root domain
  - Other services → `${service}.${project_domain}`
    See `service_domain()` in `crates/locald-core/src/url.rs`.

### Hosts File Updates

- `/etc/hosts` (or Windows hosts file) is updated with `# BEGIN locald` block. See `crates/locald-utils/src/hosts.rs`.
- Auto-sync is triggered on project start via `HostsSync`. See `crates/locald-server/src/manager.rs` and `crates/locald-server/src/hosts.rs`.
- Docs still describe manual `locald hosts sync`. See `docs/manual/features/domains.md`.

### Reverse Proxy Behavior

- Proxy routes by `Host` header, forwarding to `http://localhost:<port>` for matching service domain. See `crates/locald-server/src/proxy/router.rs`.
- Fallbacks:
  - `locald.localhost` → embedded dashboard
  - `docs.localhost` → embedded docs unless service claims domain
  - `dev.locald.localhost` and `dev.docs.localhost` → Vite/Astro dev servers
    See `crates/locald-server/src/proxy/mod.rs`.

### HTTPS Certificates & Trust

- `CertManager` generates a Root CA (stored in `~/.locald/certs`) and issues per-domain certs on SNI. See `crates/locald-server/src/certs.rs`.
- If cert setup fails, HTTPS is disabled for the proxy. See `crates/locald-server/src/proxy/mod.rs` and `crates/locald-server/src/daemon.rs`.
- Trust is not automatic; docs reference `sudo locald admin setup` but do not detail CA storage/rotation. See `docs/manual/getting-started/installation.md`.

### Port Assignment & Discovery

- Port selection: config `port`, otherwise sticky port if available, else random free port (bind `127.0.0.1:0`). See `crates/locald-server/src/ports.rs`.
- Port mismatch detection scans `/proc` for actual listening ports and adds warnings. See `crates/locald-server/src/ports.rs` and `crates/locald-server/src/controller/exec.rs`.
- Service URL generation prefers HTTPS if proxy HTTPS port is known; otherwise HTTP; defaults to HTTPS even without proxy ports. See `crates/locald-core/src/url.rs`.

---

## 3. Health Gate Behavior

- After each service starts, `HealthMonitor` begins probing and `wait_healthy()` blocks startup of dependent services. See `crates/locald-server/src/health.rs` and `crates/locald-server/src/manager.rs`.
- Timeout: 30s hard-coded; errors include:
  - "Service {name} timed out waiting for health check"
  - "Service {name} stopped unexpectedly during startup"
  - "Service {name} is not running"
    See `crates/locald-server/src/manager.rs`.
- On the proxy side, if a service has no port, a "disabled" HTML response is shown with guidance to start it. See `crates/locald-server/src/proxy/router.rs`.
- Health checks stop once a service is marked healthy; there is no continuous liveness monitoring or transition to unhealthy.

---

## 4. Friction Points by Persona

### New Hire

- Docs present a readiness hierarchy (Docker healthcheck, `sd_notify`, TCP), but runtime lacks Docker healthcheck polling and does not use configured `interval`/`timeout`. See `docs/manual/features/health-checks.md` vs `crates/locald-server/src/health.rs` and `crates/locald-core/src/config/health.rs`.
- Service domain mapping (`web` → root domain) is implemented but not documented in the config reference. See `crates/locald-core/src/url.rs`.

### Security Auditor

- Root CA is auto-generated in `~/.locald/certs` and used for on-demand cert issuance; trust installation is manual and not clearly documented. See `crates/locald-server/src/certs.rs`.
- HTTPS silently disables if cert manager init fails; no user-facing guidance beyond logs. See `crates/locald-server/src/proxy/mod.rs`.

### Test Engineer

- Health probes are time-based loops with hard-coded sleeps and no injectable clock or mock probe layer. See `crates/locald-server/src/health.rs`.
- `TcpProbe` and `CommandProbe` use real shell/network behavior, which complicates unit tests. See `crates/locald-server/src/health.rs`.
- Notify health events do not emit `HealthUpdated`, so UI tests may miss readiness changes. See `crates/locald-server/src/notify.rs`.

### SRE

- Health checks are "readiness-only": once healthy, probes stop and there is no `Unhealthy` transition or periodic liveness. See `crates/locald-server/src/health.rs`.
- No structured metrics/alerts for health beyond internal SSE; `HealthUpdated` events exist but are UI-only. See `crates/locald-server/src/events.rs`.

---

## 5. Recommendations (Priority-Ordered)

1. **Make runtime match docs**
   - Either implement Docker healthcheck polling or remove it from docs.
   - Apply `interval`/`timeout` from `health_check` config.  
     References: `crates/locald-server/src/health.rs`, `crates/locald-core/src/config/health.rs`, `docs/manual/features/health-checks.md`.

2. **Emit `HealthUpdated` on notify readiness**
   - Ensure `NotifyHealth` triggers `HealthUpdated` so dashboard/CLI reflect readiness immediately.  
     References: `crates/locald-server/src/notify.rs`, `crates/locald-server/src/events.rs`.

3. **Document domain mapping rules & auto-host sync**
   - Add explicit section for `web`/project-name root domain behavior.
   - Clarify that hosts sync is attempted automatically, but can fail without shim.  
     References: `crates/locald-core/src/url.rs`, `docs/manual/features/domains.md`, `crates/locald-server/src/hosts.rs`.

4. **Add liveness monitoring or degrade transitions**
   - Optional periodic probes after healthy, or a "stale" health state.  
     References: `crates/locald-server/src/health.rs`.

5. **Expose health state in CLI status/monitor**
   - Show `Healthy`/`Unhealthy` in CLI output for parity with dashboard.  
     References: `crates/locald-cli/src/commands/ps.rs`, `crates/locald-cli/src/commands/status.rs`.

---

## 6. Code References

- Health config schema: `crates/locald-core/src/config/health.rs`
- Health monitoring runtime: `crates/locald-server/src/health.rs`
- Health probe implementation: `crates/locald-server/src/health.rs`
- Notify readiness: `crates/locald-server/src/notify.rs`, `crates/locald-server/src/controller/exec.rs`
- Health gate timeout: `crates/locald-server/src/manager.rs`
- Domain mapping and URL building: `crates/locald-core/src/url.rs`
- Reverse proxy routing and fallbacks: `crates/locald-server/src/proxy/router.rs`, `crates/locald-server/src/proxy/mod.rs`
- Disabled/Loading proxy responses: `crates/locald-server/src/proxy/router.rs`
- Hosts file update logic: `crates/locald-utils/src/hosts.rs`, `crates/locald-server/src/hosts.rs`
- Auto host sync trigger: `crates/locald-server/src/manager.rs`
- Cert generation and CA storage: `crates/locald-server/src/certs.rs`
- HTTPS proxy bind: `crates/locald-server/src/proxy/mod.rs`
- Dashboard health UI & SSE: `locald-dashboard/src/lib/components/Sidebar.svelte`, `locald-dashboard/src/routes/api/`
- CLI status/monitor output: `crates/locald-cli/src/commands/ps.rs`, `crates/locald-cli/src/commands/status.rs`
- Docs: `docs/manual/features/health-checks.md`, `docs/manual/features/domains.md`, `docs/manual/reference/configuration.md`
