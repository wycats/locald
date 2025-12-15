---
title: "Design: Advanced Proxying & Traffic Management"
---

**Goal**: Provide a world-class, production-grade networking layer for local development that rivals modern edge proxies (Envoy, Traefik) but with zero configuration.

## The "Same-Origin" Problem

Modern web development often involves multiple services (Frontend, API, Auth, CMS) that need to appear as a single unified application to the browser to avoid CORS issues and cookie complexity.

**Current State**: `locald` maps 1 Domain -> 1 Port.
**Desired State**: `locald` maps 1 Domain -> N Paths -> N Services.

### Proposed Configuration

We can introduce a `[routes]` section in the `locald.toml` (or a dedicated `routes.toml` for complex setups) to define these mappings.

```toml
# locald.toml

[service "frontend"]
command = "npm run dev"
port = 3000

[service "api"]
command = "go run main.go"
port = 8080

[routes]
# The default route goes to frontend
"/" = "frontend"
# /api/* requests are stripped of /api prefix and sent to the 'api' service
"/api" = "api"
# Or preserve the prefix:
"/v2/api" = { service = "api", strip_prefix = false }
```

## Protocol Modernization

Most local proxies are stuck in the HTTP/1.1 era. `locald` aims to be "Production-Grade Local".

### HTTP/2 & HTTP/3 (QUIC)

- **Why**: Testing H2/H3 specific bugs (e.g., header compression, multiplexing behavior) locally is currently painful.
- **Implementation**:
  - Upgrade `axum-server` usage to fully enable H2.
  - Investigate `h3` or `quinn` integration for HTTP/3 support over UDP.
  - Ensure the proxy correctly handles protocol upgrades and ALPN negotiation.

### WebSockets & Streaming

- **Status**: Basic WebSocket support exists.
- **Goal**: Robust support for Server-Sent Events (SSE), long-polling, and gRPC streaming.
- **Validation**: We need a test suite specifically for "weird" traffic (unidirectional streams, half-closed connections).

## Observability & Debugging

The proxy is the best place to inspect traffic.

- **Traffic Log**: A live view of requests flowing through the system (Status, Method, Path, Latency, Upstream Service).
- **Replay**: The ability to "Replay" a captured request from the dashboard (Axiom 2: Workspace).
- **Failure Analysis**: When a request fails (502 Bad Gateway), the proxy should return a rich error page explaining _why_ (e.g., "Service 'api' refused connection on port 8080"), rather than a generic error.

## Prior Art & Inspiration

- **Traefik**: Great dynamic configuration, but too complex for simple local dev.
- **Caddy**: Excellent H2/H3 support and ease of use.
- **Vite Proxy**: Good for simple path rewriting, but tied to the frontend build tool.

`locald` consolidates these into a daemon-managed, language-agnostic layer.
