# locald-dashboard-e2e

End-to-end tests for the locald dashboard using Playwright.

## Setup

1. Ensure `locald` is built:

   ```bash
   cargo build
   ```

2. Install dependencies:

   ```bash
   pnpm install
   ```

3. Install Playwright browsers:
   ```bash
   pnpm exec playwright install
   ```

## Running Tests

Run all tests:

```bash
pnpm test
```

Run a specific test file:

```bash
pnpm test -- tests/resilience.spec.ts
```

## Capturing Dashboard Screenshots (Docs)

If you have the dashboard running at `http://locald.localhost/`, you can generate/update the screenshots used in the docs site:

```bash
pnpm screenshots
```

This writes images to `locald-docs/src/assets/screenshots/`.

### Visual approval (Playwright snapshots)

For a reviewable workflow (diff UI + approved baselines), use the screenshot test:

- Open the visual UI:

```bash
pnpm screenshots:ui
```

- Update baselines (accept the new screenshots):

```bash
pnpm screenshots:update
```

- Sync the approved baselines into the docs site (served at `/screenshots/...`):

```bash
pnpm screenshots:sync
```

By default the screenshot test targets `https://dev.locald.localhost/`. Override with `DASHBOARD_URL=...`.

## Test Architecture

- **Harness**: `src/locald-process.ts` manages the `locald` server process. It runs `locald` in a sandbox environment.
- **Fixtures**: `tests/fixtures.ts` provides the `locald` fixture to tests, handling setup and teardown.
- **Resilience**: The dashboard is instrumented with `data-sse-connected` attribute on the `<body>` tag to allow tests to verify connection state.

## Debugging

Run with UI mode:

```bash
pnpm run test:ui
```

Run with debug mode:

```bash
pnpm run test:debug
```
