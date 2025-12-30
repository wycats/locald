# locald-core

**Vision**: The shared language of the `locald` ecosystem.

## Purpose

`locald-core` defines the fundamental data structures, configuration schemas, and communication protocols that bind the `locald` components together. It ensures that the CLI, Server, and other tools speak the same language.

## Key Components

- **Configuration**: Structs for parsing `locald.toml` and `locald.lock`.
- **IPC**: Definitions for the inter-process communication between the CLI and Server (e.g., `BootEvent`, `LogEntry`).
- **State**: Types representing the runtime state of services and the system.

## Interaction

This crate is a dependency of almost every other crate in the workspace (`locald-server`, `locald-cli`, `locald-builder`, etc.). It contains no business logic, only definitions.

## Standalone Usage

As a library, it can be used by any Rust tool that needs to read `locald` configuration files or communicate with a `locald` server.
