# Phase 58 CLI DDD Audit (Task 58.1)

Date: 2026-01-28  
Scope: locald CLI (crates/locald-cli)

## Command Inventory

**Core**

- `init` — initialize a new project (locald.toml scaffold).
- `run` — run an ad‑hoc command, prompt to save.
- `exec` / `x` — run a task in a service context.
- `add exec` — shortcut for adding an exec service.
- `add` — add service definitions.
- `remove` — stop/wipe/restart a service.
- `up` — start daemon and register project.
- `ps` — list running services.
- `logs` — stream logs (optional follow).
- `stop` / `restart` — stop or restart services.
- `monitor` — TUI monitor.
- `ping` — ping the daemon.
- `trust` — install Root CA into system trust store.
- `daemon` — manage daemon.
- `selfupgrade` — check/install new versions.
- `dashboard` — open dashboard.
- `doctor` — diagnostics (supports JSON).
- `config` — show configuration (supports provenance).
- `registry` — registry management.
- `schema` — schema/context output.
- `port` — show process bound to port.
- `serve` — serve a directory via HTTP.

**Hidden / Internal**

- `__surface` — CLI surface manifest (JSON).

**Nightly-only / Experimental**

- `buildpack` — buildpacks (experimental-cnb).
- `container` — container run (experimental-containers).
- `plugin inspect` (experimental-plugins).
- `plugin list` (experimental-plugins).

## Friction Points (by Persona)

### New Hire

- `serve --bind` is documented but ignored in the handler, which will confuse users expecting it to work.  
  References: `crates/locald-cli/src/commands/serve.rs`, `ServeArgs.bind`
- `trust` help doesn't mention privilege requirements; guidance appears only after a permission error.  
  References: `crates/locald-cli/src/commands/trust.rs`, help text
- `config` prints global server config only, but the command name does not clarify global vs project scope.  
  References: `crates/locald-cli/src/commands/config.rs`, `run_config()`
- The internal `__surface` command is hidden; new users won't discover the CLI manifest output without doc hints.  
  References: `crates/locald-cli/src/commands/surface.rs`, `#[clap(hide = true)]`

### Security Auditor

- `run` appends raw command strings to a plaintext history file; secrets entered in commands would be stored unredacted.  
  References: `crates/locald-cli/src/commands/run.rs`, `save_to_history()`
- Crash reports include all `LOCALD_` environment variables, which may contain secrets, and write them to disk.  
  References: `crates/locald-cli/src/crash.rs`
- Daemon logs are hard-coded to `/tmp/locald-daemon.log` with no override, which can be overly permissive depending on umask.  
  References: `crates/locald-cli/src/commands/daemon.rs`

### Test Engineer

- Many "Unexpected response" branches only print to stdout and still exit successfully, making tests pass on partial failures.  
  References: `crates/locald-cli/src/commands/*.rs`
- Error handling is inconsistent: some command paths print and call `process::exit()` directly, bypassing miette diagnostics.  
  References: `crates/locald-cli/src/commands/run.rs`, `crates/locald-cli/src/commands/trust.rs`, others
- Exit codes are only explicitly codified for `doctor`; other commands' exit semantics are undocumented.  
  References: `crates/locald-cli/src/commands/doctor.rs`, exit code constants

### SRE

- JSON output is limited to a few commands (`doctor`, `config`, plugin inspect, surface manifest). Operational commands (`ps`, `logs`, `status`) are not machine-readable.  
  References: `crates/locald-cli/src/commands/ps.rs`, `crates/locald-cli/src/commands/logs.rs`, others
- Output color is always enabled (miette + crossterm styling), with no `--no-color` or `NO_COLOR` handling; ANSI escapes may pollute non‑TTY logs.  
  References: `crates/locald-cli/src/main.rs`, `crates/locald-cli/src/output.rs`, miette setup
- Log timestamps in `logs` are HH:MM:SS only, making cross-day correlation difficult.  
  References: `crates/locald-cli/src/commands/logs.rs`

## Recommendations (Prioritized)

**P0**

1. Make `serve --bind` effective or remove the flag; add coverage in CLI surface manifest tests.  
   References: `crates/locald-cli/src/commands/serve.rs`, `ServeArgs`
2. Add redaction/opt‑out for crash logs and command history; warn when writing history.  
   References: `crates/locald-cli/src/crash.rs`, `crates/locald-cli/src/commands/run.rs`, `save_to_history()`

**P1** 3. Standardize error handling to return `miette::Result` and avoid ad‑hoc `process::exit()`; ensure non‑zero exit on "Unexpected response".  
 References: `crates/locald-cli/src/commands/run.rs`, `crates/locald-cli/src/commands/trust.rs`, others 4. Add `--json` output for operational commands (`ps`, `logs`, `restart`, `stop`) and document schema.  
 References: `crates/locald-cli/src/commands/`

**P2** 5. Add `--no-color` and `NO_COLOR` support; disable colors on non‑TTY by default.  
 References: `crates/locald-cli/src/main.rs`, `crates/locald-cli/src/output.rs` 6. Clarify `config` scope and add explicit `--global` / `--project` options.  
 References: `crates/locald-cli/src/commands/config.rs`, help text 7. Update help text to explicitly mention privilege requirements (especially `trust`) and log location.  
 References: `crates/locald-cli/src/commands/trust.rs`, `crates/locald-cli/src/commands/daemon.rs`

## Code References (Issues)

- Serve bind ignored: `crates/locald-cli/src/commands/serve.rs`, `ServeArgs.bind`
- Trust privilege messaging gap: `crates/locald-cli/src/commands/trust.rs`, help text
- Config scope ambiguity: `crates/locald-cli/src/commands/config.rs`, `run_config()`
- Plaintext history storage: `crates/locald-cli/src/commands/run.rs`, `save_to_history()`
- Crash env capture: `crates/locald-cli/src/crash.rs`
- Daemon log path: `crates/locald-cli/src/commands/daemon.rs`
- Unexpected response handled as success: `crates/locald-cli/src/commands/*.rs`
- Inconsistent error handling via `process::exit()`: `crates/locald-cli/src/commands/run.rs`, `crates/locald-cli/src/commands/trust.rs`, others
- Limited JSON output coverage: `crates/locald-cli/src/commands/ps.rs`, `crates/locald-cli/src/commands/logs.rs`
- Color forced / non‑TTY: `crates/locald-cli/src/main.rs`, `crates/locald-cli/src/output.rs`
- Logs lack date/timezone: `crates/locald-cli/src/commands/logs.rs`
