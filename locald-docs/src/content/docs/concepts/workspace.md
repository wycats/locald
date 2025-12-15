---
title: The Cybernetic Workspace
description: Understanding the locald Dashboard.
---

The `locald` Dashboard is not just a status page; it is a **Cybernetic Workspace**. It is designed to provide "ambient awareness" of your system while allowing for focused interaction.

## The Rack

The sidebar, or "The Rack", provides a high-density overview of all your services.

- **Eyebrow Tags**: Quick visual indicators of service type (e.g., `PG`, `WEB`, `WORKER`).
- **Sparklines**: Real-time CPU and Memory usage graphs for each service.
- **Status Indicators**: Traffic lights (Green/Red/Grey) showing the health of each service.

## The Stream

The central view is "The Stream". Unlike traditional log viewers that force you to select a single service, The Stream aggregates logs from **all** services into a single, time-ordered feed.

- **Context**: See how services interact (e.g., Web service request followed immediately by a DB query log).
- **Filtering**: Click on a service in The Rack to filter The Stream to just that service.

## The Deck

For focused interaction, you can "pin" services to "The Deck". This creates a tiling window manager layout where you can see multiple terminal streams side-by-side.

- **Interaction**: These are not just read-only logs. You can interact with them (e.g., send input to a REPL).
- **Persistence**: Your layout is saved, so when you return to the dashboard, your workspace is exactly as you left it.
