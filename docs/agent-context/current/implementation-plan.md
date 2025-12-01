# Phase 9 Implementation Plan: CLI Ergonomics & Interactive Mode

## Goal
Improve the user experience of the `locald` CLI by making it more helpful, interactive, and visually appealing. We want to reduce the friction for new users ("App Builder" persona) and provide better visibility for power users.

## User Requirements
- **App Builder**: "I don't know how to write the config file. Can you help me?" -> `locald init`.
- **App Builder**: "Something went wrong, but I don't understand the error." -> Better error messages.
- **Power User**: "I want to see what's happening with all my services in real-time." -> `locald monitor`.

## Strategy
1.  **Interactive Init**: Use `dialoguer` or `inquire` to prompt the user for project name and service command, then generate `locald.toml`.
2.  **Better Errors**: Use `miette` or `thiserror` (already using `anyhow`, maybe refine usage) to provide context-aware error messages. Ensure IPC errors are propagated clearly.
3.  **TUI Monitor**: Use `ratatui` to build a terminal user interface that shows running services, their status, and potentially streams logs.

## Step-by-Step Plan

### Step 1: Interactive Init
- [ ] Add `dialoguer` dependency to `locald-cli`.
- [ ] Implement `locald init` command.
- [ ] Prompt for: Project Name, Service Name, Command, Port (optional).
- [ ] Generate `locald.toml`.

### Step 2: Error Handling Polish
- [ ] Review current error output for common failure modes (daemon not running, config missing, port conflict).
- [ ] Improve error messages to be actionable (e.g., "Daemon not running. Run `locald server` first.").

### Step 3: TUI Monitor (`locald monitor`)
- [ ] Add `ratatui` and `crossterm` dependencies to `locald-cli`.
- [ ] Implement `locald monitor` command.
- [ ] Create a basic layout: List of services on the left, details/logs on the right.
- [ ] Fetch status periodically from `locald-server` via IPC.

### Step 4: Verification
- [ ] Test `locald init` in a clean directory.
- [ ] Test error scenarios.
- [ ] Verify `locald monitor` updates in real-time.
