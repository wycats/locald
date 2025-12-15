# Axiom: The Dashboard is a Living Workspace

**The Dashboard is not a passive report; it is an active, living workspace where development happens.**

It transcends the traditional "admin panel" or "log viewer" to become a primary tool for understanding and manipulating the development environment. It respects the user's attention, reflects their mental model, and exposes the reality of the system without magic.

## 1. The Workspace Metaphor

Most dashboards are **Monitors**: passive screens you look _at_ to check status.
`locald`'s dashboard is a **Workspace**: a surface you work _in_.

- **Active vs. Passive**: A monitor shows you that a service crashed. A workspace lets you restart it, debug it, and edit its config, all in flow.
- **Tool, not Report**: The UI is designed for interaction. Logs are searchable and tail-able. Ports are clickable. Dependencies are visual.
- **Responsiveness**: It must feel as immediate and tactile as the CLI. Latency breaks the illusion of a workspace.

## 2. The "Calm Surface" Rule

Development environments are noisy. A raw feed of every event is overwhelming. The dashboard must act as a noise-canceling interface.

- **Suppress Noise**: Successful health checks, routine keep-alives, and stable state transitions should fade into the background.
- **Amplify Signal**: Errors, state changes, and explicit user interactions must pop.
- **Progressive Disclosure**: Show the high-level status (Green/Red) first. Allow the user to "drill down" into logs, environment variables, and raw config only when they need to.
- **Stable Layouts**: Things shouldn't jump around. A service that is crashing shouldn't resize the entire grid. The surface remains calm even when the underlying system is turbulent.

## 3. The "Glass Box" Rule

We value transparency over magic. The dashboard should show the user exactly what is happening, not a sanitized marketing version of it.

- **Visible Configuration**: Don't hide how a service is running. Show the exact command, the working directory, and the environment variables.
- **State Transparency**: If a service is waiting on a port, show that dependency explicitly. If a build is cached, show the cache key.
- **No "Settings" Tab**: Configuration is part of the workspace, not a hidden modal. The state of the system is the UI.

## 4. The "Mental Model" Alignment

The daemon thinks in PIDs, sockets, and file descriptors. The user thinks in Projects, URLs, and Features. The dashboard must bridge this gap.

- **Projects over Processes**: Group related services into logical units (e.g., "Shop Backend" + "Shop Frontend" = "Shop Project").
- **URLs over Ports**: Users want to open `http://localhost:3000`, not know that PID 12345 is listening on 0.0.0.0:3000.
- **Relationships over Lists**: Show how services connect. If Service A depends on Service B, visualize that link.
- **Human-Readable Status**: "Waiting for Database" is better than "Exit Code 1".
