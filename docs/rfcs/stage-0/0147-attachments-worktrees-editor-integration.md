---
title: "Attachments, Worktrees, and Editor Integration"
stage: 0
feature: Architecture / UX
---

# RFC 0147: Attachments, Worktrees, and Editor Integration

**Stage**: 0 (Idea)
**Author**: locald team
**Created**: 2026-04-03

## Summary

This RFC proposes three features:

1. **Attachments**. An attachment records that an editor window, CLI process, or registry pin wants a project running.
2. **Worktree-aware domains**. A worktree can derive its domain from a branch-qualified template in `locald.toml`.
3. **Editor integration protocol**. Editors call plumbing CLI commands to attach, detach, query status, and list projects.

These features make service lifetime follow attachments instead of daemon lifetime. A pinned project continues running without an active editor or CLI attachment.

## Motivation

Today, `locald up` starts services, and the daemon keeps them running until the user stops them or the daemon restarts. Projects remain in the registry. The dashboard shows running, stopped, and orphaned projects without distinguishing currently active work.

This causes two problems:

1. Inactive registered projects continue to consume memory, CPU, and ports.
2. The dashboard retains old projects without a remove flow.

An editor provides an activity signal because it knows which projects are open. Git worktrees need branch-qualified domains so two worktrees of the same repository can run simultaneously.

## Design

### Attachments

An **attachment** is a named reference that indicates a project is open and should remain running.

```text
Attachment {
    project_path: PathBuf,
    source: AttachmentSource,  // Editor("vscode", window_id), CLI(pid), Pin
    created_at: SystemTime,
}
```

Lifecycle rules:

- When the first attachment is created for a project, locald starts services.
- When the last attachment is removed, locald stops services with a graceful shutdown.
- `Pin` is a permanent attachment.
- Multiple attachments to the same project are ref-counted.

Attachment sources:

| Source             | Created when                             | Removed when                           |
| ------------------ | ---------------------------------------- | -------------------------------------- |
| `Editor(name, id)` | Editor opens a folder with `locald.toml` | Editor closes that folder or window    |
| `CLI(pid)`         | `locald up` runs                         | `locald down` or the CLI process exits |
| `Pin`              | `locald registry pin`                    | `locald registry unpin`                |

Dashboard state derives from attachments:

| Section       | Criteria                                    | Actions                                |
| ------------- | ------------------------------------------- | -------------------------------------- |
| **Active**    | Has non-`Pin` attachments                   | Monitor, Stop, Restart, Open in Editor |
| **Always On** | Has `Pin` attachment, no editor attachments | Monitor, Start, Disable                |
| **Recent**    | Zero attachments, still in registry         | Start, Enable, Remove                  |

> Note: Per RFC 0135, the dashboard does not use the term "pin." The registry uses `pin`/`unpin` commands; the dashboard shows this state as "Always On" with "Enable"/"Disable" actions.

### Worktree-aware domains

Configuration in `locald.toml` (lives in the main repo):

```toml
[project]
name = "myapp"
domain = "{{name}}.localhost"

[worktrees]
domain = "{{branch}}.{{project.domain}}"
```

Resolution rules:

1. For a normal git repository (`.git` is a directory), locald uses `[project].domain`.
2. For a git worktree (`.git` is a file), locald uses `[worktrees].domain`, substituting `{{branch}}` with the current branch name.
3. For the default branch, locald uses `[project].domain` without a branch prefix. locald detects the default branch via `git symbolic-ref refs/remotes/origin/HEAD`.
4. If `[worktrees]` is absent, all worktrees use `[project].domain`. Only one can run at a time.

Config source:

Each worktree reads its own working copy of `locald.toml`. Service definitions, environment variables, and all other config can vary per branch. The `[worktrees]` section is config like any other and can be changed on a branch.

Template variables:

| Variable                | Value                                  | Example (`feature/checkout-flow`) |
| ----------------------- | -------------------------------------- | --------------------------------- |
| `{{name}}`              | `[project].name`                       | `myapp`                           |
| `{{branch.last}}`       | Last segment after `/`, sanitized      | `checkout-flow`                   |
| `{{branch.hyphenated}}` | Full branch name, `/` → `-`, sanitized | `feature-checkout-flow`           |
| `{{project.domain}}`    | Resolved `[project].domain`            | `myapp.localhost`                 |

