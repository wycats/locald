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

- `locald-cli/`: The command-line interface binary.
- `locald-server/`: The daemon binary.
- `locald-core/`: Shared library (types, config, IPC).
- `locald-docs/`: This documentation site (Astro Starlight).
- `examples/`: Example projects for testing.

## Working on Documentation

The documentation is built with [Astro Starlight](https://starlight.astro.build/).

```bash
cd locald-docs
npm install
npm run dev
```

This will start a local development server at `http://localhost:4321`.
