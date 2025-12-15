# Implementation Plan - Phase 33: Ad-Hoc Execution

**Goal**: Improve the workflow for running one-off commands and converting successful experiments into permanent configuration, aligning with Heroku terminology.

## User Experience Changes

1.  **`locald try <command>` (New)**

    - **Purpose**: Experiment with a command to see if it works before adding it to `locald.toml`.
    - **Behavior**: Runs the command in the current directory. On exit, prompts the user to save it as a service.
    - **History**: Successful runs are saved to a history file for easy recall.

2.  **`locald run <service> <command>` (Breaking Change)**

    - **Purpose**: Run a one-off task within the context of an existing service (e.g., database migrations, consoles).
    - **Behavior**: Connects to the daemon to fetch the environment variables for `<service>`, then executes `<command>` locally with that environment injected.
    - **Note**: This replaces the old "run raw command" behavior of `locald run`.

3.  **`locald add last`**
    - **Purpose**: Quickly add the last successful `locald try` command to the configuration.

## Technical Components

### 1. IPC Protocol Update

- Add `GetServiceEnv { name: String }` message to `locald-core`.
- Implement handler in `locald-server` to compute and return the full environment map for a service (including injected dependencies).

### 2. CLI Refactor (`locald-cli`)

- **Rename**: Move existing `run` logic to `try` subcommand.
- **New `run`**: Implement new `run` subcommand that:
  1.  Accepts a service name and a command.
  2.  Calls `GetServiceEnv` via IPC.
  3.  Executes the command with the returned environment.
- **History**: Implement a simple history mechanism (text file in `~/.local/share/locald/history`) for `try` commands.
- **Add Last**: Implement `locald add last` to read from history and trigger the "add service" flow.

## Migration

- Since we are accepting breaking changes, `locald run <cmd>` will now fail if `<cmd>` is not a service name. We will provide a helpful error message: "Did you mean `locald try <cmd>`?".

## User Verification

- [ ] **Try Command**: Run `locald try "python3 -m http.server"` and verify it runs.
- [ ] **Add Service**: Verify the prompt to add the service after `locald try` exits.
- [ ] **Run Task**: Run `locald run <service> env` and verify it sees the service's environment variables.
- [ ] **History**: Run `locald add last` and verify it suggests the last `try` command.
