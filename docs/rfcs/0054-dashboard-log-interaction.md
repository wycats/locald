# RFC 0054: Dashboard Log Interaction & Copying

- **Status**: Stage 0 (Strawman)
- **Date**: 2025-12-05
- **Author**: GitHub Copilot
- **Axioms**:
  - [Axiom 2: The Dashboard is a Workspace](../design/axioms/experience/02-dashboard-workspace.md)
  - [Axiom 4: Respectful & Relevant Output](../design/axioms/experience/04-output-philosophy.md)

## The Problem

Currently, the dashboard log viewer lacks basic interaction capabilities expected of a "Workspace" (Axiom 2). Specifically:

1.  **Copying is Broken**: Standard `Ctrl-C` often conflicts with the browser or the terminal emulation layer, making it difficult or impossible to extract log data (e.g., stack traces, error messages) for sharing or debugging.
2.  **Noise Overload**: Long-running processes or verbose build outputs can flood the log view, making it hard to find relevant information. While Axiom 4 discusses "folding away" transient UI in the CLI, the Dashboard needs a similar mechanism for persistent logs.

## The Theory

We need to treat the log view not as a static text dump, but as a **structured, interactive document**.

### 1. Explicit Copy Affordances

Since `Ctrl-C` is overloaded (SIGINT vs. Copy), we cannot rely on it exclusively. We must provide explicit UI controls.

- **Selection-Based Actions**: When text is selected, a floating context menu (or a persistent toolbar action) should offer "Copy".
- **Block-Level Actions**: Each logical "block" of logs (e.g., a stack trace, a build step output) should have a "Copy" button.
- **"Copy All" / "Copy Visible"**: Global actions for the entire log stream.

### 2. Semantic Folding (The "Fold Away" Rule applied to Logs)

Axiom 4's "Fold Away" rule for CLI progress bars should be adapted for the Dashboard's persistent logs.

- **GitHub's Approach**: GitHub Actions collapses groups of lines (e.g., `Run npm install`). This is good, but often hides _too much_ context or requires manual expansion to see errors.
- **Our Approach**:
  - **Auto-Collapse Success**: If a block of operations (e.g., "Installing dependencies") succeeds, it can be auto-collapsed to a single summary line ("Installed dependencies in 4s").
  - **Auto-Expand Failure**: If that same block fails, it must automatically expand to show the error and the relevant context leading up to it.
  - **Sticky Headers**: When scrolling through a long block, the header (context) should remain visible.

### 3. The "Active" vs. "Archive" View

The dashboard should distinguish between:

- **Tailing Mode**: The "now". Auto-scrolling. Transient UI elements (spinners) are active.
- **Investigation Mode**: The user has scrolled up or paused. The view stabilizes. Collapsed sections can be manually toggled. Selection and copying are prioritized.

## Proposal

1.  **Implement a "Copy" Button**: A visible button on the log pane header to copy the current selection or the entire visible buffer.
2.  **Implement "Smart Folding"**:
    - Detect logical blocks (e.g., via special log markers or heuristics).
    - Collapse successful blocks by default.
    - Keep failed blocks expanded.
3.  **Keyboard Shortcuts**: Support standard shortcuts where possible, but provide fallbacks (e.g., a "Copy Mode" toggle).

### 3. Technical Architecture: Streaming Parser

To implement "Smart Folding" without waiting for process exit, we need a **Streaming Parser** that sits between the raw PTY output and the frontend renderer.

- **Location**: Likely in the Frontend (TypeScript) to keep the Backend (Rust) simple and agnostic.
- **Mechanism**:
  - Ingest raw chunks of text/ANSI.
  - Maintain a state machine of "Current Block".
  - Inject "Fold Markers" (virtual lines or metadata) into the xterm.js buffer.

### 4. Heuristics: What "Counts" as a Fold?

Defining "Logical Blocks" requires a mix of explicit and implicit signals:

1.  **Explicit Markers**: Support standard CI markers (e.g., `::group::My Task` ... `::endgroup::`) to allow tools to opt-in.
2.  **Implicit Heuristics**:
    - **Indentation Shifts**: A sudden increase in indentation often signals a child task.
    - **Repetitive Prefixes**: Consecutive lines starting with `[webpack]` or `Download:` can be grouped.
    - **Time-Based Grouping**: High-frequency bursts of logs followed by a pause.

## Open Questions

- **Performance**: Can we run regex heuristics on the main thread at 60fps? Do we need a Web Worker?
- How does this interact with the PTY? If we are rendering a raw PTY, can we inject HTML elements for folding? Or do we need to parse the VT100 stream?

## Implementation Strategy: Two-Tier Rendering

To balance performance with fidelity, we adopt a **Two-Tier Rendering Strategy**:

### Tier 1: The Preview (Service Card)

- **Goal**: Glanceability. Show the last few lines of activity.
- **Technology**: `ansi-to-html` (Lightweight HTML generation).
- **Constraints**:
  - **Lossy**: Complex cursor movements (`\r`, `\x1b[A`) are stripped or ignored.
  - **Static**: Does not support active progress bars or overwrites.
  - **Performance**: Extremely cheap to render for 50+ services simultaneously.
- **Justification**: The dashboard grid is for _monitoring status_, not _reading logs_. If a user needs to read logs, they will click into the Inspector.

### Tier 2: The Inspector (Drawer)

- **Goal**: Deep Work. Full terminal emulation.
- **Technology**: `xterm.js` (Full VT100/xterm emulation).
- **Capabilities**:
  - **Full Fidelity**: Supports colors, cursor movements, progress bars, and interactive TUI elements.
  - **Interaction**: Supports selection, copying, and potentially input (stdin).
  - **Performance**: Heavier, but only one instance is active at a time.
- **Justification**: When a user focuses on a service, they expect a "Terminal-like" experience (Axiom 2).

### Handling Control Codes in Tier 1

Since `ansi-to-html` does not handle cursor movements (like `\r` for progress bars), the frontend must **sanitize** the log stream before passing it to the converter.

- **Strip**: `\r` (Carriage Return) - Treat as newline or strip? _Decision: Strip to avoid overwriting issues, or replace with newline if meaningful._
- **Strip**: CSI sequences (Cursor Up/Down/Left/Right) - These cause garbage in static HTML.
