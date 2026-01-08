---
title: Windows/WSL Privileged Operations
stage: 0
feature: Container Development
---

# RFC 0131: Windows/WSL Privileged Operations

**Stage**: 0 (Idea)
**Author**: locald team
**Created**: 2026-01-07
**Related**: RFC 0130 (Host Shim Daemon)

## Summary

Enable `locald` to perform privileged operations (hosts file, certificate trust, port forwarding) when running inside WSL2, delegating to a Windows-side helper.

## Motivation

WSL2 (Windows Subsystem for Linux version 2) is a popular development environment that runs a real Linux kernel in a lightweight VM on Windows. However, privileged operations present unique challenges:

### The WSL2 Challenge

1. **Separate Hosts Files**: WSL2's `/etc/hosts` is independent from Windows's `C:\Windows\System32\drivers\etc\hosts`. For `*.localhost` domains to work in Windows browsers, the Windows hosts file must be modified.

2. **Separate Certificate Stores**: WSL2's certificate trust store (e.g., `/etc/ssl/certs`) is separate from the Windows Certificate Store. HTTPS certificates trusted in WSL2 won't be trusted by Windows browsers.

3. **Network Isolation**: WSL2 runs in a VM with separate networking:

   - **NAT mode** (default): WSL2 has a separate IP address, requires port forwarding
   - **Mirrored mode** (Windows 11 22H2+): `localhost` is shared, but still requires hosts file sync

4. **Cross-Boundary Operations**: Unlike RFC 0130 (container development environments), which deals with Linux-to-Linux communication via Unix sockets, WSL2 requires Linux-to-Windows IPC across the VM boundary.

### Goal

`locald up` inside WSL2 should automatically:

- Update the Windows hosts file (not just WSL2's)
- Install certificates in the Windows Certificate Store (not just WSL2's)
- Configure port forwarding (if in NAT mode)
- Provide a seamless "just works" experience for Windows browsers

## Key Differences from RFC 0130

| Aspect                   | RFC 0130 (Containers)           | RFC 0131 (WSL2)               |
| ------------------------ | ------------------------------- | ----------------------------- |
| **Type**                 | Linux container (shared kernel) | Linux VM (separate kernel)    |
| **Network**              | Shared network namespace        | Separate (NAT or Mirrored)    |
| **Hosts file**           | One (`/etc/hosts`)              | Two (WSL + Windows)           |
| **Certificate store**    | One (Linux trust store)         | Two (Linux + Windows Store)   |
| **IPC mechanism**        | Unix socket                     | Windows named pipe or network |
| **Target platform**      | Host Linux                      | Host Windows                  |
| **Privilege escalation** | `sudo` / setuid                 | Windows UAC / service         |

## Proposed Approach

### High-Level Architecture

```
┌───────────────────────────────┐
│ WSL2 (Linux)                  │
│                               │
│  locald (Rust)                │
│      │                        │
│      │ IPC Request            │
│      ▼                        │
│  Named Pipe Client            │
│  (\\.\pipe\locald-shim)       │
└───────────────────────────────┘
             │
             │ VM boundary
             ▼
┌───────────────────────────────┐
│ Windows                       │
│                               │
│  locald-shim.exe (Rust)       │
│      │                        │
│      ├─► C:\...\etc\hosts     │
│      ├─► certutil.exe         │
│      └─► netsh (port fwd)     │
└───────────────────────────────┘
```

### Components

1. **locald-shim.exe** (Windows executable)

   - Runs as a Windows service or on-demand process
   - Listens on named pipe `\\.\pipe\locald-shim`
   - Handles privileged operations:
     - Hosts file manipulation
     - Certificate trust via `certutil.exe`
     - Port forwarding via `netsh interface portproxy`
   - Requires Administrator privileges

2. **WSL2 Client** (inside locald)

   - Detects WSL2 environment
   - Connects to Windows named pipe via `/mnt/c/...` or `\\wsl$\...` path translation
   - Falls back gracefully if shim not available

3. **Protocol**
   - JSON-based request/response over named pipe
   - Similar to RFC 0130's Unix socket protocol
   - Authentication via pipe permissions

### Windows Hosts File Manipulation

Windows hosts file is located at `C:\Windows\System32\drivers\etc\hosts` and requires Administrator privileges to modify.

**Approach**:

- Read existing file
- Add/remove locald-managed entries (marked with `# locald:` comment)
- Write atomically (temp file + rename)
- Flush DNS cache via `ipconfig /flushdns`

### Windows Certificate Store Integration

Install certificates using `certutil.exe`:

```cmd
certutil -addstore -user "Root" certificate.crt
```

**Approach**:

- Export certificate from WSL2 to shared location
- Windows shim reads certificate
- Installs to user's Trusted Root Certification Authorities
- Cleanup on service shutdown

### Port Forwarding (NAT Mode)

Detect NAT mode and set up port forwarding:

```cmd
netsh interface portproxy add v4tov4 listenaddress=0.0.0.0 listenport=3000 connectaddress=<WSL2_IP> connectport=3000
```

**Detection**:

- Mirrored mode: Skip port forwarding (localhost works)
- NAT mode: Use `wsl hostname -I` to get WSL2 IP, configure forwarding

### Windows Firewall

May need to add firewall rules for forwarded ports:

```cmd
netsh advfirewall firewall add rule name="locald-3000" dir=in action=allow protocol=TCP localport=3000
```

## Open Questions

1. **Installation Method**: How should users install `locald-shim.exe`?

   - MSI installer?
   - winget package?
   - Manual download + installation script?
   - Auto-download from WSL2 on first run?

2. **WSL1 Support**: Should we support WSL1, or only WSL2?

   - WSL1 has different architecture (syscall translation, no VM)
   - Likely different requirements, may need separate RFC

3. **Windows Firewall**: Should the shim automatically configure firewall rules?

   - Security implications
   - User consent/notification
   - Cleanup on uninstall

4. **Auto-Detection**: How to detect NAT vs Mirrored mode?

   - Check `.wslconfig` file?
   - Network probing?
   - User configuration override?

5. **Service vs On-Demand**: Should `locald-shim.exe` run as:

   - Windows Service (persistent)
   - On-demand process (started by WSL2 client)
   - Both modes supported?

6. **Security Model**:

   - How to authenticate requests from WSL2?
   - Prevent unauthorized processes from using the shim?
   - Named pipe security descriptors?

7. **Update Strategy**: How to handle shim updates?
   - Auto-update from WSL2?
   - Separate update mechanism?
   - Version compatibility checks?

## References

- [RFC 0130: Host Shim Daemon for Container Development Environments](../stage-2/0130-host-shim-daemon.md)
- [Microsoft WSL Documentation](https://docs.microsoft.com/en-us/windows/wsl/)
- [WSL2 Networking](https://docs.microsoft.com/en-us/windows/wsl/networking)
- [Windows Named Pipes](https://docs.microsoft.com/en-us/windows/win32/ipc/named-pipes)
- [certutil Command Reference](https://docs.microsoft.com/en-us/windows-server/administration/windows-commands/certutil)
