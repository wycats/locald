---
title: "CLI: TUI Monitor"
stage: 3
feature: CLI
---

# RFC: CLI: TUI Monitor

## 1. Summary

Implement `locald monitor` using a TUI library for a real-time dashboard.

## 2. Motivation

Users want to see the status of all services and stream logs without leaving the terminal.

## 3. Detailed Design

A split-screen view: Service list on the left, logs on the right.

### Terminology

- **TUI**: Text User Interface.

### User Experience (UX)

`locald monitor` opens the TUI.

### Architecture

CLI command.

### Implementation Details

Use `ratatui`.

## 4. Drawbacks

- Complexity of TUI development.

## 5. Alternatives

- Web Dashboard (we have that too).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Interactive controls (start/stop) in TUI.
