---
title: Minimum AWESOME Product (MAP): scope, quarantine, and enforcement
stage: 0
feature: Shipping
exo:
    tool: exo rfc create
    protocol: 1
---

# RFC 0116: Minimum AWESOME Product (MAP): scope, quarantine, and enforcement

## 1. Summary

MAP defines what locald can **ship and support with high confidence**. It is the core value proposition we can stand behind on day one:

> **clone → `locald up` → stable `*.localhost` HTTPS → dashboard + logs + basic control → keep things up**

This RFC establishes the scope boundary, quarantine rules for experimental features, and enforcement mechanisms to keep the announcement story coherent.

## 2. The Pitch

**One-sentence:**
> locald manages all of your local development environments, giving them local domains and SSL certificates automatically, booting them and keeping them up, and giving you a single CLI and Dashboard to manage them all. It's like Heroku for local development.

**The "instead of" framing:**
> Instead of running `pnpm start` or `docker-compose up`, you run `locald try pnpm start --port '$PORT'` and it Just Works™ with a local domain and SSL cert. You get logs for free, and you don't need to worry about port conflicts or keeping the service up.

## 3. Platform Promise

MAP commits to these platforms:

| Platform | Support Level | Notes |
|----------|---------------|-------|
| **Linux (Debian)** | ✅ Full | Latest stable + latest LTS |
| **Linux (Fedora)** | ✅ Full | Latest stable + latest LTS |
| **Linux (Ubuntu)** | ✅ Full | Latest stable + latest LTS |
| **macOS** | ✅ Full | Latest 2 major versions |
| **WSL** | ✅ Full | Same as Linux (locald installed in WSL, not Windows natively) |
| **Windows Native** | ❌ Out | Not in MAP scope |

**"Working" means:**
- Domain names resolve correctly from where the user's browser runs
- SSL certificates are trusted by the browser
- `locald try` and `locald up` complete the happy path

## 4. The Day 1 Story

### 4.1 Hello World: `locald try`

`locald try` is the canonical "hello world" workflow—not just for announcement, but for the long term. It demonstrates the value prop most clearly:

```bash
# Zero config, immediate value
locald try pnpm start --port '$PORT'
```

**What happens:**
1. locald assigns a stable local domain (`<dirname>.localhost`)
2. locald provisions a trusted SSL certificate
3. locald starts the command with `$PORT` bound
4. User opens `https://<dirname>.localhost` in browser
5. It Just Works™

### 4.2 Persistent Projects: `locald up`

For projects with `locald.toml`:

```bash
# In a project directory with locald.toml
locald up
```

**What the user gets:**
- Stable domain (`project.localhost`)
- HTTPS by default
- Dashboard access to logs and controls
- Process stays up (daemon-managed)

### 4.3 The Dashboard

The Dashboard is **almost more important than the CLI** for the announcement story. It shows the value prop of managing multiple local environments more clearly than CLI output.

**Dashboard MAP scope:**
- View all running/stopped services
- Start/stop/restart controls
- Live log streaming
- Service health status

## 5. Scope Boundary

### 5.1 IN (Core MAP)

Features that are **definitely in** for announcement:

| Feature | Rationale |
|---------|-----------|
| `locald try` | The hello world, zero-config entry point |
| `locald up` / `locald down` | Core lifecycle management |
| Local domains (`*.localhost`) | Core value prop |
| Automatic HTTPS/TLS | Core value prop |
| Dashboard (view, start/stop, logs) | Primary management interface |
| CLI (up, down, status, logs, try) | Essential commands |
| Daemon-managed persistence | "Keep things up" promise |
| Multi-service support | Real-world apps have multiple processes |
| `${services.*}` interpolation | Service-to-service communication |
| Privileged shim (sudo setup) | Required for port 443 and certs |

### 5.2 OUT (Not in MAP)

Features explicitly **out of scope** for announcement:

| Feature | Status | Rationale |
|---------|--------|-----------|
| Windows native | Not supported | WSL is the path |
| CNB/Buildpacks | Experimental | Tease as future, not core |
| Container runtime | Experimental | Docker for dependencies only, not apps |
| WASM plugins | Experimental | Infrastructure exists but not polished |
| `locald ai` | Proposal | Not implemented |
| Ephemeral environments | Experimental | Not fully baked |
| Hot reloading config | Not implemented | Nice-to-have, not core |

### 5.3 Quarantine Rules

Experimental features MUST be:
1. **Hidden from default CLI help** (require `--experimental` or hidden subcommands)
2. **Marked in docs** with experimental banners
3. **Excluded from announcement messaging**
4. **Tested but not promised** (may break between releases)

## 6. Vocabulary (Must Resolve Before Announcement)

These terms need consistent definition across CLI, Dashboard, and docs:

| Concept | Current State | Resolution Needed |
|---------|---------------|-------------------|
| **pin** vs **monitor** | Inconsistent | Define: "keep running always" vs "track but don't auto-start" |
| **disable** | Exists | Clarify relationship to pin/monitor |
| **project** vs **service** | Mostly clear | Ensure consistency |
| **up** vs **start** | Clear | `up` is the public verb |

**Core vocabulary promise:**
> "It stays up and you don't have to think about it"

## 7. Shippable Core Criteria

### 7.1 Rules for IN

A feature is IN for MAP if it:
- ✅ Has tests
- ✅ Has documentation
- ✅ Is proven by real daily usage
- ✅ Has graceful error paths

### 7.2 Rules for OUT

A feature is OUT (quarantined) if it:
- ❌ Is experimental or unfinished
- ❌ Missing tests or docs
- ❌ Requires special flags or awkward setup
- ❌ Has unclear recovery/error paths

## 8. Enforcement Mechanisms

### 8.1 CLI Surface

- Default `--help` shows only MAP commands
- Experimental commands require `locald experimental <cmd>` or are unlisted
- Error messages reference only MAP workflows

### 8.2 Documentation

- Main docs cover MAP features only
- "Roadmap" section teases future direction
- "Experimental" section documents quarantined features
- No experimental features in getting started guides

### 8.3 Dashboard

- Default views show MAP functionality
- Experimental features behind feature flags or hidden routes
- No UI for unshipped features

### 8.4 CI/Testing

- MAP features have e2e coverage
- Experimental features may have unit tests but not e2e requirements
- Release checklist validates MAP scope

## 9. Post-MAP Themes

Once MAP is stable, these are candidates for the next release theme (see RFC 0124):

1. **Front End Apps** — HMR, WebSockets, FE↔BE ergonomics
2. **Managed Services v1** — Redis, MinIO, better service lifecycle
3. **Multi-project platform UX** — Project registry, pinning, cross-project dashboard
4. **Reliability & Recovery** — Crash recovery, restore semantics
5. **Observability Workspace** — Log search, event timelines
6. **Install/privilege smoothing** — Better `doctor`, self-healing setup

## 10. Acceptance Criteria

MAP is "done" when:

1. ✅ `locald try <cmd>` works on all supported platforms
2. ✅ `locald up` with `locald.toml` works on all supported platforms
3. ✅ HTTPS works in browser without certificate warnings
4. ✅ Dashboard shows running services with logs
5. ✅ Vocabulary is consistent across CLI, Dashboard, docs
6. ✅ Experimental features are quarantined (hidden/marked)
7. ✅ Installation path is documented and tested
8. ✅ `locald doctor` can diagnose and remediate common issues

## 11. Open Questions

1. **Postgres data location/reset semantics** — What's the default? How do users reset?
2. **GC/cleanup behavior** — When does locald remove state? How does user trigger cleanup?
3. **WSL domain resolution** — Does `*.localhost` resolve from Windows browser into WSL?

## 12. References

- RFC 0124: Post-MAP Release Themes
- RFC 0135: Dashboard Vocabulary
- RFC 0138: macOS Domain and Certificate Setup
- `docs/research/review/plan.md`: Ship-Readiness Review Plan
- `docs/research/review/questions.md`: Pre-Launch Q&A