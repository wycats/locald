---
title: Post-MAP Release Themes: Front End Apps
stage: 0
feature: Shipping
exo:
    tool: exo rfc create
    protocol: 1
---


# RFC 0124: Post-MAP Release Themes: Front End Apps


## 1. Summary
This RFC proposes a **post-MAP release theme**: **Front End Apps**.

MAP defines what we can ship and support with high confidence (clone → `locald up` → stable `*.localhost` HTTPS → dashboard + logs + basic control → keep things up). Once MAP is stable, the next best “feel it immediately” value is: **make frontend development seamless and production-like**.

This RFC also captures **alternative post-MAP themes** we could choose instead (or sequence later) once we’re ready to implement beyond MAP.

## 2. Motivation
MAP gets us to “it runs” with coherent vocabulary and a shippable support stance. But a huge fraction of developers experience local platforms primarily through frontend work:

- HMR, WebSockets, and dev servers
- secure cookies / auth flows
- browser constraints (HTTPS, mixed content, CORS)
- “same-origin vs cross-origin” ergonomics

If locald makes these flows effortless, it becomes evangelizable even for people who don’t (yet) care about managed DBs, containers, or deeper infrastructure.

## 3. Scope framing
This is a **theme RFC**, not an implementation spec. It is intended to:

- define the user-visible outcomes we want to promise next
- identify what must be true in locald’s proxy/dashboard/CLI surfaces
- provide a decision framework and alternatives

It should not expand MAP itself.

## 4. Primary theme: Front End Apps
### 4.1 Persona focus
Primary: **App Builder (Regular Joe)**.
Secondary: **Power User** (only insofar as it supports FE ergonomics).

### 4.2 The “frontend happy loop” we want
1) `locald up`
2) open `https://<project>.localhost`
3) frontend dev server boots; HMR works
4) frontend can call backend “the way production does” (or we make the tradeoff explicit)
5) auth flows work (secure cookies)
6) failures are legible (loading/building state, logs)

### 4.3 What we should be able to promise
This is the minimal “Front End Apps” contract that feels like a platform:

1) **WebSockets/HMR reliability**
    - WebSockets work end-to-end through the proxy.
    - Documentation and defaults are aligned for common frameworks.

2) **Browser-grade HTTPS as the default reality**
    - The default story assumes HTTPS (`*.localhost`) so service workers, secure cookies, and modern browser APIs behave like production.

3) **FE↔BE ergonomics (choose and commit to a stance)**
    - Today: multi-service and `${services.*}` wiring exist, but FE↔BE “just works” depends on app-level CORS or framework proxies.
    - Post-MAP we should choose a first-class stance:
      - **Same-origin by default (preferred when it matches production)**: `https://<project>.localhost/api/*` routes to the backend service via proxy routing.
      - **Cross-origin by default**: frontend and backend live on different subdomains and users configure CORS or dev-server proxies.

### 4.6 Production parity via provider configs (no rewrite DSL)
To follow the spirit of Factor 10 (“dev/prod parity”), the FE↔BE stance and route shapes should be derived from the user’s **deployment provider configuration**, not from a half-specified routing DSL in `locald.toml`.

Approach
- Implement first-class support for providers (initially Vercel + Netlify).
- Parse the provider configs into a small internal **Rust struct** (a “deployment profile”) that captures only what locald needs to mirror production:
    - the public URL shape (single origin vs split)
    - the production path mapping that must be mirrored locally (e.g. `/api/*` → backend)
    - any provider-specific function prefixes (e.g. Netlify functions)

Only after the struct and semantics are stable should we consider reifying this shape as an explicit user configuration surface.

Minimal override
- Provide one knob to resolve ambiguity without creating an options army:
    - `project.deploy = "auto" | "vercel" | "netlify" | "custom"`
- In `auto`, locald detects the governing provider config files.

Drift (configs change over time)
- If a repo changes providers (e.g. `netlify.toml` removed, `vercel.json` added), locald should adapt.
- Detection should be deterministic and evidence-based; when the inferred profile changes, surface a durable “deployment profile changed” notice (dashboard + CLI status), not an interactive prompt storm.

Pinned apps / autostart
- Since pinned apps may be started automatically, detection must be daemon-resident and cheap.
- Store a **profile snapshot** in the project registry, including evidence and a lightweight fingerprint of the governing files.
- On autostart, revalidate via fingerprints; recompute only when the governing inputs changed.

4) **Fast, legible “building/loading” UX**
    - When the dev server accepts a connection but is still compiling, the user sees a coherent loading/building state rather than confusing proxy failures.
    - Ideally: the loading state can surface the right logs.

### 4.4 Explicit non-goals for this theme
- Designing a production deployment story end-to-end.
- Adding a new runtime (containers/VMM) as part of “frontend apps”.
- Solving framework-specific CORS for users (we can offer guidance and good defaults, but the app owns its policies).
- Building a “magic” SPA router or rewriting paths in ways that make development diverge from production.

### 4.5 What “done” looks like (acceptance checks)
Define 1–2 canonical fixtures (or evolve existing examples) and verify:

- `locald up` → can load the frontend over HTTPS
- HMR works through the proxy (WebSocket upgrade path)
- FE can reach BE in the chosen stance (same-origin routing or documented cross-origin/CORS story)
- failures show a building/loading state and point to logs

## 5. Related work already in the repo (signals)
This theme is not starting from zero:

