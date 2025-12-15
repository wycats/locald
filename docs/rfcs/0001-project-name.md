---
title: "Project Name: locald"
stage: 3
feature: Architecture
---

# RFC: Project Name: locald

## 1. Summary

The project shall be named `locald`.

## 2. Motivation

We need a name for the local development proxy/manager. The name should reflect its purpose: a daemon (`d`) that runs locally (`local`) to manage development services.

## 3. Detailed Design

The name `locald` is concise, descriptive, and follows the Unix convention of appending `d` to daemon processes (e.g., `systemd`, `httpd`).

### Terminology

- **locald**: The name of the project and the primary binary.

### User Experience (UX)

Users will interact with the tool via the `locald` command.

### Architecture

N/A

### Implementation Details

The binary will be named `locald`.

## 4. Drawbacks

None.

## 5. Alternatives

- `dev-proxy`
- `local-manager`

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
