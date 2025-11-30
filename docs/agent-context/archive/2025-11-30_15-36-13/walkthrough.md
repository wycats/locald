# Phase 4 Walkthrough: Local DNS & Routing

## Overview
This phase focuses on enabling domain-based access to services (e.g., `http://app.local`) via a reverse proxy and `hosts` file management.

## Changes

### Hosts File Management
We implemented a `HostsFileSection` manager in `locald-core` that safely manages a block of domains in `/etc/hosts` (or Windows equivalent). It uses `# BEGIN locald` and `# END locald` markers to avoid touching other entries.

### CLI Admin Commands
We added a new `admin` subcommand group to `locald-cli`:
- `locald admin setup`: Applies `setcap cap_net_bind_service=+ep` to the `locald-server` binary, allowing it to bind port 80 without running as root.
- `locald admin sync-hosts`: Fetches the list of running services from the daemon and updates the hosts file to point their domains to `127.0.0.1`.

### Reverse Proxy
We implemented a `ProxyManager` in `locald-server` using `hyper` and `hyper-util`.
- It listens on Port 80 by default.
- If Port 80 is unavailable (e.g., permission denied), it falls back to Port 8080.
- It inspects the `Host` header of incoming requests.
- It queries the `ProcessManager` to find a running service with a matching `domain` in its configuration.
- It forwards the request to the service's assigned port.

### Integration
The `locald-server` main loop now starts the `ProxyManager` alongside the IPC server.
