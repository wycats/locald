# Phase 18 Implementation Plan: Documentation Fresh Eyes

**Goal**: Review the documentation with "Fresh Eyes" to ensure it reflects the current state of the project, especially after the Phase 15 changes (Single Binary, SSL, `.localhost`).

## 1. Content Updates

### Getting Started & Index
- [ ] **`index.mdx`**:
    - Update hero section to mention "Zero-Config SSL" and "Single Binary".
    - Update feature cards to reflect `.localhost` and HTTPS support.
- [ ] **`guides/getting-started.md`**:
    - **Installation**: Remove `locald-server` install step. Change to `cargo install --path locald-cli` (or just `locald`).
    - **Quick Run**: Add a new section before "Your First Service" demonstrating `locald run python3 -m http.server`.
    - **HTTPS**: Add a "Enable HTTPS" step explaining `locald trust`.
    - **Updates**: Ensure all examples use `.localhost` instead of `.local`.

### Core Concepts & Guides
- [ ] **`guides/dns-and-domains.md`**:
    - **Purge `.local`**: Replace all `.local` references with `.localhost`.
    - **SSL**: Explain the "Pure Rust" SSL architecture (no `mkcert` needed) and how `locald trust` works.
    - **Hosts File**: Clarify that `.localhost` is a secure context and might not strictly require `/etc/hosts` in some environments, but `locald` syncs it for consistency.
- [ ] **`guides/common-patterns.mdx`**:
    - Update all code snippets to use `.localhost`.
- [ ] **`concepts/health-checks.md`**:
    - Verify it reflects the Phase 13 hierarchy (Docker -> Notify -> TCP).

### Reference
- [ ] **`reference/cli.md`**:
    - **New Commands**: Add `locald run`, `locald trust`, and `locald server` (subcommand).
    - **Updates**: Update `locald start`/`stop`/`status` output examples to show `.localhost` URLs.
- [ ] **`reference/configuration.md`**:
    - Update default domain to `.localhost`.
    - Clarify port binding behavior (GlobalConfig) and the `admin setup` requirement for port 80/443.

### Internals
- [ ] **`internals/architecture.md`**:
    - **Single Binary**: Refactor to describe `locald` as a single binary containing both Client and Server logic.
    - **SSL Stack**: Document `rcgen`, `rustls`, and `ca_injector` usage for on-the-fly signing.
    - **Shutdown**: Document the Graceful Shutdown protocol (SIGTERM -> Wait -> SIGKILL).

## 2. Global Polish
- [ ] **Purge `.local`**: Grep for `.local` across the entire `locald-docs` directory and replace with `.localhost` (except historical context).
- [ ] **Purge `locald-server`**: Ensure no instructions tell the user to install or run `locald-server` as a standalone binary.

## 3. Verification

- [ ] **Link Check**: Ensure all internal links are valid.
- [ ] **Build Check**: Run `pnpm build` in `locald-docs`.
- [ ] **Visual Verification**: Browse the site locally to ensure formatting is correct.