Default `[worktrees].domain` uses `{{branch.last}}`.

Project identity remains `path`. Each worktree has a distinct path, so each worktree is a separate registry entry. The dashboard groups worktrees under the parent repository.

Worktree detection uses `git2`. `Repository::open(path)` handles both normal repos and worktrees. A `.git` file (vs directory) indicates a worktree. `repo.head()?.shorthand()` provides the branch name. `repo.worktrees()` lists all worktrees from the main repo. Default branch detection falls back to treating the branch as non-default if `git symbolic-ref refs/remotes/origin/HEAD` fails (e.g., no remote configured).

### Editor integration protocol

Machine-facing CLI commands for editors, scripts, and the dashboard:

```text
locald project attach <path> [--source editor --editor-name vscode --editor-id <window-id>] [--json]
locald project detach <path> [--source editor --editor-id <window-id>]
locald project start <path>
locald project stop <path>
locald project status <path> --json
locald project list --json [--filter active|pinned|recent|all]
```

Command semantics:

- `attach` registers an attachment and starts services if this is the first attachment.
- `detach` removes an attachment and stops services if this was the last attachment and the project is not pinned.
- `start` and `stop` are explicit overrides that bypass attachment counting.
- `status` returns machine-readable project state (services, ports, domains, attachments).
- `list` returns known projects with attachment state.
- All commands accept `--json` for machine-readable output. TODO(MISSING): Define JSON schema in Stage 1.

The VS Code extension shows a status bar item with service count and health. Clicking the item opens the locald dashboard filtered to the project. The command palette exposes Open Dashboard, Restart Services, and Stop Services. The protocol is editor-agnostic.

### Copilot integration

The VS Code extension exposes locald to Copilot through two mechanisms:

**`chatInstructions`** (stable API). A `contributes.chatInstructions` entry in `package.json` loads when `locald:projectDetected` context key is set. This tells Copilot:

- Services are managed by locald — don't start them manually
- How to query status via `locald project status <path> --json`
- The dev URLs for each running service
- That the integrated browser can reach `*.localhost` domains via HTTPS

**Copilot tool** (proposed API). The extension registers a language model tool (`vscode.lm.registerTool`) that Copilot can invoke directly:

| Tool              | Input                                  | Output                                                   |
| ----------------- | -------------------------------------- | -------------------------------------------------------- |
| `locald_services` | none                                   | List of services with name, status, port, URL, health    |
| `locald_restart`  | `{ service: string }`                  | Restart result                                           |
| `locald_logs`     | `{ service?: string, lines?: number }` | Recent log lines                                         |
| `locald_open`     | `{ service: string }`                  | Opens the service URL in Simple Browser, returns the URL |

The tool enables a workflow where Copilot can make a code change, ask locald for the service URL, open it in the integrated browser, and verify the result visually — all without the user leaving the editor.

Example interaction:

```
User: "Fix the login form validation and make sure it works"
Copilot: [edits the validation code]
Copilot: [calls locald_services to find the web service URL]
Copilot: [calls locald_open to open it in Simple Browser]
Copilot: [uses the browser tool to navigate to /login and test the form]
```

The tool calls the same plumbing CLI commands. The `locald_open` tool uses `vscode.env.openExternal` or `vscode.commands.executeCommand('simpleBrowser.show', url)` to open the URL.

Porcelain compatibility:

`locald up`, `locald stop`, and `locald status` remain human-facing commands.

- `locald up` maps to `locald project attach <path> --source cli` with interactive output (build progress, log streaming). locald records the calling process PID and removes the attachment when it exits.
- `locald stop` maps to `locald project detach <path>` (normal graceful flow), not `locald project stop` (emergency override).
- `locald status` maps to `locald project list` with human-formatted output.

### Dashboard behavior

When a repository has multiple worktrees, the dashboard groups them:

```text
myapp
├─ main          → myapp.localhost                  (active, VS Code)
├─ feat-checkout → feat-checkout.myapp.localhost    (active, VS Code)
└─ hotfix-auth   → hotfix-auth.myapp.localhost      (recent, stopped)
```

