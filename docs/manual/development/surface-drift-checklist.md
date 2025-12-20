# Surface Drift Prevention Checklist

This checklist is the lightweight “muscle memory” for keeping the *taught surface* coherent.

Use it whenever a PR changes any user-facing behavior, wording, or CLI surface.

## When you add/change a CLI command

- Update the CLI implementation and regenerate/validate the CLI surface manifest (Phase 111 enforcement).
- Update user docs to match the canonical spelling and flags (avoid teaching aliases as “primary” unless deliberate).
- If the command affects environment readiness, update `locald doctor` output (and its stable IDs) as needed.
- Add/adjust an entry in the Feature Readiness Ledger.

## When you add/change a user-facing concept (noun/verb)

- Check it against the Surface Contract vocabulary (RFC 0114) and existing taught docs.
- Ensure dashboard labels, CLI help text, and docs use the same term for the same concept.
- If a new concept is dashboard-only, either:
  - add it to the contract (if it’s taught), or
  - mark it explicitly experimental/internal and keep it out of front-door docs.

## When you add/change an integration boundary

- Update the Integrations matrix (stable/experimental/legacy + missing behavior + remediation).
- Ensure `locald doctor` severity matches the matrix (warn vs critical).
- Avoid adding optional integration checks that block privileged acquisition.

## When you add/change configuration (`locald.toml`)

- Update canonical configuration docs (reference/locald-toml + service-types as needed).
- Keep example snippets using the real keys/sections (avoid `service.*` vs `services.*` drift).
- If schemas change, ensure any schema-based tooling (AI schema, docs generation, validation) stays consistent.

## When you change storage paths / persistence behavior

- Treat paths surfaced in docs as part of the contract; if a path is not intended to be stable, say so explicitly.
- Update docs that mention persistence (managed services, builds cache, registry paths).
- Ensure `reset/clean` tooling deletes the same paths the runtime actually uses.

## Before merge

- Run `exo verify`.
- Add a walkthrough entry summarizing the user-facing impact.
- Update the phase task list statuses (complete vs pending) via `exo task`.
