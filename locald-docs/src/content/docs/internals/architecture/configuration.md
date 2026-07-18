---
title: "Architecture: Configuration"
---

This document describes how `locald` manages configuration, state, and project tracking.

## 1. Configuration Hierarchy

Configuration is resolved from multiple sources in a specific order (highest priority last):

1.  **Global**: `~/.config/locald/config.toml` (User defaults).
2.  **Context**: `.locald.toml` in parent directories (Directory-specific defaults).
3.  **Workspace**: `locald.workspace.toml` or Git Root (Shared resources, env vars).
4.  **Project**: `locald.toml` (Service-specific settings).

### In-Repo Configuration

The primary configuration lives in `locald.toml` within the project root. This ensures configuration is versioned with the code ("Infrastructure as Code").

### Typed Configuration

Service configuration uses a typed enum approach (e.g., `type = "exec"`, `type = "postgres"`) to allow different schemas for different service types.

## 2. Durable Local State

The daemon keeps durable identity metadata separate from runtime observations.

- **`catalog.json`**: Versioned repository, worktree, project, and project-instance identities; current and historical path locators; Git display metadata; and coarse present/missing state.
- **`state.json`**: Runtime snapshots used to identify stale processes during daemon recovery.
- **Location**: locald's platform data directory (the XDG data directory on Linux and the corresponding application-data directory on macOS).

The identity catalog is daemon-owned metadata. `locald.toml` and discovered workspace files remain authoritative for service configuration.
The daemon holds an exclusive catalog writer lock from startup import through shutdown, so one daemon owns catalog mutation at a time.

## 3. Legacy Project Compatibility

Earlier locald versions recorded paths in `registry.json`, `attachments.json`, and `state.json`. On first catalog creation, locald preserves those files and imports their path locators as compatibility evidence. A present project is resolved to its stable identity; a missing path remains inspectable until the project is rediscovered or explicitly forgotten.

## 4. Gitignore Automation

To prevent local state (logs, temporary files) from being committed, `locald` can automatically append `.locald/` to the project's `.gitignore` file.
