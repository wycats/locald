---
title: "Default Domain: .localhost"
stage: 3
feature: Architecture
---

# RFC: Default Domain: .localhost

## 1. Summary

Switch the default domain suffix from `.local` to `.localhost`.

## 2. Motivation

`.local` relies on mDNS, which is flaky on macOS and not treated as a Secure Context by browsers. `.localhost` is loopback-only and secure by default.

## 3. Detailed Design

Change default config. Update docs.

### Terminology

- **Secure Context**: A context where powerful features (Service Workers, etc.) are available.

### User Experience (UX)

More reliable DNS resolution.

**Clean URLs**: When displaying URLs in the CLI or Dashboard, standard ports (80 for HTTP, 443 for HTTPS) MUST be omitted. This reinforces the "Production-like" feel of the local environment.

### Architecture

Config default.

### Implementation Details

String replacement.

## 4. Drawbacks

- None.

## 5. Alternatives

- `.test`, `.example`.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
