---
title: "Hosts File: Section Management"
stage: 3
feature: Networking
---

# RFC: Hosts File: Section Management

## 1. Summary

The system shall manage a dedicated section in `/etc/hosts` for local domain mapping, ensuring safety and minimizing conflicts.

## 2. Motivation

We need to map local domains (e.g., `project.localhost`) to `127.0.0.1`. Modifying `/etc/hosts` is risky and requires root privileges. A managed section ensures we don't overwrite user data.

## 3. Detailed Design

We implement a "Section Manager" that only touches lines between `# BEGIN locald` and `# END locald`.

### Terminology

- **Section**: A block of text delimited by markers.

### User Experience (UX)

Users run `locald admin sync-hosts` (with sudo) to update the hosts file.

### Architecture

`HostsFileSection` struct in `locald-core`.

### Implementation Details

Read `/etc/hosts`, find the markers, replace the content between them, and write back.

## 4. Drawbacks

- Requires root.
- Modifies a system file.

## 5. Alternatives

- `dnsmasq` (more complex setup).
- Custom DNS resolver.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Integration with systemd-resolved.
