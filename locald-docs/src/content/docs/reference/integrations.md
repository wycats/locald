---
title: Integrations
description: What locald integrates with, what’s optional, and how failures surface.
---

This page documents integration boundaries: which external capabilities `locald` can use, how stable each integration is, what happens when it’s missing, and how to fix it.

## Stable vs Experimental

- **Stable**: part of the core “taught” workflow; missing requirements are described plainly.
- **Experimental**: opt-in and may change; missing requirements are usually warnings.
- **Legacy**: supported for compatibility but not the preferred path.

## Integration matrix

| Integration | Stability | Used for | If missing | What you see | Remediation |
| --- | --- | --- | --- | --- | --- |
| Privileged shim + cgroup v2 | Stable | Container execution (OCI bundles), cleanup, host safety | Critical failure | `locald doctor` reports critical failures; some features won’t run | `sudo locald admin setup` |
| OCI registry access (pull images) | Stable | `type = "container"`, CNB builder/buildpack image pulls | Runtime failure when pulling | Command fails with an image pull/download error | Ensure outbound HTTPS works; check corporate proxy/CA; retry |
| CNB / Buildpacks | Experimental | `locald build`, services with `[services.*.build]` | Build fails (can’t pull builder/buildpacks or run lifecycle) | Build command errors during builder download or lifecycle run | Ensure registry access; ensure disk space; rerun with verbose logs |
| Docker daemon API (legacy) | Experimental / Legacy | Services using deprecated `image`+`container_port` under `exec` | Docker-based services unavailable | Server logs warn at startup; starting a legacy docker service fails | Start Docker (or compatible daemon) or set `DOCKER_HOST=unix:///path/to/docker.sock` |
| KVM (/dev/kvm) | Experimental | VMM-based workflows (if/when enabled) | VMM features disabled | `locald doctor --verbose` may report KVM missing/unusable | Enable virtualization; ensure `/dev/kvm` exists and is accessible |

## Notes

- `type = "container"` runs OCI images using locald’s embedded runtime via `locald-shim`. It does not require a Docker daemon.
- The legacy Docker daemon path is deprecated and only applies to the older `exec` + `image` configuration.
