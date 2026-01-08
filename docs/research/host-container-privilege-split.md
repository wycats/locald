# Host/Container Privilege Split: Research Analysis

## Executive Summary

This document analyzes how various development tools handle the split between host-privileged operations and containerized development environments, with recommendations for locald's architecture.

**Key Finding**: The most robust pattern is a **host-side daemon with socket-based IPC**, where the container CLI communicates with a privileged host process through a Unix socket mounted into the container.

---

## 1. Toolbx/Distrobox Architecture

### How They Work

Both Toolbx and Distrobox create containers that are **tightly integrated with the host**, not isolated:

| Feature           | Mechanism                                                |
| ----------------- | -------------------------------------------------------- |
| Home directory    | Bind-mounted directly (`-v $HOME:$HOME`)                 |
| Network namespace | **Shared with host** (no NAT, no port forwarding needed) |
| X11/Wayland       | Sockets bind-mounted into container                      |
| D-Bus             | Session bus socket shared                                |
| /dev, /sys        | Selectively shared                                       |
| Host filesystem   | Available at `/run/host`                                 |

### `distrobox-host-exec` / `host-spawn`

The key mechanism for running host commands from inside the container:

```bash
# Inside container - runs on host
distrobox-host-exec podman version

# Or via symlinks (auto-detection)
ln -s /usr/bin/distrobox-host-exec /usr/local/bin/podman
podman version  # Automatically runs on host
```

**Implementation**: Uses `flatpak-spawn --host` or the reimplementation `host-spawn`, which:

1. Communicates via D-Bus to the host's `org.freedesktop.Flatpak` portal
2. Allocates a PTY for proper terminal handling
3. Forwards signals (including SIGWINCH for terminal resize)
4. Preserves environment variables like `$TERM`

### Key Insight for locald

> **Toolbx/Distrobox share the network namespace**, meaning there's no port forwarding problem—containers bind directly to host ports. This is why `locald up` works from inside the container for most features, but privileged operations (cgroups, `/etc/hosts`, privileged ports) still fail.

---

## 2. Prior Art: Privileged Proxies

### Docker Desktop for Mac/Windows

Docker Desktop runs containers inside a **Linux VM** (using HyperKit on older Macs, or Apple Virtualization Framework on newer ones). The architecture:

```
┌─────────────────────────────────────────────────┐
│                 macOS / Windows                  │
│  ┌───────────────────────────────────────────┐  │
│  │           Docker Desktop App               │  │
│  │  • Manages VM lifecycle                    │  │
│  │  • Exposes /var/run/docker.sock            │  │
│  │  • Port forwarding from host → VM          │  │
│  └───────────────────────────────────────────┘  │
│                      │                           │
│  ┌───────────────────▼───────────────────────┐  │
│  │              Linux VM                      │  │
│  │  • Runs dockerd                            │  │
│  │  • Full Linux kernel                       │  │
│  │  • gvproxy for networking                  │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

**Key patterns**:

1. **Socket forwarding**: `/var/run/docker.sock` on the host is a proxy to the VM's socket
2. **Port forwarding**: Handled by `gvproxy` or `vpnkit`
3. **DNS**: `host.docker.internal` resolves to a gateway address (169.254.x.x)
4. **File sharing**: Via 9P/virtiofs between host and VM

### VS Code Dev Containers

Dev containers handle port forwarding through the **VS Code server** running inside the container:

```json
{
  "forwardPorts": [3000, 5432],
  "portsAttributes": {
    "3000": { "label": "Web", "onAutoForward": "openBrowser" }
  }
}
```

**How it works**:

1. VS Code server (inside container) detects listening ports
2. Reports to VS Code client (on host) via websocket
3. Host VS Code opens port on host, tunnels traffic to container
4. Works because VS Code has a privileged host process

> **Key insight**: The pattern is a **host-side agent** that the container process communicates with.

### Podman Rootless Privileged Ports

Podman's approach to privileged ports (< 1024) without root:

```bash
# Option 1: Kernel sysctl (requires root to set, but then works for all users)
sudo sysctl net.ipv4.ip_unprivileged_port_start=80

# Option 2: Use slirp4netns/pasta with port handler
podman run --network pasta:-t,80:80 ...

