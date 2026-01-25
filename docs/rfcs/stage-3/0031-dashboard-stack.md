---
title: "Dashboard Stack: Svelte 5 & xterm.js"
stage: 3
feature: Dashboard
---

# RFC: Dashboard Stack: Svelte 5 & xterm.js

## 1. Summary

Use Svelte 5 and xterm.js for the dashboard.

## 2. Motivation

Performance and developer experience. Svelte 5 is fast. xterm.js handles logs better than the DOM.

## 3. Detailed Design

Svelte for UI. xterm.js for log panels.

### Terminology

- **Svelte 5**: The framework.
- **xterm.js**: The terminal component.

### User Experience (UX)

Snappy, responsive dashboard.

### Architecture

Frontend stack.

### Implementation Details

Vite build.

## 4. Drawbacks

- New tech (Svelte 5 is beta/new).

## 5. Alternatives

- React.
- Plain HTML.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
