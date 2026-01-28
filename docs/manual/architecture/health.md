# Architecture: Health Checks

> **Core focus**: Health checks exist to keep the primary workflow reliable and transparent.

This document describes how `locald` determines if a service is ready and healthy.

## 1. Zero-Config Hierarchy

`locald` attempts to automatically detect the best health check strategy for a service, minimizing manual configuration. The hierarchy of checks is:

1.  **HTTP Probe**: If the service exposes an HTTP health endpoint, `locald` checks for a successful response.
2.  **TCP Probe**: If the service has an assigned port, we attempt to connect to that port. Success implies the service is listening.
3.  **Explicit Command**: Users can define a custom `health_check` command in `locald.toml`.
4.  **`sd_notify`**: If the service supports the systemd notification protocol, we wait for the `READY=1` signal.

## 2. Health Check Types

- **HTTP Probes**: Hit a configured or auto-detected health URL and require a successful HTTP response.
- **TCP Probes**: Attempt a TCP connection to the service port to confirm it is listening.
- **Command Probes**: Execute a user-specified command and require a zero exit status.
- **`sd_notify`**: Wait for `READY=1` from services that support systemd-style readiness.

## 3. Dependency Management

Health checks are the foundation of the `depends_on` feature.

- Service B depends on Service A.
- `locald` starts Service A.
- `locald` waits for Service A's health check to pass.
- Only then does `locald` start Service B.
