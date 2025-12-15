# Dashboard UI

The `locald` dashboard provides a web-based interface for monitoring and controlling services.

## Serving

- The dashboard is served at `http://locald.localhost/`.
- The docs are served at `http://docs.localhost/`.
- By default, `locald` embeds prebuilt dashboard + docs assets into the binary (Cargo feature: `ui`).
- The embed pipeline is enforced at build time: if assets are missing or stale, `locald-server/build.rs` runs `pnpm build` for `locald-dashboard` and `locald-docs` and fails loudly if it cannot produce the assets.
- Assets are embedded from Cargo’s build output directory (`OUT_DIR`); they are not committed under `locald-server/src/assets`.
- To build a headless binary (no embedded UI, no Node/pnpm requirement), build with `--no-default-features`. UI routes return a clear "compiled without UI" response.

## The Rack (Sidebar)

The sidebar, known as "The Rack", lists all services in the current constellation.

- **Scrolling**: The list is scrollable, ensuring all services are accessible even in large constellations.
- **Service Items**: Each item displays the service name, status, and type.
- **Web Chip**: For services with an exposed URL (e.g., `web` type), a "web" chip is displayed. This chip is interactive and serves as a direct link to the service's local URL.

## The Stream (Log View)

The main content area, "The Stream", displays logs from running services.

- **Solo Mode**: When a specific service is selected, the view enters "Solo Mode". In this mode, the service name column is hidden to maximize space for log content and reduce visual clutter.
- **All Services Mode**: When viewing the aggregate stream, service names are displayed to allow correlation between events from different services.
