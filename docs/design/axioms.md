# Design Axioms

These are the fundamental principles that guide the development of `locald`.

## 1. Decentralized Configuration (In-Repo)
The source of truth for a project's configuration lives in the project's repository (e.g., `locald.toml`), not in a central registry.
- **Implication**: `locald` discovers configuration by being invoked within the repo or by reading the file at a registered path.
- **Implication**: "Registration" is a lightweight pointer to the path, not a copy of the config.

## 2. Daemon-First Architecture
The core logic runs as a background daemon (`locald-server`).
- **Implication**: The CLI (`locald`) is a thin client that communicates with the daemon.
- **Implication**: Processes survive CLI disconnects.
- **Implication**: State (running apps, logs) is managed by the daemon.

## 3. Managed Ports & DNS
The user should never have to manually manage ports or `/etc/hosts` for local development.
- **Implication**: Apps bind to ports assigned by `locald` (via env vars like `PORT`).
- **Implication**: `locald` manages `*.local` domains automatically.

## 4. Process Ownership
`locald` owns the child processes. It is a process manager, not just a proxy.
- **Implication**: `locald` captures stdout/stderr.
- **Implication**: `locald` handles restarts and environment injection.

## 5. Interface Parity (TUI = Web)
Information and control available in the Web UI must also be available in the terminal (TUI/CLI).
- **Implication**: The underlying API must support both structured data (JSON) and streaming (logs/terminal).

## 6. 12-Factor Alignment
The tool is designed for 12-factor apps.
- **Implication**: Configuration via environment variables.
- **Implication**: Logs to stdout/stderr.
