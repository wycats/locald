---
title: "Configuration: In-Repo"
stage: 3
feature: Architecture
---

# RFC: Configuration: In-Repo

## 1. Summary

Configuration for a project shall be stored in a `locald.toml` file in the project root.

## 2. Motivation

Configuration should live with the code (Infrastructure as Code). This ensures that the configuration is versioned along with the application and is shared among all developers working on the project.

## 3. Detailed Design

The `locald.toml` file will define the services, dependencies, and other project-specific settings.

### Terminology

- **locald.toml**: The configuration file.

### User Experience (UX)

Users create a `locald.toml` file in their project root. `locald` automatically detects this file when run.

### Architecture

The `locald-core` crate will define the configuration schema and handle parsing.

### Implementation Details

We will use the TOML format and the `serde` library for serialization/deserialization.

## 4. Drawbacks

- Requires a file in the project root.

## 5. Alternatives

- `.locald/config.toml`
- `package.json` extension (for JS projects)

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Support for other formats (YAML, JSON).
- Global configuration (addressed in RFC 0026).
