---
title: "Design: AI Usability Features"
---

**Goal**: Make `dotlocal` the most AI-friendly local development tool by providing first-class support for AI agents via the CLI and documentation.

## 1. The `locald ai` Namespace

We will introduce a new top-level subcommand `ai` to house tools specifically designed for machine consumption.

### `locald ai context`

**Purpose**: Provide a "one-shot" context dump for an AI to understand the current state of the system.

**Output Format**: Markdown (optimized for LLM reading).

**Content**:

1.  **System Info**: OS, `locald` version, uptime.
2.  **Configuration**: The content of `locald.toml` (if present).
3.  **Service Status**: A table of running services, their PIDs, ports, and health status.
4.  **Recent Logs**: The last 10-20 lines of logs for _failed_ or _unhealthy_ services (critical for debugging).
5.  **Environment**: Key environment variables that might affect execution.

**Example Output**:

````markdown
# locald Context

## System

- Version: 0.1.0
- OS: Linux

## Configuration (locald.toml)

```toml
[project]
name = "my-app"
...
```
````

## Services

| Name | Status  | Health  | Port | PID |
| ---- | ------- | ------- | ---- | --- |
| web  | running | healthy | 3000 | 123 |
| db   | running | healthy | 5432 | 124 |

## Issues

No unhealthy services detected.

```

### `locald ai schema`

**Purpose**: Provide the JSON Schema for `locald.toml` to help AIs generate valid configuration.

**Implementation**: Use `schemars` to derive JSON Schema from the `LocaldConfig` struct.

## 2. `llms.txt` Generation

**Purpose**: Provide a condensed, high-signal documentation file for AIs to "read" before working with `locald`.

**Implementation**:
- Create a script `scripts/generate-llms-txt.sh` (or Rust binary).
- It concatenates key documentation files (Introduction, Configuration Reference, CLI Reference).
- It strips out "human" sections (like "Why locald?", screenshots, etc.).
- It is served at `https://docs.localhost/llms.txt` (via the embedded docs server).

## 3. Implementation Plan

1.  **Add `schemars` dependency** to `locald-core` and derive `JsonSchema` for config structs.
2.  **Implement `locald ai` subcommand** in `locald-cli`.
3.  **Implement `locald ai schema`** to print the derived schema.
4.  **Implement `locald ai context`** to gather state from the daemon (via a new IPC message `GetContext` or by aggregating existing `Status` + `GetConfig` calls).
    - *Note*: Aggregating on the client side might be easier for now to avoid bloating the daemon protocol, but the daemon has the logs. Let's add `GetContext` to IPC for efficiency.
5.  **Create `llms.txt` generator** and integrate it into the build process.
```
