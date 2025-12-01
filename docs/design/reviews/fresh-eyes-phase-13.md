# Fresh Eyes Review: Phase 13 (Smart Health Checks)

**Mode**: The Reviewer
**Date**: 2025-12-01
**Subject**: Smart Health Checks Documentation

## 1. The "New User" Perspective

As a new user, I just want my services to start in the correct order. I don't want to write complex health check scripts if I don't have to.

**Questions I have:**
- "How does `locald` know when my database is ready?"
- "What if my service doesn't have a port?"
- "Can I override the health check if `locald` guesses wrong?"
- "Does this work with `docker-compose` style healthchecks?"

## 2. The "Drift" Check

**Feature**: Smart Health Checks
**Implementation**: Docker Health -> sd_notify -> TCP Probe.
**Documentation Gap**: Currently, the docs likely don't mention this hierarchy. Users might assume it just checks if the process is running (PID check), which is the old behavior.

## 3. Recommendations

### Concept: "Zero-Config Readiness"
We need a new concept page explaining that `locald` is "smart" about readiness.
- **Hierarchy**: Explain the order of precedence.
- **Docker**: Explain that we respect the image's `HEALTHCHECK`.
- **Notify**: Explain `sd_notify` support for power users/daemon developers.
- **TCP**: Explain the fallback.

### Reference: Configuration
We need to clarify if there are any configuration options exposed in `locald.toml` for this.
*Self-Correction*: Looking at the code, we didn't add explicit configuration for health checks in `locald.toml` in this phase (it was discussed but maybe not fully implemented as "Explicit Config" in the hierarchy yet, or maybe it is? The walkthrough says "Explicit Config" is last in hierarchy, but I don't recall seeing the code for parsing `healthcheck` in `locald.toml` in `manager.rs`. Let me double check `locald-core/src/config.rs` or `manager.rs` to see if `healthcheck` field exists).

## 4. Action Plan

1.  **Create `concepts/health-checks.md`**: Detailed explanation of the 3 strategies.
2.  **Update `reference/configuration.md`**: Mention that `locald` infers health, and (if implemented) how to configure it.

---

*Checking code for explicit config support...*
