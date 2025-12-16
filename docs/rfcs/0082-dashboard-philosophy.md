---
title: "Rationale: The Dashboard is a Living Workspace"
stage: 3 # Recommended
feature: Design
---

# RFC 0082: Rationale for Dashboard Philosophy

## 1. Summary

This RFC establishes the rationale behind **Axiom 5: The Dashboard is a Living Workspace**. It argues that for `locald` to succeed as a "Premium" developer tool, its dashboard must evolve from a passive status monitor into an active, noise-canceling workspace that reflects the user's mental model of "Projects" and "Features" rather than the daemon's model of "Processes" and "Ports".

## 2. Motivation

### 2.1. The "Monitor" Trap

Most developer tool dashboards fall into the "Monitor" trap: they are passive screens you look _at_ to check if something is broken. They are often:

- **Low Fidelity**: Just a list of names and green dots.
- **High Noise**: Streaming raw logs that mix successful health checks with critical errors.
- **Opaque**: Hiding configuration and state behind "Settings" tabs or CLI flags.

### 2.2. The "Premium" Expectation

Modern "Premium" tools (Vercel, Railway, Tilt) have shifted the baseline. Developers expect:

- **Context**: Grouping services by Project/Repo, not just a flat list.
- **Calmness**: "Smart Folding" of logs to hide success and highlight failure.
- **Transparency**: Seeing exactly what command is running and why.
- **Tactility**: Being able to restart, debug, and inspect services with immediate feedback.

## 3. The Philosophy (Axiom 5)

We have codified this philosophy in [Axiom 5](../../design/axioms/experience/05-dashboard-philosophy.md). The core tenets are:

1.  **The Workspace Metaphor**: The dashboard is a tool you work _in_. It must be responsive, interactive, and dense with useful controls.
2.  **The "Calm Surface" Rule**: The UI must act as a noise-canceling filter. Stable states fade back; changes and errors pop forward.
3.  **The "Glass Box" Rule**: No magic. Configuration (Env vars, commands) is visible and accessible, not hidden.
4.  **The "Mental Model" Alignment**: The UI speaks the user's language (Projects, URLs) not the kernel's language (PIDs, Sockets).

## 4. Implications for Design

This philosophy mandates specific design choices for Phase 81 and beyond:

- **Navigation**: Must support a "Project" hierarchy, not just a flat service list.
- **Logs**: Must implement "Smart Folding" to suppress successful build output.
- **Status**: Must use rich semiotics (Color + Icon + Animation) to convey state nuances (e.g., "Building" vs "Starting").
- **Configuration**: Must be exposed in an "Inspector" view, always one click away.

## 5. Alternatives Considered

- **CLI-Only**: We could abandon the dashboard and focus solely on the TUI.
  - _Rejection_: The web offers richer visualization (graphs, links) and is a better "second screen" for long-running tasks.
- **"Heroku Clone"**: We could just copy the Heroku dashboard.
  - _Rejection_: Heroku is designed for _production_ monitoring (slow, stable). `locald` is for _development_ (fast, chaotic). We need a tighter feedback loop.
