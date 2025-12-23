# locald-dashboard

The `locald` dashboard web UI.

This package is built with Svelte (via SvelteKit tooling) + Vite. In this repository, the dashboard is typically served by `locald-server` (embedded at build time for distribution builds).

## Develop

```bash
pnpm install
pnpm dev
```

## Build

```bash
pnpm build
```

## Repository integration

In this repo, dashboard assets are embedded into `locald-server` when building with the default UI features.

- Build assets for embedding: `./scripts/build-assets.sh`
- When building without embedded UI assets, `locald-server` serves a small fallback page instead.
