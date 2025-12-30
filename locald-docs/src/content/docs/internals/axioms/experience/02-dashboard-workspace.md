---
title: "Axiom 9: The Dashboard is a Workspace"
---


**The Dashboard is not merely a log viewer; it is the primary interface for the development environment.**

It must support the user's workflow through stability, interactivity, and organization. It should feel as responsive and capable as a local terminal.

## Implications

- **Persistence**: The dashboard state (sort order, active filters) should persist across reloads.
- **Interactivity**: Users should be able to interact with running processes (PTY) directly from the browser.
- **Completeness**: Any action available in the CLI should be available in the Dashboard, and vice versa.
- **Stability**: The layout should be predictable. Services shouldn't jump around randomly.