The Remove action in Recent stops services if running, removes the project from the registry, and cleans up the state directory.

## Resolved questions

1. **Stale attachment cleanup.** RESOLVED: Editors proactively detach on window close (normal path). The daemon also polls attached PIDs and detaches when a PID disappears (fallback for crashes, force-quit, power loss). PID is stored at attach time. No heartbeat protocol needed.

2. **Worktree-local `locald.toml` changes.** RESOLVED: Each worktree reads its own working copy of `locald.toml`. Service definitions, env vars, etc. can vary per branch — that's what the developer is iterating on. The `[worktrees]` section (domain template) is the part that defines cross-worktree behavior; it comes from whatever copy each worktree has.

3. **Branch name sanitization.** RESOLVED: Default uses the last segment after `/` (e.g., `feature/checkout` → `checkout`). Sanitize for DNS: lowercase, replace `[^a-z0-9-]` with `-`, collapse consecutive hyphens, trim, truncate to 63 chars. Template variables: `{{branch.last}}` (default, last segment) and `{{branch.hyphenated}}` (full name, `/` → `-`). On collision (two simultaneous worktrees resolve to the same domain), fail with an error suggesting the user rename one of the branches.

4. **Port allocation for worktrees.** RESOLVED: Ports are dynamically assigned per service start (OS picks a free port). Each worktree is a separate project (distinct path), so each gets its own ports. No collision risk.

5. **`start`/`stop` vs attachment counting.** RESOLVED: Normal lifecycle flows through attachments: attach starts, detach stops, enable/disable controls persistence. `start` and `stop` are low-level emergency overrides (e.g., runaway process). `stop` force-stops services and sets a "manually stopped" flag. The flag inhibits auto-restart from existing attachments until the next explicit `start` or a new `attach`. For pinned projects, `stop` also sets this flag — the daemon does not auto-restart until `start` or re-pin. The dashboard's primary actions are attach-oriented (Enable, Disable, Remove), not process-oriented (start, stop).

6. **Constellation interaction.** RESOLVED: Workspaces (multiple services in one `locald.toml`) are already implemented and compose naturally — one project, one attachment, all services start/stop together. A worktree of a workspace gets all the same services on a branch-qualified domain. Constellations (cross-repo grouping) are orthogonal future work.

7. **Attachment persistence across daemon restart.** RESOLVED: Attachments persist to disk. On daemon restart, Pin attachments restore unconditionally. Editor and CLI attachments restore, then PID-check immediately — if the source PID is gone, detach. If still alive, keep. This makes daemon restart transparent: editor still open → project comes back up. Editor crashed while daemon was down → stale attachment cleaned up on first poll.

8. **Copilot / agent skill.** RESOLVED: The VS Code extension contributes a `chatInstructions` entry (stable API) that loads when `locald.toml` is detected. This tells Copilot not to start services manually, how to query status via plumbing commands, and where to find the dev URL. Ships as part of the extension, not a separate artifact.

## Drawbacks

- Attachments add lifecycle complexity.
- Worktree detection adds `git2` as a server dependency.
- The plumbing CLI adds commands intended for editor and script use.
- A VS Code extension adds a new maintained artifact.

## Alternatives

1. **No attachments.** Pin and unpin only. Lifecycle management stays manual. Loses editor-driven start/stop.
2. **File watchers.** Watch for lock files or `.git/index.lock` to detect editor activity. Fragile and platform-dependent.
3. **Always-on with resource limits.** Keep all projects running but limit resources. Simpler lifecycle, higher baseline cost.

## Future possibilities

- Gnome/KDE StatusNotifier integration via the tray agent pattern
- Neovim and Zed extensions calling the same plumbing protocol
- `locald workspace` commands for Constellation-level attach/detach
- Warm standby: detached projects stop services but retain cached build artifacts for fast restart

## References

- RFC 0135: Dashboard Vocabulary (canonical terms)
- RFC 0036: Project Registry
- RFC 0116: MAP Scope (vocabulary)
- RFC 0112: User Programming Model Audit
- docs/manual/roadmap/constellations-and-config.md
- docs/design/user-interaction-modes.md
