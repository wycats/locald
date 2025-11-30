# Axiom 3: Managed Ports & DNS

**The user should never have to manually manage ports or `/etc/hosts` for local development.**

## Rationale

"Port conflict hell" is a major friction point. "Which port was that service on again?" is a waste of mental energy. `localhost:3000` is meaningless; `my-app.local` is semantic.

## Implications

- **Dynamic Ports**: `locald` binds to a random available port (or a pool) and assigns it to the service via an environment variable (e.g., `PORT`). The service _must_ respect this variable.
- **Reverse Proxy**: `locald` runs a reverse proxy (likely on port 80/443) that maps `domain.local` -> `localhost:ASSIGNED_PORT`.
- **DNS/Hosts**: `locald` must ensure that `*.local` (or specific domains) resolve to `127.0.0.1`. This usually involves editing `/etc/hosts` or running a small DNS resolver.
- **Privilege**: Binding to port 80/443 or editing `/etc/hosts` often requires `sudo`. We need a strategy for this (e.g., `sudo` only for the initial setup or specific operations).
