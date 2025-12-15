# Phase 18 Walkthrough: Documentation Fresh Eyes

**Goal**: Review the documentation with "Fresh Eyes" to ensure it reflects the current state of the project, especially after the Phase 15 changes (Single Binary, SSL, `.localhost`).

## Changes

<!-- Add changes here as they are implemented -->


### Documentation Updates (Phase 18)

- **Getting Started**:
  - Removed `locald-server` installation step (Single Binary).
  - Added "Quick Run" section for `locald run`.
  - Added "Enable HTTPS" section for `locald trust`.
  - Updated examples to use `.localhost` and `locald up`.
- **Index**:
  - Updated hero tagline and feature cards to highlight Zero-Config SSL.
- **Guides**:
  - `dns-and-domains.md`: Purged `.local`, added Zero-Config SSL section, clarified Hosts File usage.
  - `configuration-basics.md`: Updated to use `.localhost` and `locald up`.
- **Reference**:
  - `cli.md`: Added `run`, `trust`, `server`, `up`. Updated examples.
  - `configuration.md`: Updated default domain to `.localhost`.
- **Internals**:
  - `architecture.md`: Rewrote to reflect Single Binary, SSL Stack, and Graceful Shutdown.
  - `development.md`: Updated project structure descriptions.
- **Global Polish**:
  - Purged remaining `.local` and `locald-server` references across all docs.
