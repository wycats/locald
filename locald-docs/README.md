# locald-docs

The `locald` documentation site, built with Astro + Starlight.

This site is intended to be the public, persona-driven entry point for documentation.
Operational truth for contributors lives in `docs/manual/`.

## Develop

```bash
pnpm install
pnpm dev
```

Then open the dev server URL printed by Astro (typically `http://localhost:4321`).

## Build

```bash
pnpm build
```

The build output is written to `dist/`.

## Repository integration

In this repository, the docs site is also embedded into the `locald-server` binary at build time. If you’re changing docs as part of a feature, verify both:

1. The site builds (`pnpm build`).
2. The repo docs checks pass (`cargo xtask verify docs`).
