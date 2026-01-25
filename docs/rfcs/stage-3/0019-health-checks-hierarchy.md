---
title: "Health Checks: Zero-Config Hierarchy"
stage: 3
feature: Health
---

# RFC: Health Checks: Zero-Config Hierarchy

## 1. Summary

Implement a hierarchy of detection strategies for service health.

## 2. Motivation

Users shouldn't have to manually configure health checks for standard setups. We should try to guess the best method.

## 3. Detailed Design

Priority:

1. Docker Native (`HEALTHCHECK`)
2. `sd_notify` (systemd style)
3. TCP Probe (if port is defined)
4. Explicit `health_check` command

### Terminology

- **Health Check**: A mechanism to verify if a service is ready.

### User Experience (UX)

"It just works" for most services.

### Architecture

`HealthChecker` trait/structs.

### Implementation Details

Probe logic in `locald-server`.

## 4. Drawbacks

- False positives/negatives if heuristics fail.

## 5. Alternatives

- Always require explicit config.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- HTTP probe (Phase 23).
