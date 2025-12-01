# Phase 9 Walkthrough: CLI Ergonomics & Interactive Mode

## Overview
In this phase, we are focusing on the "User Experience" of the CLI.

## Key Decisions

### 1. Interactive Init
We used `dialoguer` to create a wizard-style interface for `locald init`. This guides the user through creating a valid `locald.toml` without needing to read the documentation first.

### 2. TUI Library
We chose `ratatui` as it is the community standard fork of `tui-rs`. It provides a robust framework for building terminal UIs.

### 3. Error Hints
We added a `handle_ipc_error` helper to detect common connection errors and suggest running `locald server`. This directly addresses the "App Builder" persona's need for clear error messages.

## Changes

### Codebase
- **`locald-cli/src/init.rs`**: Implemented interactive project initialization.
- **`locald-cli/src/monitor.rs`**: Implemented TUI monitor using `ratatui`.
- **`locald-cli/src/main.rs`**: Added `init` and `monitor` commands, and improved error handling.
- **`locald-cli/Cargo.toml`**: Added `dialoguer`, `ratatui`, `crossterm`.

### Documentation
- (Pending updates to CLI reference)

