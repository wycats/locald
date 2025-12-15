---
title: "Design: UX Improvements (Web & CLI)"
---

**Goal**: Improve the visual presentation of the CLI and TUI, especially for tables and monitoring.

## CLI/TUI Improvements

- **Goal**: Improve the visual presentation of the CLI and TUI, especially for tables and monitoring.
- **Mechanism**:
  - **Tables**: Use a better table library (e.g., `nu-table` or similar) that handles wrapping and resizing gracefully. The current `locald status` table looks "smooshed" on narrow terminals.
  - **TUI**: The current `locald monitor` is "pretty weak". Investigate better TUI libraries or components (e.g., `ratatui` widgets, `tui-realm`, or `inquire` for interactive prompts) to create a more robust dashboard.
  - **Dashboard Alias**: Add `locald.localhost` as an alias for the dashboard and ensure it appears in the service list.
