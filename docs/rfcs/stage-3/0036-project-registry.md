---
title: "Project Registry: Centralized Tracking"
stage: 3
feature: Architecture
---

# RFC: Project Registry: Centralized Tracking

## 1. Summary

Implement a centralized `registry.json` to track known projects.

## 2. Motivation

Users have multiple projects. They want to know what's running or pinned.

## 3. Detailed Design

Store list of projects in `~/.local/share/locald/registry.json`.

### Terminology

- **Registry**: The list of known projects.

### User Experience (UX)

`locald registry list`.

### Architecture

`Registry` struct.

### Implementation Details

JSON file.

## 4. Drawbacks

- State synchronization.

## 5. Alternatives

- Scan filesystem (slow).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
