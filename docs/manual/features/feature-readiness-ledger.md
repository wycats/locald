# Feature Readiness Ledger

This document is a living ledger that maps each **user-facing feature** to:

- a **stability label** (Stable / Experimental / Hidden / Removed)
- owning **RFC(s)** (or “no RFC yet”)
- the **canonical docs location** (or “not documented”)

The goal is to prevent drift between:

- what the system does,
- what docs teach,
- what RFCs claim.

## Stability labels

- **Stable**: taught as the default workflow; expected to work; docs + RFC are kept in sync.
- **Experimental**: shipped but may change; docs must label as such; drift is tolerated but should be tracked.
- **Hidden**: exists for contributors/tests/CI; not taught as a user workflow.
- **Removed**: previously taught or proposed, but intentionally not present in the current surface.

## Ledger

| Feature | Stability | Owning RFC(s) | Canonical docs location | Notes / drift guard |
| --- | --- | --- | --- | --- |
| Core CLI entrypoint (`locald`) | Stable | docs/rfcs/stage-0/0112-user-programming-model-audit-and-doc-plan.md | locald-docs: reference/cli | CLI surface is enforced via a manifest (Phase 111); docs should not invent commands. |
| Project bring-up (`locald up`) | Stable | docs/rfcs/stage-0/0112-user-programming-model-audit-and-doc-plan.md | locald-docs: reference/cli + getting-started | Treat as the canonical “start” verb; avoid teaching legacy `start`. |
| Service lifecycle (`stop`, `restart`, `status`, `logs`) | Stable | docs/rfcs/stage-0/0112-user-programming-model-audit-and-doc-plan.md | locald-docs: reference/cli | If the surface changes, update CLI manifest + docs in the same PR. |
| Dashboard (`locald dashboard`, `http://locald.localhost`) | Stable | docs/rfcs/0031-dashboard-stack.md; docs/rfcs/0087-cybernetic-dashboard.md | locald-docs: concepts/workspace + reference/cli | UI vocabulary must remain aligned with Surface Contract (RFC 0114). |
| TUI monitor (`locald monitor`) | Experimental | docs/rfcs/0016-cli-tui-monitor.md | locald-docs: reference/cli | Keep as optional; avoid letting it define canonical nouns/verbs. |
| Docs site (`http://docs.localhost`) | Stable | docs/rfcs/0103-docs-site-language-and-persona-routing.md | locald-docs: (this site) | Docs are a taught surface; broken links should fail CI. |
| Configuration file (`locald.toml`) | Stable | docs/rfcs/0026-configuration-hierarchy.md | locald-docs: reference/locald-toml | Treat schema + examples as canonical; avoid stale manual snippets. |
| Host execution (default) | Stable | docs/rfcs/0069-host-first-execution.md (if present) or no RFC yet | locald-docs: reference/execution-modes | Keep “host-first” as default story; avoid over-indexing on containers. |
| Service type: exec/worker | Stable | no RFC yet | locald-docs: reference/service-types | These are core; keep docs consistent with actual config keys. |
| Managed Postgres service | Stable | docs/manual/features/managed-data-services.md; (RFCs: none canonical) | locald-docs: guides/managed-services | Treat storage paths + connection string surface as contract (avoid invented passwords/paths). |
| Health checks | Stable | docs/manual/features/process-types.md; (RFCs: none canonical) | locald-docs: reference/health-checks | Keep default behavior and fallbacks documented accurately. |
| Service URLs & routing (`*.localhost`) | Stable | docs/rfcs/0024-default-domain-localhost.md | locald-docs: guides/dns-and-domains + reference/service-types | `.localhost` is canonical; avoid `.local`/other suffixes in taught docs. |
| Trust / local CA (`locald trust`) | Stable | no RFC yet | locald-docs: guides/dns-and-domains | If cert paths change, update docs and `doctor` output together. |
| Privileged setup (`sudo locald admin setup`) | Stable | docs/rfcs/stage-1/0110-privileged-capability-acquisition.md | locald-docs: reference/cli | This is the canonical remediation for privileged readiness problems. |
| Doctor readiness report (`locald doctor`) | Stable | docs/rfcs/stage-1/0109-locald-doctor.md; docs/rfcs/stage-1/0110-privileged-capability-acquisition.md | locald-docs: reference/cli | Problem IDs should remain stable; add new checks as Warning unless truly blocking. |
| Optional integrations matrix | Stable | docs/rfcs/stage-0/0114-surface-contract-program-keep-ux-coherent-.md | locald-docs: reference/integrations | Matrix defines warn-vs-critical policy for integrations. |
| CNB builds (`locald build`, `[services.*.build]`) | Experimental | docs/rfcs/0049-composable-cnb-library.md; docs/rfcs/0056-extract-build-environment.md | locald-docs: reference/builds | Experimental until the failure modes and surfaces are fully stabilized. |
| OCI container execution (`type = "container"`) | Stable | docs/rfcs/0048-rust-oci-registry.md (registry); (runtime RFC: no single canonical) | locald-docs: reference/locald-toml + reference/service-types | Does not require Docker; relies on privileged shim + registry access. |
| Legacy Docker daemon integration | Experimental / Legacy | docs/manual/features/docker-integration.md; docs/rfcs/0079-unified-service-trait.md | locald-docs: reference/integrations | Must not be required for the default container story. |
| Sandbox mode (`--sandbox`) | Hidden | docs/rfcs/stage-0/0112-user-programming-model-audit-and-doc-plan.md | not documented (user-facing) | Keep under CI/contributor docs unless promoted deliberately. |
| Daemon lifecycle (`locald server start/shutdown`) | Hidden | no RFC yet | not documented (user-facing) | Keep as contributor/CI surface; don’t teach as the primary workflow. |
| Removed: `locald down` | Removed | docs/rfcs/stage-0/0112-user-programming-model-audit-and-doc-plan.md | not documented | Do not reintroduce into docs until implemented (or defined as an alias). |

## Realignment notes (Phase 114)

- This ledger intentionally prefers **conservative stability**. If a surface is drifting, it should not be labeled Stable until the docs + behavior are aligned.
- Where there is “no RFC yet”, the ledger is the forcing function: either create/promote an RFC, or explicitly decide the feature does not need one.
