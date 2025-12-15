---
title: "Ad-Hoc Execution & History"
stage: 3 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Ad-Hoc Execution
---

# RFC 0032: Ad-Hoc Execution & History

## 1. Summary

This RFC proposes splitting the current `locald run` command into two distinct workflows to better align with user intent and industry standards (Heroku):

1.  **`locald try`**: For experimenting with commands and optionally saving them as services ("Draft Mode").
2.  **`locald run`**: For executing one-off tasks within the context of an existing service ("Task Mode").

It also introduces a history mechanism to easily recall and save successful `try` commands.

## 2. Motivation

The current `locald run` command is overloaded. It is used both for:

1.  Testing a new command to see if it works before adding it to `locald.toml`.
2.  Running a one-off task (like a migration) that needs the environment of a running service.

Furthermore, the term `run` in the PaaS ecosystem (specifically Heroku) strongly implies "run a command inside a dyno/container". Our current usage conflicts with this mental model.

By separating these concerns, we can optimize the UX for each:

- **Drafting** needs quick feedback and an easy path to persistence.
- **Tasks** need environment injection and usually don't need to be saved.

## 3. Detailed Design

### 3.1 `locald try <command>` (Draft Mode)

This command replaces the previous behavior of `locald run`.

**Usage**:

```bash
$ locald try "python -m http.server 8080"
```

**Behavior**:

1.  Executes the command as a child process of the CLI in the current directory.
2.  Does **not** inject any special environment variables from the daemon (unless we add a `--context` flag later).
3.  On exit (success or failure), prompts the user:
    ```text
    Command exited with status: 0.
    Do you want to save this as a service in locald.toml? [Y/n]
    ```
    (Defaults to **Yes** to align with Axiom 1: Zero-Friction Start).
4.  Appends the command to a history file (`~/.local/share/locald/history`) (Aligns with Axiom 7: Persistent Context).

### 3.2 `locald run <service> <command>` (Task Mode)

This command aligns with `heroku run`.

**Usage**:

```bash
$ locald run backend -- rails db:migrate
```

**Behavior**:

1.  Connects to the `locald` daemon via IPC.
2.  Requests the **Runtime Environment** for the specified service (`backend`).
    - This includes variables defined in `locald.toml`, `.env` files, and injected dependencies (e.g., `DATABASE_URL`).
3.  Executes `<command>` locally (client-side) with the fetched environment variables injected.
4.  Does **not** prompt to save on exit.

**Breaking Change**:

- `locald run <command>` (without a service) will now error if `<command>` is not a known service.
- Error Message: `Error: 'python' is not a known service. Did you mean 'locald try python ...'?`

### 3.3 History & Recall

Users often run a command, verify it works, but skip the "Save" prompt, only to realize later they wanted to keep it.

**Command**: `locald add last`

**Behavior**:

1.  Reads the last entry from `~/.local/share/locald/history`.
2.  Initiates the "Add Service" workflow (prompts for name, adds to `locald.toml`).

## 4. Implementation Details

### IPC Protocol

- New Message: `GetServiceEnv { name: String }` -> `Result<HashMap<String, String>, Error>`.
- The daemon must be able to compute the environment for a service without spawning it.

### CLI Structure

- `locald-cli/src/command/try.rs`: Handles the drafting logic.
- `locald-cli/src/command/run.rs`: Handles the task execution logic.
- `locald-cli/src/history.rs`: Manages the append-only history file.

## 5. Future Considerations

- **`locald try --context <service>`**: Allow drafting a new command while borrowing the environment of an existing service.
- **Interactive History**: `locald add --from-history` could show a TUI list of recent commands.

## 6. Implementation Plan

- [x] Rename `locald run` to `locald try` in CLI.
- [x] Implement `locald try` logic (execution + prompt).
- [x] Implement history tracking (`~/.local/share/locald/history`).
- [x] Implement `locald add last`.
- [x] Implement `GetServiceEnv` IPC message in Core and Server.
- [x] Implement `locald run <service> <command>` in CLI.
- [x] Verify with `examples/adhoc-test/verify.sh`.
