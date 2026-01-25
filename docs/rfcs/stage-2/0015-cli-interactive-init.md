---
title: "CLI: Interactive Init"
stage: 3
feature: CLI
---

# RFC: CLI: Interactive Init

## 1. Summary

Implement `locald init` to guide users through project creation.

## 2. Motivation

New users struggle to create valid `locald.toml` files manually. An interactive wizard reduces friction.

## 3. Detailed Design

The command asks questions (Project name? Command? Port?) and generates the TOML file.

### Terminology

- **Wizard**: Interactive CLI prompt.

### User Experience (UX)

`locald init` -> Answer questions -> `locald.toml` created.

### Architecture

CLI command.

### Implementation Details

Use `dialoguer` crate.

## 4. Drawbacks

- Maintenance of the wizard logic.

## 5. Alternatives

- Documentation only.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Detect existing config (Procfile, package.json) and import it.
