<!-- exo:145 ulid:01krkpxdwqmkfazrkcrej5th1x -->

# RFC 145: Miette Error Handling for CLI

Status: Stage 2 (Draft)

## 1. Summary

Migrate `locald-cli` error handling from `anyhow`-centric reporting to `miette` diagnostics, with structured error types and richer user-facing messages.

## 2. Motivation

The CLI needed consistent, user-friendly diagnostics with clear error codes and actionable help text. The migration to `miette` delivers:

- Structured diagnostics with codes and help text.
- Improved terminal output (color, unicode, terminal links).
- A consistent error surface for CLI and daemon interactions.

## 3. Detailed Design

### Terminology

- **Diagnostic**: A rich error with codes, help text, and formatted output via `miette`.
- **CLI Error**: Top-level error type emitted by `locald-cli`.

### User Experience (UX)

Users see short, formatted diagnostics when commands fail. For fatal errors or panics, the CLI writes a crash report and prints the rendered diagnostic plus a pointer to the log file.

### Architecture

- `CliError` is the top-level error type for the CLI, implementing `miette::Diagnostic`.
- `DaemonError` represents daemon/IPC failures and provides diagnostic codes and help text.
- `ConfigError` is reserved for configuration diagnostics.
- `CliResult<T>` is a type alias for `Result<T, CliError>`.

### Implementation Details

- `locald-cli` installs a global `miette` handler in `main` with color, unicode, and terminal links enabled.
- `CliError` wraps `DaemonError` and `ConfigError` transparently, and provides an `Other` variant for ad-hoc messages.
- Conversions from `anyhow::Error`, `std::io::Error`, `std::env::VarError`, `serde_json::Error`, and `toml` errors map into `CliError::Other` by stringifying the error.
- `DaemonError` provides concrete diagnostics for:
  - daemon not running
  - connection refused
  - permission denied
  - generic connection failure
  - invalid socket environment
  - daemon request failure
- Crash reporting collects environment context and emits a formatted `miette::Report` to a crash log for post-mortem debugging.

## 4. Implementation

What is implemented today:

- `CliError`, `DaemonError`, and `ConfigError` are defined in `locald-cli` and derive `Diagnostic` for `miette` rendering.
- `CliResult<T>` is used across CLI entry points and client calls.
- `main` installs the `miette` hook and wraps error paths with `miette::Report` for crash reporting.
- CLI IPC operations map connection failures into `DaemonError` variants with user-facing help text.

## 5. Implementation Plan (Stage 2)

- [x] Define `CliError`, `DaemonError`, and `ConfigError` with `miette` diagnostics.
- [x] Add `CliResult<T>` and convert CLI fallible paths.
- [x] Install `miette` handler and connect crash reporting to `miette::Report`.

## 6. Context Updates (Stage 3)

- [ ] Create/Update `docs/manual/cli/errors.md`
- [ ] Update CLI architecture docs to reflect diagnostic codes and crash logging

## 7. Drawbacks

- Error conversions from `anyhow` lose structured context and may hide underlying causes.
- Diagnostics require additional code/attributes for best UX.

## 8. Alternatives

- Continue using `anyhow` with manual formatting and ad-hoc error strings.
- Adopt a different diagnostic library (e.g., `eyre`) without codes/help text.

## 9. Unresolved Questions

- How much structured context should be preserved when converting from `anyhow` errors?
- Should `ConfigError` be expanded to include specific config diagnostic codes?

## 10. Future Work

- Improve `IpcError` conversions so they preserve full context instead of collapsing to a single diagnostic.
