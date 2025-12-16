# Axiom 11: The Platform is Self-Hosting

**`locald` is not just a process runner; it is a platform that provides its own operational interfaces as built-in services.**

## The Concept

A traditional process manager (like `systemd` or `supervisord`) is a "silent" utility. You interact with it via CLI, and it runs your processes.

`locald` takes a different approach: it is a **Self-Hosting Platform**. It includes "batteries-included" services that are essential for the developer experience. These services are not external tools; they are part of the daemon itself.

## The Built-in Services

1.  **The Dashboard** (`locald.localhost`):

    - The graphical interface for the workspace.
    - Always available.
    - Served directly by the daemon (embedded assets).

2.  **The Documentation** (`docs.localhost`):
    - The manual for the platform.
    - Always available (offline-first).
    - Served directly by the daemon.

## Principles of Built-in Services

### 1. Always On

These services start when `locald` starts. There is no configuration required to enable them. They are the "dial tone" of the development environment.

### 2. Well-Known Domains

They reside at predictable, memorable domains (`*.localhost`). This reinforces the [Managed Networking](09-managed-networking.md) axiom—`locald` owns the `.localhost` TLD namespace and uses it to provide system services.

### 3. Overridable (The "Dogfooding" Principle)

To support the development of `locald` _using_ `locald`, these built-in services must be overridable.

- If a user starts a project that claims `locald.localhost` (e.g., the `locald-dashboard` repo), the daemon must route traffic to that user service instead of the internal handler.
- This allows developers to iterate on the platform's tools using the platform itself, ensuring [Interface Parity](../experience/03-interface-parity.md).

### 4. Fallback Safety

If the overriding service stops or crashes, the platform should gracefully fall back to the built-in version (or at least not crash the daemon). The "Meta-Service" layer must be robust.
