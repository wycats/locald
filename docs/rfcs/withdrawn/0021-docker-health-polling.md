---
title: "Docker Health: Polling"
stage: 3
feature: Health
---

# RFC: Docker Health: Polling

## 1. Summary

Poll `inspect_container` to check Docker health status.

## 2. Motivation

We need to know when a Docker container is healthy. Polling is simple and effective.

## 3. Detailed Design

Periodically run `docker inspect` (via API) and check `.State.Health.Status`.

### Terminology

- **Polling**: Repeatedly checking a resource.

### User Experience (UX)

Docker services wait until healthy.

### Architecture

`DockerHealthChecker`.

### Implementation Details

Use `bollard` crate.

## 4. Drawbacks

- Latency (polling interval).

## 5. Alternatives

- Docker Events API (more complex).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Switch to Events API.
