# Phase 24 (Prerequisite): Asset Pipeline & Dashboard Fix

## Goal

Ensure that `locald` serves the actual SvelteKit dashboard and Starlight documentation, rather than placeholders, and establish a reliable build pipeline for these assets.

## Changes

### 1. Asset Build Pipeline

- Created `scripts/build-assets.sh`: A script that:
  - Builds `locald-dashboard` (SvelteKit).
  - Builds `locald-docs` (Astro/Starlight).
  - Copies the build artifacts to `locald-server/src/assets`.
- Added `build:assets` script to `package.json` for easy execution.

### 2. Dashboard Configuration

- Switched `locald-dashboard` to use `@sveltejs/adapter-static`.
- Configured `fallback: 'index.html'` to support Single Page Application (SPA) routing within the embedded environment.

### 3. Documentation Configuration

- Updated `locald-docs/scripts/sync-manifesto.mjs` to inject YAML frontmatter (title) into synced design documents, ensuring they build correctly with Starlight's content collections.

### 4. Server Routing

- Verified `locald-server/src/proxy.rs` logic:
  - `locald.localhost` -> Serves the embedded Dashboard.
  - `docs.localhost` -> Serves the embedded Documentation.

## Verification

- Ran `scripts/build-assets.sh` successfully.
- Recompiled `locald` (forced rebuild of `locald-server`).
- Verified `curl http://locald.localhost` returns the SvelteKit app.
- Verified `curl -H "Host: docs.localhost" http://127.0.0.1` returns the Starlight docs.
