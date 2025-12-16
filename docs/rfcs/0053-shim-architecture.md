---
title: "LocalD Shim Architecture"
stage: 3 # Recommended
feature: Security
---

# RFC 0053: LocalD Shim Architecture

## 1. Summary

This RFC documents the architecture of `locald-shim`, a small, privileged binary that enables the unprivileged `locald` daemon to perform restricted root-level operations.

The shim implements the **Privilege Separation** pattern, ensuring that the complex, network-facing `locald` daemon runs with standard user privileges, while only a minimal, auditable codebase runs as root.

## 2. Motivation

`locald` aims to provide a seamless developer experience, which often requires privileged operations:

- Binding to privileged ports (80, 443) for the reverse proxy.
- Creating container namespaces and cgroups for service isolation.
- Managing system-level resources.

Running the entire `locald` daemon as root is a security risk. The daemon parses complex configuration files, handles network traffic, and executes arbitrary user code (via buildpacks and services). A vulnerability in any of these components could lead to full system compromise.

## 3. Detailed Design

### 3.1 The Shim Binary

`locald-shim` is a Rust binary installed to `~/.local/share/locald/bin/locald-shim` (or similar).

- **Ownership**: `root:root`
- **Permissions**: `4755` (setuid root)

When executed by a regular user, the kernel elevates the process effective UID to 0 (root).

### 3.2 Security Model

The shim is designed to be **minimal** and **strict**. It does not accept arbitrary commands. It only accepts a specific set of hardcoded subcommands with strictly typed arguments.

**Principles:**

1.  **No Arbitrary Execution**: The shim never executes a string passed by the user as a shell command.
2.  **No Callbacks**: The shim never executes the `locald` binary while retaining root privileges. All privileged logic must be native to the shim.
3.  **Argument Validation**: All arguments are validated before use (e.g., ensuring a port is a number, ensuring a path is within allowed bounds).
4.  **Environment Sanitization**: The shim clears dangerous environment variables before executing privileged operations.
5.  **Drop Privileges**: For operations that don't require root (like reporting version), the shim should ideally drop privileges or just run safely.

### 3.3 Supported Commands

The shim currently supports the following operations:

#### `bind`

Opens a privileged port and passes the file descriptor back to the calling process via Unix Domain Socket.

- **Usage**: `locald-shim bind <port>`
- **Security**: Only allows binding to specific ports or interfaces as defined by policy (currently any port, but intended for 80/443).

#### `runc` (Added in RFC 0052)

Executes the OCI runtime `runc` to start a container.

- **Usage**: `locald-shim runc [args...]`
- **Security**:
  - Verifies the command is `runc`.
  - `runc` itself handles the container isolation.
  - The shim ensures `runc` is executed with root privileges to create namespaces.

#### `admin sync-hosts` (Added in RFC 0053 Update)

Updates the `/etc/hosts` file with a list of domains.

- **Usage**: `locald-shim admin sync-hosts [domain1] [domain2] ...`
- **Security**:
  - **Native Implementation**: The logic is implemented directly in Rust within the shim. It does **not** exec `locald` or any other binary.
  - **Input Validation**: Arguments are treated as domain strings.
  - **Scoped Writes**: Only modifies the section between `# BEGIN locald` and `# END locald`.

#### `version` (Added in RFC 0045)

Reports the version of the shim for compatibility checking.

- **Usage**: `locald-shim version` (or `--version`)
- **Output**: SemVer string (e.g., `0.2.0`).

#### `admin cleanup` (Added in RFC 0091)

Recursively removes a directory that may contain root-owned files.

- **Usage**: `locald-shim admin cleanup <path>`
- **Security**:
  - **Path Validation**: Requires absolute path containing `.locald`.
  - **Purpose**: Allows cleaning up artifacts left by `runc` (e.g., `root:root` directories in `.locald/build`).

### 3.4 Installation & Updates

The shim is installed via the `locald admin setup` command, which uses `sudo` to copy the binary and set permissions.

- **Update Mechanism**: `locald` checks the shim version on startup (RFC 0045). If outdated, it prompts the user to run `admin setup`.

### 3.5 Why No Self-Update?

A common request is for the shim to update itself (e.g., `locald-shim admin update <new-binary>`). This is explicitly **rejected** for security reasons:

1.  **Privilege Escalation**: Allowing the shim (root) to replace itself with a user-provided binary is equivalent to `chmod +s <user-file>`. A compromised user account could use this to gain full root access by "updating" the shim to a malicious shell wrapper.
2.  **No Root of Trust**: Since `locald` is often built from source by the user, there is no central signing authority to verify the new binary against.
3.  **Sudo as Trust Anchor**: Requiring `sudo` for updates ensures that the user explicitly authorizes the new code to run as root, maintaining the security boundary.

## 4. Implementation Status

This architecture is currently implemented and active in the codebase.

- **Source**: `locald-shim/` crate.
- **Usage**: `locald-server` uses it for port binding. `locald-builder` uses it for `runc` execution.

## 5. Future Work

- **Capability Dropping**: The shim could be further restricted to only hold `CAP_NET_BIND_SERVICE` or `CAP_SYS_ADMIN` instead of full root, depending on the operation.
- **Signature Verification**: The shim could verify the signature of the `locald` binary calling it. However, this is complex because:
  - **Distribution Model**: `locald` is often built from source (`cargo install`). There is no central authority to sign these binaries. If users sign their own builds, they could just as easily sign a malicious binary, negating the protection.
  - **Novelty & Fragility**: Robust application whitelisting on Linux is typically done at the kernel level (IMA/EVM). Implementing it in userspace is non-standard and prone to bypasses (e.g., a malicious user `ptrace`-ing the signed process to hijack it).
  - **Standard Alternative**: The standard solution is **Polkit**, which authenticates the _user_ or _session_, not the specific binary. We may move to Polkit in the future.
