---
title: "Axiom 3: Managed Ports & DNS"
---

**The user should never have to manually manage ports or `/etc/hosts` for local development.**

## Rationale

"Port conflict hell" is a major friction point. "Which port was that service on again?" is a waste of mental energy. `localhost:3000` is meaningless; `my-app.localhost` is semantic.

## Implications

- **Dynamic Ports**: `locald` binds to a random available port (or a pool) and assigns it to the service via an environment variable (e.g., `PORT`). The service _must_ respect this variable.
- **Reverse Proxy**: `locald` runs a reverse proxy (likely on port 80/443) that maps `domain.localhost` -> `localhost:ASSIGNED_PORT`.
- **DNS/Hosts**: `locald` ensures that `*.localhost` (or specific domains) resolve to `127.0.0.1` by managing a delimited block in `/etc/hosts`.
- **Privilege**: Binding to port 80/443 requires privilege. We use `setcap cap_net_bind_service=+ep` on the `locald-server` binary (applied via `locald admin setup`) to allow this without running the entire daemon as root.
