# Implementation Plan - Phase 99: Demo Repository Setup

**Goal**: Create the "Perfect Demo" environment by setting up two realistic-looking projects: `color-system` and `hydra`. These will be used to demonstrate `locald`'s capabilities during the pitch.

## 1. Color System

A "Design System" project.

- **Structure**: Monorepo-ish.
- **Services**:
  - `web`: The documentation site.
  - `api`: The token distribution API.
  - `db`: Postgres.

## 2. Hydra

An "Identity Provider" project.

- **Structure**: Single service + DB + Helper.
- **Services**:
  - `hydra`: The core service.
  - `consent`: The login UI.
  - `db`: Postgres.

## 3. Implementation Strategy

We will use "Mock" services where possible to keep the demo lightweight and reliable.

- **Web/UI**: `python3 -m http.server` serving static HTML.
- **API**: Simple Rust binary or Python script returning JSON.
- **DB**: Real Postgres (managed by `locald`).

## 4. Success Criteria

- Running `locald up` in `examples/color-system` starts 3 services.
- Running `locald up` in `examples/hydra` starts 3 services.
- All services are green in the dashboard.
- URLs are accessible.
