---
title: Development Setup
description: How to set up the development environment for locald.
---

## Prerequisites

- **Rust**: Latest stable version. [Install Rust](https://rustup.rs/).
- **Node.js**: Required for building documentation. [Install Node.js](https://nodejs.org/).

## Building the Project

`locald` is a standard Rust workspace.

```bash
# Clone the repository
git clone https://github.com/ykatz/dotlocal.git
cd dotlocal

# Build all crates
cargo build
```

Binaries will be located in `target/debug/`.

## Running Tests

```bash
cargo test
```

## Project Structure

- `crates/locald-cli/`: The main binary (contains both CLI and Server).
- `crates/locald-server/`: The daemon library/module (internal).
- `crates/locald-core/`: Shared library (types, config, IPC).
- `locald-docs/`: This documentation site (Astro Starlight).
- `examples/`: Example projects for testing.

## Working on Documentation

The documentation is built with [Astro Starlight](https://starlight.astro.build/).

```bash
pnpm -C locald-docs install
pnpm -C locald-docs dev
```

This will start a local development server at `http://localhost:4321`.

### Design doc sync

Some pages in this site are generated from the repository’s design documents (for example, `docs/design/*`).

- The sync runs as part of `pnpm -C locald-docs build`.
- When editing these pages, prefer updating the source document in `docs/design/` rather than the generated copy.

### Embed pipeline

In this repository, the built docs site is embedded into the `locald` binary (behind the default UI feature set) and served at runtime.
If you change documentation as part of a feature, verify both:

- `pnpm -C locald-docs build`
- `cargo xtask verify docs`
