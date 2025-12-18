# locald

`locald` is a local development platform: it runs and orchestrates your project services (processes and containers), provides stable local domains + HTTPS, and exposes a dashboard for observability.

This repo is a Rust workspace with a small privileged helper (`locald-shim`) and several supporting packages (dashboard, docs, e2e harness).

Contributing guide: see `CONTRIBUTING.md`.

## Start here

- **CLI (user entrypoint):** `locald-cli/`
- **Daemon / orchestration:** `locald-server/`
- **Core config + types:** `locald-core/`
- **Privileged shim (setuid root):** `locald-shim/` (security-critical)
- **Utilities:** `locald-utils/`
- **Dashboard (Svelte):** `locald-dashboard/`
- **Docs site:** `locald-docs/`
- **E2E harness:** `locald-e2e/`

Design/architecture references:

- `docs/design/` (axioms, vision, architecture)
- `docs/rfcs/` (decision history)

## Validate like CI

CI is the contract. These commands are intended to mirror what GitHub Actions runs (see `.github/workflows/ci.yml`).

### Rust

Fast, developer-friendly (no sudo, no `locald-e2e`):

- `./scripts/ci-rust-checks-local.sh`

CI-like (includes installing the privileged shim + running `locald-e2e`; requires sudo):

- `LOCALD_PREPUSH_FULL=1 ./scripts/ci-rust-checks-local.sh`

### Web (Dashboard + Docs)

- `pnpm -C locald-dashboard install --frozen-lockfile && pnpm -C locald-dashboard build`
- `pnpm -C locald-docs install --frozen-lockfile && pnpm -C locald-docs build`

### Convenience

- `./scripts/check` runs a fast sanity pass (Rust build + clippy, docs checks/build, and a basic IPC ping via a sandboxed daemon).

## Privileged shim notes

Some features require a privileged shim installed with setuid root permissions.

- Install/repair: `sudo locald admin setup`
- Diagnose readiness: `locald doctor`

The shim is treated as an internal protocol surface; `locald` will instruct you when the installed shim is outdated.
