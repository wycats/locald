---
title: Smart Health Checks
description: How locald determines when your services are ready.
---

`locald` uses a "Smart Health Check" system to determine when a service is ready to accept traffic or when dependent services can be started. This allows for a "Zero-Config" experience where `locald` infers the best strategy for your service.

## The Readiness Hierarchy

`locald` checks for readiness signals in the following order of precedence:

1.  **Docker Healthcheck**: If your service is a Docker container and the image has a `HEALTHCHECK` defined, `locald` will poll the container's health status.
2.  **sd_notify**: If your service sends a `READY=1` notification via the systemd `sd_notify` protocol, `locald` will mark it as healthy immediately upon receipt.
3.  **TCP Probe**: If neither of the above applies, but your service exposes a port (either via `port` config or `container_port`), `locald` will attempt to connect to that TCP port. Once a connection is established, the service is considered healthy.

## Strategies in Detail

### 1. Docker Healthcheck

This is the preferred method for containerized services. It relies on the `HEALTHCHECK` instruction in your `Dockerfile`.

**Example `Dockerfile`:**

```dockerfile
FROM nginx
HEALTHCHECK --interval=5s --timeout=3s \
  CMD curl -f http://localhost/ || exit 1
```

`locald` uses the Docker API to inspect the container. When the status reports `healthy`, `locald` proceeds.

### 2. sd_notify (Systemd Notification)

For services that run as native processes (or containers that support it), `locald` implements the `sd_notify` protocol.

When `locald` starts a process, it sets the `NOTIFY_SOCKET` environment variable. Your application can send a datagram to this socket to signal readiness.

**Example (Node.js):**

```javascript
const net = require("net");

const server = net.createServer((socket) => {
  socket.end("Hello World\n");
});

server.listen(process.env.PORT, () => {
  console.log("Server listening");

  // Signal readiness
  if (process.env.NOTIFY_SOCKET) {
    const socket = net.createConnection(process.env.NOTIFY_SOCKET);
    socket.write("READY=1");
    socket.end();
  }
});
```

This is ideal for services that need to perform complex initialization (e.g., database migrations, cache warming) before they are actually ready, which a simple TCP probe might miss.

### 3. TCP Probe (Fallback)

If no other method is detected, `locald` falls back to a TCP probe. It repeatedly attempts to open a TCP connection to the service's configured port on `127.0.0.1`.

**Limitations:**

- A service might accept a TCP connection before it is fully initialized (e.g., a web server might bind the port but return 503s while loading).
- Requires the service to listen on a TCP port.

### 4. Builtin Services

For builtin services like `postgres`, `locald` automatically configures the appropriate health check strategy (usually a TCP probe on the assigned port) so you don't have to do anything.

## Dependency Management

The primary use of Health Checks is to coordinate startup order. If Service B `depends_on` Service A, `locald` will:

1.  Start Service A.
2.  Wait for Service A to become **Healthy** (using one of the strategies above).
3.  Start Service B.

This ensures that Service B doesn't crash because Service A wasn't actually ready yet.
