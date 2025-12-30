# locald-utils

**Vision**: The reliable toolbox for system interactions.

## Purpose

`locald-utils` provides robust, reusable utilities for low-level system operations. It abstracts away platform-specific details and common pitfalls related to file system manipulation, process management, and networking.

## Key Components

- **Process**: Helpers for signal handling, graceful termination, and PID utilities.
- **Filesystem**: Safe path joining, atomic writes, and directory traversal.
- **Certificates**: Utilities for generating and managing development SSL certificates.

## Interaction

Used by `locald-server`, `locald-shim`, and `locald-builder` to perform system operations reliably.

## Standalone Usage

This is a general-purpose utility library that could be useful in other Rust systems programming projects, though it is tailored to `locald`'s needs.
