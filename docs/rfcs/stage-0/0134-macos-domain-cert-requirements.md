<!-- exo:134 ulid:01krkpxdtdapc2q2w3wwm9bm6b -->

# RFC 134: macOS Domain and Certificate Requirements


# RFC 0134: macOS Domain and Certificate Requirements

**Stage**: 0 (Idea)
**Author**: locald team
**Created**: 2026-01-20

## Summary

Clarify macOS requirements and user-facing setup steps for local domains and HTTPS certificates, aligning daemon behavior, CLI guidance, and docs.

## Motivation

macOS users depend on seamless local domains (e.g., `myapp.localhost`) and trusted HTTPS. This requires root-level operations and OS trust store integration that must be predictable, documented, and audited before announcement.

## Goals

- Identify required macOS privileges and integration points.
- Define a consistent, user-friendly setup path.
- Ensure `locald doctor` and docs provide the same guidance.

## Non-Goals

- Replacing macOS trust mechanisms with custom tooling.
- Supporting non-standard TLS stores or third-party keychains.

## Proposed Requirements

- **Hosts file management**: `/etc/hosts` updates via the existing shim.
- **Certificate trust**: install/remove locald CA in the System Keychain.
- **Root privileges**: single `locald admin setup` flow that configures setuid shim and trust prerequisites.

## Daemon Lifecycle (macOS)

If `locald` is “always-on” on Linux via `systemd`, the macOS equivalent should be a `launchd` agent/daemon.

For launch readiness, it’s acceptable to start the daemon on first use, but the product intent should be:

- a background daemon that stays up,
- and a clear install path for enabling it at login.

## UX Expectations

- `locald doctor` reports:
  - whether the shim is installed and privileged,
  - whether the locald CA is trusted,
  - recommended fix commands.
- `locald up` surfaces actionable errors if prerequisites are missing.

## Open Questions

1. **Certificate scope**: System keychain vs login keychain?
2. **Compatibility**: minimum macOS versions supported for CA trust flow?
3. **Revocation/cleanup**: how to ensure certs/hosts entries are removed cleanly?
4. **Automation**: can `locald admin setup` safely prompt for Keychain access?
5. **Always-on**: do we ship a `launchd` plist (install/enable/disable) for v1?

## References

- `locald admin setup` flow (current CLI)
- docs/manual pages for DNS + TLS