# Option 3: iptables REDIRECT (requires CAP_NET_ADMIN)
iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080
```

**pasta (default in Podman 5.0)** is interesting:

- Preserves source IP addresses (unlike slirp4netns)
- Copies the host's network configuration
- Supports automatic port forwarding with `-t auto`

---

## 3. IPC/Socket-Based Privilege Delegation

### Pattern: Host Daemon with Unix Socket

The most robust pattern used by Docker, Podman, and similar tools:

```
┌─────────────────────────────────────────────────┐
│                     Host                         │
│  ┌───────────────────────────────────────────┐  │
│  │         Privileged Daemon (locald-shim)    │  │
│  │  • Runs as root (or with capabilities)     │  │
│  │  • Listens on Unix socket                  │  │
│  │  • Performs: hosts sync, cgroups, ports    │  │
│  └─────────────────────┬─────────────────────┘  │
│                        │ /run/locald/shim.sock  │
│  ┌─────────────────────▼─────────────────────┐  │
│  │              Container                     │  │
│  │  ┌─────────────────────────────────────┐  │  │
│  │  │    locald CLI / Server               │  │  │
│  │  │  • Connects to socket                │  │  │
│  │  │  • Requests privileged operations    │  │  │
│  │  └─────────────────────────────────────┘  │  │
│  │  Socket mounted: -v /run/locald:/run/locald │ │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

### Security Considerations

| Concern                  | Mitigation                                                        |
| ------------------------ | ----------------------------------------------------------------- |
| **Who can connect?**     | Socket permissions (0660), group membership, socket path location |
| **Authentication**       | Peer credentials via `SO_PEERCRED`, optional token/secret         |
| **Authorization**        | Allowlist of operations, validate all inputs                      |
| **Sandboxing**           | AppArmor/SELinux profiles on the daemon                           |
| **Privilege escalation** | Principle of least privilege—only expose necessary ops            |

### Example: Docker's Socket Security

Docker's `/var/run/docker.sock`:

- Owned by `root:docker`
- Mode `0660`
- Users in `docker` group get equivalent-to-root access
- More secure setups use `rootless` mode or TLS authentication

---

## 4. DNS/Certificate Solutions

### mkcert Across Host/Container

mkcert creates a local CA and installs it into system trust stores:

```bash
# On host (or wherever mkcert runs)
mkcert -install
mkcert localhost 127.0.0.1 myapp.localhost

# The CA root is stored at:
$(mkcert -CAROOT)/rootCA.pem
```

**Cross-boundary pattern**:

1. Run `mkcert -install` on the **host** to install CA in host browsers
2. Copy `rootCA.pem` into container and install there
3. Share generated certificates via volume mount

