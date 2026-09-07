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

## Isolated service-card dashboard preview

The service-card suite serves the production dashboard build directly from a
loopback-only fixture server. It does not launch locald, use the Vite development
proxy, or contact the installed daemon. Build the dashboard before running it:

```sh
pnpm install --frozen-lockfile
cd locald-dashboard
pnpm build
cd ../e2e/node/locald-dashboard-e2e
PLAYWRIGHT_SKIP_BROWSER_GC=1 pnpm exec playwright install chromium
pnpm test:service-cards
```

For manual review, run `pnpm preview:service-cards` from this package and open the
printed URL (default `http://127.0.0.1:47831`). Set
`SERVICE_CARD_FIXTURE_PORT` to select another owned loopback port. Stop the server
with Ctrl-C after review. All service actions are simulated and recorded in
`/__fixture/requests`; unexpected API calls fail with HTTP 501 and are recorded.
The server has no proxy or process-management capability.

All destination links use reserved `.invalid` names. They preserve realistic
canonical URL shapes and port information, but cannot open live projects.
Automated tests fulfill their navigation with a harmless mock destination.
Manual navigation to them is expected to show the browser's unavailable-site
page. Published fixtures deliberately supply different endpoint and canonical
origin URLs so that the suite verifies which one owns navigation.

The dedicated suite runs without automatic retries. It covers layout at 945,
1440, 640, and 390 pixels; duplicate checkout names; colon-containing service
names; persistent stopped/building destinations; no-URL services; published
all six readiness states and keyboard-operable status details; compact Open
links with full-address popovers; once-per-project checkout labels; native
keyboard and pointer activation; and exact
instance-scoped simulated writes. Each layout case writes a screenshot into its
Playwright result directory. Existing daemon-backed suites use the default
configuration and remain separate.

The nine-row baseline uses actual server-default publication guidance. Bounded
fixture-only state overrides exercise Ready, Waiting for app, Checking connection,
Unavailable, Paused, and Worktree missing without changing row density. The suite
checks translated explanations/actions, canonical destinations, absence of
managed-process controls, and readable readiness labels at narrow widths.

Project headings retain checkout context, with compact tree branches grouping
their services. Healthy operational details live in the project view; failures
remain visible in the sidebar. Coverage includes exact project selection,
selection styling during hover, failure-detail navigation, and expanding the
initially collapsed Recent section.

Address-popover coverage exercises keyboard opening, Escape/focus restoration,
pointer/light dismissal, narrow-screen bounds, and exact URL copying. Clipboard
success and failure are simulated only on the owned fixture page: the tests do
not read or write the user's clipboard. The compact Open link retains its native
one-click destination behavior independently of the details popover.

The Dashboard Browser CI job runs this isolated suite on Linux and macOS for
dashboard, fixture, or workspace dependency changes. Browser failures retain
screenshots and traces as CI artifacts.
