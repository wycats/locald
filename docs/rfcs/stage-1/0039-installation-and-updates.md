---
title: "Installation & Updates"
stage: 0
feature: General
---

# Design: Installation & Updates

## Installation & Updates

- **Goal**: Streamline the installation and update process, supporting multiple channels.
- **Mechanism**:
  - **`cargo binstall`**: Add support for `cargo binstall` to allow binary installation without compiling.
  - **GitHub Releases**: Support transparent updates from GitHub Releases.
  - **Nightly/Beta Channels**: Use custom attributes or metadata in GitHub artifacts to support nightly and beta channels.
  - **`locald selfupgrade`**: A dedicated command to fetch upstream updates and recycle the server.
  - **`locald up` Integration**: `locald up` should also check for updates before starting services.
  - **Auto-Update Strategy**:
    - Periodically HEAD a lightweight endpoint (e.g., GitHub Pages) to check for new releases.
    - **Opt-In**: Must be strictly opt-in via configuration (e.g., `auto_update = true`) to respect user privacy ("no call home without consent").
    - **Onboarding**: Prompt the user to enable auto-updates during `locald admin setup`.

  ### `locald selfupgrade` (Detailed Flow)

  ```
  locald selfupgrade [--check] [--version <VERSION>]
  ```

  1. Query GitHub Releases for the latest (or specified) version.
  2. If `--check`: print the version comparison and exit.
  3. Download the release tarball to a temp directory.
  4. Verify the SHA256 checksum against the `.sha256` file.
  5. If the daemon is running: call `locald shutdown` for a graceful stop.
  6. Atomically replace the current binary (rename).
  7. Print success.
  8. If the shim version mismatches: print "Run `sudo locald admin setup`".
  9. The user runs `locald up` to restart (services are restored from state).

  Notes:
  - No process adoption. Services restart fresh.
  - State files record which projects were running.
  - Shim upgrades are separate and require sudo.

## Secure Installation

- **Goal**: Provide a secure and ergonomic alternative to `curl | sh`.
- **Mechanism**:
  - Explore alternatives that verify signatures or use trusted package managers where possible.
  - Design an installation pattern that balances ease of use with security best practices (e.g., verifying checksums, using a bootstrap binary).

## Capability Preservation

- **Goal**: Preserve `cap_net_bind_service` across upgrades so users don't have to run `sudo locald admin setup` every time.
- **Mechanism**:
  - Option A: A small, static wrapper binary that execs the real binary (but wrapper needs caps).
  - Option B: `locald up` detects missing caps and prompts for sudo to re-apply them automatically.
  - Option C: Use a systemd service (for Linux) where caps are defined in the unit file.
- **Decision**: Option A, while inelegant, is probably the most reliable across platforms. We should also overtly support Option C for people who know they want systemd.

## Implementation Plan (Stage 2)

- [ ] Implement `locald selfupgrade`.
- [ ] Add `cargo binstall` support.
- [ ] Implement capability preservation strategy.

## Context Updates (Stage 3)

List the changes required to `docs/agent-context/` to reflect this feature as "current reality".

- [ ] Create `docs/agent-context/features/installation.md`
- [ ] Update `docs/agent-context/plan-outline.md` to mark Phase 30 as complete.
