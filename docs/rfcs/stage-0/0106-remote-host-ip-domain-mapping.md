---
title: Remote Host/IP Domain Mapping
stage: 0
feature: Networking
---


# RFC 0106: Remote Host/IP Domain Mapping

---
title: Remote Host/IP Domain Mapping
stage: 0
feature: Networking
---

# RFC 0106: Remote Host/IP Domain Mapping

## 1. Summary

Allow a user to declare **non-local hosts** (IP/hostname) and the **domains that should target them**, so locald’s “stable domain” ergonomics extend beyond `localhost`.

Optionally support a **command-based resolver** that can derive the host identity dynamically (e.g. Tailscale peer identity), so mappings remain stable even when addresses drift.

## 2. Motivation

locald’s URL model (project/service domains, HTTPS, same-origin workflows) is most valuable when the compute and the browser aren’t always on the same machine:

- Remote dev on a more powerful workstation while browsing locally.
- Multi-device testing (phone/tablet hitting a laptop-hosted service).
- A home lab / always-on machine running “long-lived” dev services.
- Tailnets where the stable identifier is a peer name / DNS name, not an IP.

Today this tends to devolve into hand-maintained `/etc/hosts`, ad-hoc DNS config, or “just use ports”, which breaks the stable-domain + stable-certificate story.

## 3. Detailed Design (Strawman)

### 3.1 Configuration Surface (Resolve the naming mismatch)

The repository’s documented hierarchy says:

- **Global**: `~/.config/locald/config.toml`
- **Context**: `.locald.toml` in parent directories

So the idea “put this in `~/.config/locald/.locald.toml`” conflicts slightly with current reality.

This RFC proposes:

- Canonical location is **Global config** (`~/.config/locald/config.toml`),
- but the data can appear at **any layer** (Global/Context/Workspace/Project) and merge deterministically.

If we want to preserve the “hidden dotfile” aesthetic, we can optionally add `~/.config/locald/.locald.toml` as an alias for global config, but that’s not required for the core capability.

### 3.2 Data Model

A minimal model that keeps intent explicit and supports both static and dynamic targets:

```toml
# Global or Context config

[[host_targets]]
name = "ykatz-mbp"            # stable identifier
address = "100.64.12.34"       # IP or hostname

# Domains that should resolve *to that host*
domains = [
  "dev.locald.localhost",
  "shop.localhost",
]

# Optional: resolve dynamically
[host_targets.resolve]
source = "command"
argv = ["tailscale", "status", "--json"]
selector = { type = "jq", expr = ".Self.DNSName" }
```

Notes:

- Prefer argv form to avoid shell injection / portability traps.
- The “selector” concept is intentionally abstract here; the important idea is that stdout is parsed deterministically.

### 3.3 Behavior (What locald does with it)

Two plausible lanes (we can implement either first):

1. **Export lane**: locald emits config for other systems to consume.
   - Example: `locald domains export --format hosts|dnsmasq|caddy|traefik`.
   - The user applies it to their environment.

2. **Managed lane**: locald keeps host mappings current.
   - Requires a firm responsibility boundary and (potentially) privileges.
   - Needs a refresh policy and failure semantics.

This RFC treats the “mechanism” as downstream of the “source of truth”: the important part is declaring target host identity + domains in one place.

### 3.4 Interaction with HTTPS / Certificates

There are at least two coherent models:

- **locald terminates TLS** for these domains and proxies/forwards traffic to the target host.
- **the target host terminates TLS** and locald only helps establish correct name->address mapping.

The config should be able to support both without forcing the project to change its domain scheme.

## 4. Implementation Plan (If Adopted)

- [ ] Decide lane: export-only first vs managed mapping.
- [ ] Define deterministic merge rules for `[[host_targets]]` (likely merge-by-name with nearest layer winning).
- [ ] Provide introspection tooling (`locald config show --provenance`).
- [ ] Implement the first exporter (likely `/etc/hosts` format as the lowest-friction on Linux).
- [ ] (Optional) Add command-based resolution with strict safety constraints (argv-only, timeouts, opt-in).

## 5. Alternatives

- Keep this out of locald and recommend “Tailscale + external proxy/DNS” recipes.
- Put host targets in project `locald.toml` (but host identity is usually user/environment-specific).

## 6. Unresolved Questions

1. Responsibility boundary: should locald **manage** host/DNS mapping, or only **export**?
2. Canonical layer + merge semantics: Global-only vs layerable; if layerable, do we merge `[[host_targets]]` by `name`?
3. Resolver safety: what is the allowed execution/parsing model, refresh cadence, caching, and failure behavior?
