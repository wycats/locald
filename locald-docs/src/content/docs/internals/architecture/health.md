---
title: "Architecture: Health Checks"
---

This document describes how `locald` determines if a service is ready and healthy.

## 1. Zero-Config Hierarchy

`locald` attempts to automatically detect the best health check strategy for a service, minimizing manual configuration. The hierarchy of checks is:

1.  **Docker Native**: If the service is a Docker container, we poll the Docker API for the container's health status (defined by `HEALTHCHECK` in the Dockerfile).
2.  **`sd_notify`**: If the service supports the systemd notification protocol, we wait for the `READY=1` signal.
3.  **TCP Probe**: If the service has an assigned port, we attempt to connect to that port. Success implies the service is listening.
4.  **Explicit Command**: Users can define a custom `health_check` command in `locald.toml`.

## 2. Docker Polling

For Docker services, `locald` polls the `inspect_container` API endpoint.

- It checks `.State.Health.Status`.
- It waits until the status is `healthy`.
- This avoids race conditions where the container is "running" but the application inside is still booting.

## 3. Dependency Management

Health checks are the foundation of the `depends_on` feature.

- Service B depends on Service A.
- `locald` starts Service A.
- `locald` waits for Service A's health check to pass.
- Only then does `locald` start Service B.
