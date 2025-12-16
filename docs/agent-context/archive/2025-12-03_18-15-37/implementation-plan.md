# Implementation Plan - Phase 21: UX Improvements (Web & CLI)

**Goal**: Improve the user experience of the built-in Web UI (dashboard) and the CLI/TUI.

## Brainstorming

- [x] Discuss specific improvements for the Web UI.
- [x] Discuss specific improvements for the CLI output.
- [x] Discuss specific improvements for the TUI.

## Backend (locald-server)

- [x] **PTY Integration**: Replace `Stdio::piped` with `portable-pty` to support true terminal emulation (colors, progress bars).
- [x] **API Enhancements**:
    - `POST /api/services/:name/start`
    - `POST /api/services/:name/stop`
    - `POST /api/services/:name/restart`
    - `POST /api/services/:name/reset`
    - `GET /api/services/:name` (Detailed info: Env vars, Docker info, etc.)
- [ ] **AI Usability**:
    - Implement `locald ai context` IPC message.
    - Implement `locald ai schema` using `schemars`.

## Web UI (locald-dashboard)

- [ ] **Scaffold**: Create a new Svelte 5 + Vite project in `locald-dashboard/` with TypeScript.
- [ ] **Quality**: Configure strict linting (ESLint, Prettier), `svelte-check`, and add to `lefthook.yml`.
- [ ] **Terminal**: Integrate `xterm.js` with `FitAddon` and `WebLinksAddon` to render the raw PTY stream.
- [ ] **Layout**: Implement a Master-Detail layout (Sidebar + Main Pane).
- [ ] **Service Controls**: Add Start/Stop/Restart/Reset buttons.
- [ ] **Resource Info**: Display Postgres connection strings, Docker container info, etc.
- [ ] **Theming**: Support Light/Dark/System modes.

## CLI (locald-cli)

- [ ] **Tables**: Use `comfy-table` for `locald status`.
- [ ] **AI Commands**:
    - `locald ai context`: Dump system state for LLMs.
    - `locald ai schema`: Dump JSON schema for config.

## TUI

- [ ] **Library**: Evaluate `ratatui` widgets or other libraries for a better experience.
- [ ] **Layout**: Improve the layout to show more information density.

## Dashboard Alias

- [x] `locald.localhost` is already routed to the dashboard.

## User Verification

- [ ] **Web UI**:
    - Open `http://locald.localhost` and verify the new dashboard loads.
    - Verify that logs are streaming correctly with colors (ANSI codes).
    - Verify that Start/Stop/Restart buttons work.
    - Verify that the layout is responsive.
- [ ] **CLI**:
    - Run `locald status` and verify the new table format.
    - Run `locald ai context` and verify the output.
    - Run `locald ai schema` and verify the output.
