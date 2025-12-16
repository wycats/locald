---
title: Dashboard Workspace
description: Understanding the locald Dashboard.
---

The `locald` dashboard is a workspace for running local services: you can see what’s up, read logs, and focus on the few things you’re actively working on.

<div class="screenshot-wide">

[![Dashboard overview showing the Rack sidebar, Stream timeline, and pinned tiles](/screenshots/dashboard-overview.png)](/screenshots/dashboard-overview.png)

</div>

## The Rack

The left sidebar (“The Rack”) is your service list.

- **At-a-glance status**: which services are running and healthy.
- **Type hints**: small labels like `PG` or `WEB`.
- **Quick access**: click a service to focus it.

## The Stream

The center view (“The Stream”) is a single, time-ordered log timeline.

- **Context**: see how services interact (web requests, DB queries, workers).
- **Filter**: select a service in the Rack to narrow the Stream.

## The Deck

For focused work, pin one or more services to “The Deck”.
This turns the dashboard into a tiled layout of live service terminals.

- **Interactive**: not just read-only logs; you can send input when a service supports it.
- **Persistent**: your layout is saved.

## The System Plane

The Rack footer has a special entry (“System Normal”) that pins a **virtual service**: `locald` itself.

<div class="screenshot-wide">

[![System Plane view showing locald pinned as a virtual service](/screenshots/dashboard-system-plane.png)](/screenshots/dashboard-system-plane.png)

</div>

This is the **System Plane**: a dedicated place to observe the daemon using the same UI as everything else.

- **Daemon logs and status**: view `locald` output without switching back to a terminal.
- **Unified pinning**: there’s one “pin” concept; pin what you want to focus on.
