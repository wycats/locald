# Phase 4 Implementation Plan: Local DNS & Routing

## Goals
1.  Enable accessing services via domain names (e.g., `http://my-app.local`) instead of just ports.
2.  Implement a reverse proxy within `locald-server`.
3.  Manage `/etc/hosts` entries for local domains.

## Key Decisions
1.  **Process Ownership (Axiom 04)**: The daemon **must** run as the user to correctly manage child processes (environment, paths, permissions). It cannot run as root.
2.  **Port 80 Strategy**:
    *   **Primary**: Use `setcap cap_net_bind_service=+ep` on the `locald-server` binary. This allows the unprivileged user process to bind port 80.
    *   **Fallback**: If `setcap` is not available or fails, default to a high port (e.g., `8080`).
    *   **Setup**: A `locald admin setup` command (requires sudo) will apply the capability.
3.  **Hosts File Strategy**:
    *   **Mechanism**: We will implement a "Section Manager" in `locald-core`. It will manage a block delimited by `# BEGIN locald` and `# END locald`.
    *   **Cross-Platform**: It will detect the OS and use the correct path (`/etc/hosts` vs `C:\Windows\System32\drivers\etc\hosts`).
    *   **Operation**: The daemon detects changes and prompts the user to run `locald admin sync-hosts` (requires sudo).

## Components

### 1. Reverse Proxy (`locald-server`)
-   **Listener**: A Tokio task listening on Port 80 (or 8080).
-   **Router**: Matches `Host` header to registered services.
-   **Proxy**: Uses `hyper` / `axum` to forward requests.

### 2. Hosts File Manager (`locald-core`)
-   **Struct**: `HostsFileSection`
-   **Methods**: `read`, `needs_update(domains)`, `write(domains)`.
-   **Logic**: Reads file, finds the `locald` block, replaces it with new domains, writes back. Preserves all other content.

### 3. CLI (`locald-cli`)
-   `locald admin setup`: Applies `setcap`.
-   `locald admin sync-hosts`: Updates the hosts file block.

## Risks
-   **Capability Loss**: Recompiling/updating the binary removes the `setcap`. The user will need to re-run `setup`. We should detect this.
-   **Port Conflicts**: Port 80 is popular. We need a nice error message if it's taken.
