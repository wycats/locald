---
title: Docker Integration
description: Proposal for hybrid host + container workflows.
---

> **Roadmap**: This is a proposal, not yet implemented.

## Goal

Let `locald up` manage both host services and containerized dependencies so logs and lifecycle are unified.

## Direction

- Define container services directly in `locald.toml`.
- Keep the schema minimal (image, ports, env, volumes).
- Preserve the core story: host execution is still the default.
