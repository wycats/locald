# Dashboard UI

The `locald` dashboard provides a web-based interface for monitoring and controlling services.

## Serving

- The dashboard is served at `http://locald.localhost/`.
- The docs are served at `http://docs.localhost/`.
- By default, `locald` embeds prebuilt dashboard + docs assets into the binary (Cargo feature: `ui`).
- The embed pipeline is enforced at build time: if assets are missing or stale, `crates/locald-server/build.rs` runs `pnpm build` for `locald-dashboard` and `locald-docs` and fails loudly if it cannot produce the assets.
- Assets are embedded from Cargo’s build output directory (`OUT_DIR`); they are not committed under `crates/locald-server/src/assets`.
- To build a headless binary (no embedded UI, no Node/pnpm requirement), build with `--no-default-features`. UI routes return a clear "compiled without UI" response.

## The Rack (Sidebar)

The sidebar, known as "The Rack", lists all services in the current constellation.

- **Scrolling**: The list is scrollable, ensuring all services are accessible even in large constellations.
- **Service Items**: Each item displays the service name, status, and type.
- **Web Chip**: For services with an exposed URL (e.g., `web` type), a "web" chip is displayed. This chip is interactive and serves as a direct link to the service's local URL.

## The Stream (Log View)

The main content area, "The Stream", displays logs from running services.

- **Global Stream**: When nothing is pinned, the Stream shows the aggregate log view. Service names are displayed to allow correlation between events from different services.

## The Deck (Pinned Terminals)

The "Deck" is the focused, interactive surface for one or more pinned services.

- **Unified Pinning**: Selecting a service toggles its pinned state. If one service is pinned, it effectively replaces the old "solo" view; if multiple are pinned, the Deck tiles them.
- **Behavior**:
  - 0 pinned: show the global Stream.
  - 1+ pinned: show the Deck.

This pinning is a dashboard UI concept; it is distinct from registry/"Always Up" pinning used for autostart.

## The System Plane (Daemon Control Center)

The dashboard exposes a dedicated system view for the daemon itself.

- **Entry Point**: The "System Normal" footer at the bottom of the Rack.
- **Effect**: Pins a virtual service named `locald` into the Deck.
- **Purpose**: Stream daemon logs and surface platform state without requiring a separate terminal.