**For Node.js** (which doesn't use system store):

```bash
export NODE_EXTRA_CA_CERTS="$(mkcert -CAROOT)/rootCA.pem"
```

### `.localhost` DNS Resolution

Per RFC 6761, `.localhost` and subdomains should resolve to loopback. However:

| OS/System        | Behavior                                     |
| ---------------- | -------------------------------------------- |
| Modern browsers  | `*.localhost` → 127.0.0.1 (built-in)         |
| macOS resolver   | `*.localhost` → 127.0.0.1 (built-in)         |
| Linux glibc      | Only `localhost` → 127.0.0.1, not subdomains |
| systemd-resolved | Configurable via `Domains=~localhost`        |

**Recommended approach for locald**:

1. Use `systemd-resolved` with `~localhost` routing on host
2. Configure dnsmasq/resolved to handle `*.localhost`
3. Or use `/etc/hosts` entries (requires privilege)

### Container DNS Options

For containers needing custom DNS:

```bash
# Use host's resolv.conf
podman run --dns-opt="host"

# Or explicit DNS server
podman run --dns=169.254.1.1

# pasta's built-in DNS forwarding
# Automatically adds aardvark-dns
```

---

## 5. Lateral Ideas & Recommendations

### Idea 1: `flatpak-spawn --host` for On-Demand Operations

**Pros**:

- Already works in Toolbx/Distrobox
- No persistent daemon needed
- Uses existing D-Bus portal infrastructure

**Cons**:

- Requires D-Bus session bus access
- Each operation spawns a new process
- Complex for operations needing persistent state

**Implementation**:

```bash
# Inside container
distrobox-host-exec locald-shim /etc/hosts sync
distrobox-host-exec locald-shim cgroup setup --service myapp
```

### Idea 2: Host Server + Container CLI (Recommended)

**Architecture**:

```
Host:   locald-server (privileged) ←─┐
                                     │ Unix socket or HTTP
Container: locald CLI ───────────────┘
```

**Advantages**:

- Clean separation of concerns
- Server can run as systemd service
- Container can be stateless
- Works across any container runtime
- Similar to Docker's architecture

**Implementation approach**:

1. `locald-shim` becomes a minimal privileged daemon on host
2. Exposes socket at `/run/locald/shim.sock`
3. Socket is bind-mounted into containers
4. CLI detects container environment and uses socket

### Idea 3: Setuid/Capabilities Across User Namespaces

**Short answer**: Not possible in the general case.

User namespaces remap UIDs, so root (UID 0) inside a container maps to an unprivileged UID on the host. Setuid binaries and capabilities are **namespace-scoped**:

- `CAP_NET_BIND_SERVICE` inside the container doesn't grant host port < 1024
- Setuid bit on a binary only elevates within the namespace

**Exception**: With `--userns=keep-id`, some operations work because the container user matches the host user.

### Idea 4: Hybrid Architecture for locald

**Recommended design**:

```
┌──────────────────── HOST ────────────────────┐
│                                               │
│  locald-shim (systemd service, root/caps)    │
│  ├─ Manages /etc/hosts entries                │
│  ├─ Binds privileged ports (port proxy)       │
│  ├─ Manages cgroups                           │
│  ├─ Certificate trust store management        │
│  └─ Exposes: /run/locald/shim.sock           │
│                                               │
│  ┌────────── CONTAINER (Toolbx) ───────────┐ │
│  │                                          │ │
│  │  locald server (unprivileged)            │ │
│  │  ├─ Watches filesystem                   │ │
│  │  ├─ Manages services (non-root)          │ │
│  │  ├─ Handles build processes              │ │
│  │  └─ Delegates to shim via socket         │ │
│  │                                          │ │
│  │  locald CLI                              │ │
│  │  └─ Communicates with server             │ │
│  │                                          │ │
│  │  [Socket mounted from host]              │ │
│  └──────────────────────────────────────────┘ │
└───────────────────────────────────────────────┘
```

**Benefits**:

1. Most operations stay in container (fast, no privilege needed)
2. Only specific privileged ops go through shim
3. Works with existing Toolbx/Distrobox workflow
4. Graceful degradation when shim unavailable (current behavior)

---

## 6. Specific Implementation Suggestions

### 6.1 Socket Protocol

Use a simple JSON-over-Unix-socket protocol:

```json
// Request
{
  "op": "hosts_sync",
  "entries": [
    {"hostname": "myapp.localhost", "ip": "127.0.0.1"}
  ]
}

// Response
{"ok": true}
```

Or use gRPC/protobuf for type safety and streaming.

### 6.2 Socket Location and Permissions

```bash
# Socket path
/run/locald/shim.sock

# Permissions
drwxr-xr-x root:locald /run/locald
srw-rw---- root:locald /run/locald/shim.sock

# Container mount
-v /run/locald:/run/locald:ro
```

### 6.3 Graceful Degradation

Current behavior is already correct—detect and warn:

```
⚠ locald-shim is not available as a privileged helper
⚠ Continuing without privileged features (hosts sync, cgroup isolation, privileged ports)
⚠ For full setup, run `sudo locald admin setup` on the host OS
```

### 6.4 Distrobox Integration

Create a wrapper that auto-mounts the socket:

```bash
# ~/.config/distrobox/distrobox.conf
container_additional_volumes="/run/locald:/run/locald:ro"
```

Or use `distrobox-host-exec` for fallback:

```rust
fn request_privileged_op(op: ShimOp) -> Result<()> {
    if let Ok(socket) = connect_socket("/run/locald/shim.sock") {
        // Use socket
        socket.send(op)?;
    } else {
        // Fallback to host-exec
        Command::new("distrobox-host-exec")
            .args(["locald-shim", &op.to_cli_args()])
            .status()?;
    }
}
```

---

## 7. Security Considerations Summary

| Risk                       | Mitigation                                    |
| -------------------------- | --------------------------------------------- |
| Unauthorized socket access | Socket permissions, group membership          |
| Malicious requests         | Input validation, operation allowlist         |
| Privilege escalation       | Minimal daemon capabilities, AppArmor profile |
| Socket path injection      | Fixed path, no user-controlled paths          |
| Denial of service          | Rate limiting, resource limits on daemon      |
| Information disclosure     | Minimal error messages, audit logging         |

---

## 8. Comparison Matrix

| Approach                        | Complexity | Performance | Security  | Container Agnostic |
| ------------------------------- | ---------- | ----------- | --------- | ------------------ |
| `host-exec` fallback            | Low        | Medium      | Good      | Yes                |
| Host daemon + socket            | Medium     | High        | Good      | Yes                |
| Direct D-Bus                    | Medium     | High        | Good      | No (needs D-Bus)   |
| Kernel sysctl tweaks            | Low        | N/A         | Fair      | Yes                |
| VM-based (Docker Desktop style) | High       | Medium      | Excellent | Yes                |

---

## 9. Recommended Next Steps

1. **Short term**: Implement socket-based communication protocol in `locald-shim`
2. **Medium term**: Add `locald admin setup` command to install and configure shim as systemd service
3. **Long term**: Consider container-native DNS resolver that works without host privilege

## References

- [Distrobox Documentation](https://distrobox.it/)
- [Toolbx Documentation](https://containertoolbx.org/)
- [host-spawn](https://github.com/1player/host-spawn)
- [mkcert](https://github.com/FiloSottile/mkcert)
- [Dev Containers Spec](https://containers.dev/)
- [Podman Rootless](https://github.com/containers/podman/blob/main/rootless.md)
- [Docker Rootless Mode](https://docs.docker.com/engine/security/rootless/)
