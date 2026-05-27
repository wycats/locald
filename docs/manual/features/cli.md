# CLI Reference

This manual page intentionally does **not** duplicate the user-facing CLI reference.

The canonical public CLI reference lives at `locald-docs/src/content/docs/reference/cli.md`. Keep command descriptions, taught examples, and stability labels there so the docs site has one source of truth.

For the actual command surface, prefer generated/help-backed evidence:

- `locald --help` and subcommand `--help` output
- `crates/locald-cli/tests/cmd/docs-cli.md` for checked help snapshots
- `docs/manual/features/feature-readiness-ledger.md` for canonical docs-location and stability decisions

If the CLI surface changes, update the command implementation, help snapshots, and `locald-docs/src/content/docs/reference/cli.md` together. Do not reintroduce a second hand-maintained CLI reference in this manual.