- Docs already teach frontend patterns and HMR notes.
- Proxy “loading state” work exists as an RFC and implementation direction.
- `${services.*}` interpolation exists and is tested.

The theme is about turning these into a crisp, reliable, evangelizable loop.

## 6. Alternatives to consider post-MAP
These are intentionally framed as **coherent release themes** we can pick up once MAP is stable.

### A) Managed Services v1 (beyond Postgres)
What it is: Redis/MinIO/etc, better `service add/reset` UX, strong data-dir lifecycle story.
Why it’s attractive: huge persona fit (App Builder), strong differentiation.
Why it might wait: it expands support surface area quickly; needs reliability + upgrade story.

### B) Multi-project platform UX (“my machine as a platform”)
What it is: project registry, pinned projects, easy switching, clearer “what is running” across projects.
Why it’s attractive: operationalizes “multiple apps per machine” as a product experience.
Why it might wait: demands a lot of cross-surface product design (dashboard + CLI + daemon behavior).

### C) Reliability & Recovery as a product promise
What it is: sharpen restore semantics, fewer ghost states, tighter stop/restart guarantees, crash recovery polish.
Why it’s attractive: concentrates effort on trust and “keep things up”.
Why it might wait: less “marketing-visible” than frontend ergonomics, even if it’s crucial.

### D) Observability Workspace (dashboard depth)
What it is: log search/retention UX, event timelines, status history, tighter CLI↔dashboard parity.
Why it’s attractive: makes locald feel like a real platform.
Why it might wait: can become a broad UI project if not tightly scoped.

### E) Installation + privilege/trust smoothing
What it is: make the privileged setup path smoother and more self-healing; strengthen `doctor` remediation.
Why it’s attractive: reduces onboarding friction dramatically.
Why it might wait: risks becoming OS-specific churn unless we pin a minimal contract.

## 7. Decision criteria (when we choose the post-MAP theme)
When MAP is green, pick the next theme by asking:

- Which theme most increases “time to first joy” for the App Builder?
- Which theme has a crisp support stance (small taught surface area)?
- Which theme has 1–2 acceptance scenarios we can enforce in CI?
- Which theme best leverages what is already implemented (fast win)?

## 8. Theme selection checklist (acceptance scenarios)
This is the “make it real” rubric. A theme is ready to execute when it has **2–3 canonical scenarios** that:

- can be run deterministically (fixtures/examples/e2e)
- can be checked automatically (CI)
- fail with actionable output

### 8.1 Front End Apps (primary)
Acceptance scenarios:

1) **HMR through proxy**
    - Fixture: a Vite (or equivalent) app with HMR.
    - Check: `locald up` then assert WebSocket upgrade works and HMR client connects successfully.

2) **HTTPS-first browser reality**
    - Check: frontend loads at `https://<project>.localhost` with a trusted cert (assumes `locald trust` and privileged setup are done).

3) **FE↔BE stance check (pick one)**
    - Same-origin option: `https://<project>.localhost/api/health` routes to backend and returns 200.
    - Cross-origin option: docs + fixture show CORS configured (or dev-server proxy) and the frontend can fetch a backend endpoint successfully.

### 8.2 Managed Services v1 (alternative)
Acceptance scenarios:

1) **Provision + wire Postgres**
    - Fixture: app service consumes `${services.db.url}`.
    - Check: `locald up` then app can connect and run a simple query/migration.

2) **Reset semantics**
    - Check: `locald service reset db` wipes state and service comes back cleanly.

3) **Second managed service (if added)**
    - Check: Redis/MinIO/etc has an equivalent “it runs + it wires” smoke test.

### 8.3 Multi-project platform UX (alternative)
Acceptance scenarios:

1) **Two projects, no collisions**
    - Check: bring up two different project directories; both serve stable, distinct `*.localhost` origins.

2) **Project registry visibility**
    - Check: a stable command/UI can list known projects and identify what’s running.

3) **Pin/restore (if included)**
    - Check: pinned project restores to “up” after daemon restart (or an explicit documented restore trigger).

### 8.4 Reliability & Recovery (alternative)
Acceptance scenarios:

1) **Daemon restart recovery**
    - Check: start services, restart daemon, services return to expected state and logs remain available.

2) **Crash legibility**
    - Check: intentionally crash a service and assert the user sees a coherent “what happened + what to do next” surface (CLI/dashboard).

### 8.5 Observability Workspace (alternative)
Acceptance scenarios:

1) **Log retention + retrieval**
    - Check: logs survive daemon restart and are accessible via `locald logs` and dashboard.

2) **Event timeline (if added)**
    - Check: a small set of lifecycle events is visible (start/build/ready/crash/restart) for one fixture.

### 8.6 Install / privilege / trust smoothing (alternative)
Acceptance scenarios:

1) **Doctor → remediation loop**
    - Check: on an unprepared machine profile (or simulated), `locald doctor` points to a single, correct remediation path.

2) **Trust setup**
    - Check: `locald trust` succeeds and enables HTTPS for `*.localhost` in a repeatable way.

## 9. Open questions
1) Which FE↔BE stance do we want to evangelize first: same-origin routing or cross-origin + explicit CORS guidance?
2) Which frameworks are first-class for docs/adapters (Vite/Next/Remix/etc)?
3) Do we treat “loading state shows logs” as part of this theme or as observability-workspace follow-up?
