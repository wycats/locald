---
title: WSL Windows Helper Bridge
stage: 0
feature: Platform Support
---

# RFC 0133: WSL Windows Helper Bridge

**Stage**: 0 (Idea)
**Author**: locald team
**Created**: 2026-01-20
**Related**: RFC 0131 (Windows/WSL Privileged Operations)

## Summary

Define a Windows-side helper that complements `locald` running inside WSL2, enabling end-to-end “works in Windows browsers” experiences while keeping WSL the execution environment.

This RFC is specifically about **Windows-host integration**. Self-contained WSL operation (domains + HTTPS usable *inside WSL*) is expected to work without a Windows helper.

## Motivation

WSL2 users expect `locald up` to work inside WSL (domain routing + HTTPS), and ideally to also work in **Windows** browsers.

- **In-scope for launch**: self-contained WSL usage, where the browser is also inside WSL (e.g., WSLg).
- **Coming**: Windows-host integration so Windows browsers can resolve `*.localhost` and trust TLS.

RFC 0131 outlines privileged operations; this RFC focuses on the **user-facing product shape** and the minimal helper footprint for Windows-host integration.

## Goals

- Provide a clear, minimal helper surface that can be distributed and updated safely.
- Ensure Windows browsers can resolve `*.localhost` and trust local TLS certificates.
- Avoid promising full Windows-native support; keep scope explicitly “Linux-in-WSL2 + Windows browser interop.”

## Non-Goals

- Full Windows-native daemon or service manager.
- WSL1 support.
- Comprehensive Windows networking/VPN edge-case coverage in the first release.

## Proposed Shape

- A Windows helper binary (`locald-helper.exe`) with a narrow API for:
  - Hosts file sync
  - Certificate trust install/remove
  - Port forwarding (when required)
- WSL-side client inside `locald` connects via a named pipe.
- `locald doctor` detects WSL and reports helper status.
- Installers/scripts make helper setup a **guided, explicit opt-in** step.

## UX Expectations

- `locald up` inside WSL prints a clear status line:
  - “Windows helper connected” or
  - “Windows helper missing (run `locald wsl setup`).”
- A single command (`locald wsl setup`) performs Windows helper installation.
- If helper is missing, `locald` still works within WSL, but warns Windows browser support is partial.

## Open Questions

1. **Distribution**: winget vs manual download vs a bundled installer?
2. **Privileges**: service vs on-demand helper with UAC prompts?
3. **Protocol**: shared JSON schema with RFC 0130/0131 or a WSL-specific variant?
4. **Telemetry/Diagnostics**: how to surface helper failures in `locald doctor` and dashboard?
5. **Uninstall/cleanup**: who owns removal of hosts/certs/portproxy rules?

## References

- [RFC 0131: Windows/WSL Privileged Operations](0131-wsl-privileged-operations.md)
