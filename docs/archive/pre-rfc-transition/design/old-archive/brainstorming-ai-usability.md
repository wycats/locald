# Brainstorming: Making `dotlocal` AI-Native

As AI coding agents (like GitHub Copilot, Cline, etc.) become primary users of development tools, `dotlocal` has a unique opportunity to be "AI-Native". This means being as ergonomic for an LLM as it is for a human.

## 1. The "Context Window" Problem

**Problem:** When an AI tries to debug a `locald` issue, it often has to run multiple commands (`status`, `cat logs`, `cat config`) to build a mental model. This consumes tokens and requires multiple round-trips.

**Idea: `locald ai context`**
A single command that outputs a highly compressed, token-efficient summary of the entire system state designed specifically for LLM consumption.

- **Content:**
  - Current `locald.toml` (normalized).
  - Running services and their PIDs/Ports.
  - Last N lines of logs for failing services.
  - Environment variable overrides.
  - System constraints (ports in use, permissions).
- **Format:** JSON or a concise Markdown report.

## 2. The "Action" Interface (CLI Surface Area)

**Philosophy:** We prefer a well-designed CLI surface area over a separate protocol like MCP. If the CLI is good for AIs, it's likely good for power users too.

**Idea: `locald ai` Subcommand Namespace**
Group all AI-specific tools under `locald ai` to keep the main help output clean but discoverable.

## 3. Deterministic Configuration Generation

**Problem:** AIs often struggle to write correct configuration files from scratch because they don't know the exact schema or valid options.

**Idea: `locald ai schema`**

- **Export Schema:** Output the JSON Schema for `locald.toml`.
- **Use Case:** Niche cases where the AI needs to construct a complex config from scratch and wants to guarantee validity.

## 4. Semantic Error Messages

**Problem:** "Connection refused" is generic. An AI needs to know _why_ to fix it.

**Idea: Augmented Error Context**
When `locald` encounters an error, it could provide a "Hint for AI" section in the verbose output.

- _Example:_
  ```text
  Error: Port 8080 is in use.
  [AI Hint]: The process using port 8080 is 'node' (PID 1234).
  Resolution: Kill PID 1234 or change the 'port' in locald.toml.
  ```

## 5. "Intent-Based" Commands

**Problem:** AIs are good at intent ("I want to run a postgres db") but bad at specific implementation details ("download binary X, put it in Y").

**Idea: High-Level Scaffolding**

- `locald add postgres` -> Automatically appends the correct `[service]` block to `locald.toml`.
- `locald add redis` -> Same.
- This reduces the chance of the AI generating a slightly-wrong config block.

## 6. Documentation for AIs (`llms.txt`)

**Idea:** Generate an `llms.txt` file as part of the documentation build process.

- **Content:** A condensed version of the documentation, stripped of marketing fluff, focused on API, configuration options, and common patterns.
- **Mechanism:** Build as a side-effect of the `locald-docs` build (Astro).
- **Annotations:** Allow annotating docs as "human-only" or "AI-only" to tailor the output.

## Summary of Potential Features

1.  **`locald ai context`**: Token-optimized state dump.
2.  **`locald ai schema`**: Source of truth for config generation.
3.  **`llms.txt`**: AI-optimized documentation.
4.  **`locald add <template>`**: Safe scaffolding (already partially exists).
