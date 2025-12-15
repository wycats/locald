---
title: Security Architecture
description: How locald handles privileges securely using the shim
---

# Security Architecture

`locald` is designed to be secure by default. While it needs to perform some privileged operations (like binding to port 80 or 443), it avoids running the entire daemon as `root`.

## The Problem

Developers want to use standard ports (80/443) for their local development servers (`http://docs.localhost`), but these ports are "privileged" on Unix systems, meaning only `root` can bind to them.

Running the entire `locald` server as `root` would be dangerous. If there were a bug in `locald`, it could compromise the entire system. Furthermore, files created by a root process would be owned by root, causing permission headaches in your project directories.

## The Solution: `locald-shim`

`locald` solves this using a helper binary called `locald-shim`.

### How it works

1.  **Installation**: When you run `sudo locald admin setup`, the shim is installed to `~/.cargo/bin/locald-shim`. It is owned by `root` and has the `setuid` bit set. This means that whenever it is executed, it starts with `root` privileges.

2.  **Privilege Separation**: The shim is very small and does not contain the full logic of `locald`. Its only job is to manage permissions and then hand off control.

### Use Cases

#### 1. Starting the Server

When you run `locald up` or `locald server start`:

1.  The CLI detects it needs to bind privileged ports.
2.  It executes `locald-shim server start`.
3.  The shim starts as `root`.
4.  It grants the `CAP_NET_BIND_SERVICE` capability to the process.
5.  It **drops privileges** back to your normal user account.
6.  It executes the real `locald` daemon.

**Result**: The `locald` daemon runs as **you**, but the operating system allows it to bind to port 80.

#### 2. Debugging Ports

When you run `locald debug port 80`:

1.  The CLI detects it needs to inspect system processes (which requires root).
2.  It executes `locald-shim debug port 80`.
3.  The shim starts as `root`.
4.  The shim **internally** checks which process is listening on port 80.
5.  It prints the result and exits.

**Result**: You get the information you need, but the shim never executes arbitrary user code as root.

## Verification

To prevent malicious replacement of the shim, `locald` verifies the integrity of the shim binary on every startup.

- **Debug Builds**: Checks that the version of the shim matches the version of the daemon.
- **Release Builds**: Checks the cryptographic hash of the shim binary against a known good value embedded in the daemon.

If verification fails, `locald` will refuse to start and instruct you to run `sudo locald admin setup` to reinstall the correct shim.

## Security Guarantees

- **No Confused Deputy**: The shim does not blindly execute the `locald` binary as root. It only executes it _after_ dropping privileges.
- **Auditable**: The shim code is small, self-contained, and easy to audit.
- **Update Safe**: When you update `locald` via `cargo install`, the shim remains untouched. This preserves the security configuration without needing to run `sudo` again.
