---
title: "Axiom 8: Secure Privilege Separation"
---


`locald` often requires privileged operations (binding ports < 1024, inspecting other users' processes), but running the entire daemon as root is a security risk.

## Principles

1.  **Least Privilege**: The main `locald` daemon should run as the normal user whenever possible.
2.  **Secure Shim**: Privileged operations are delegated to a small, auditable, setuid-root binary (`locald-shim`).
3.  **No Confused Deputy**: The shim must **never** blindly execute arbitrary code or binaries provided by the user with elevated privileges.
    - For `debug` commands, the shim implements the logic internally and runs it as root.
    - For `server start`, the shim drops privileges to the target user _before_ executing the daemon, retaining only the specific capabilities needed (e.g., `CAP_NET_BIND_SERVICE`).
4.  **Unidirectional Control Flow**: The shim must **never** execute the `locald` binary (or any other user-controlled binary) while retaining root privileges.
    - Privileged operations (like writing to `/etc/hosts`) must be implemented **natively** within the shim.
    - This prevents infinite recursion loops where the shim calls `locald`, which then tries to call the shim again.
5.  **Transparent Escalation**: The CLI should detect when privileges are needed and automatically re-execute via the shim, providing a seamless user experience without requiring manual `sudo` invocation for every command.
6.  **Strict Discovery**: The shim must be located relative to the `locald` executable (sibling). Environment variable overrides (like `LOCALD_SHIM_PATH`) are strictly forbidden to prevent configuration drift and security bypasses. The only way to "fix" a shim issue is to run `admin setup` (which `locald` may attempt automatically in interactive sessions).

