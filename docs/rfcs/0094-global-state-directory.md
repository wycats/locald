# Global State Directory

- **Status**: Stage 3 (Recommended)
- **Date**: 2025-12-11
- **Author**: GitHub Copilot

## Context

`locald` needs to store ephemeral state for each project, including:

- Build artifacts (CNB layers, OCI layouts).
- Container bundles (rootfs, config.json).
- Crash logs.
- Temporary files.

Previously, this state was stored in a `.locald/` directory within the project root.

## Problem

Storing state in the project root causes several issues:

1.  **File Watcher Loops**: Many development tools (IDEs, test runners, hot-reloaders) watch the project directory for changes. Writing build artifacts to `.locald/` can trigger these watchers, leading to infinite build loops or high CPU usage.
2.  **Source Pollution**: It clutters the user's workspace with files that are not part of the source code.
3.  **Gitignore Management**: Users must remember to add `.locald/` to their `.gitignore`, or `locald` must attempt to manage it automatically (which is intrusive).
4.  **Accidental Commit**: If `.gitignore` is misconfigured, large build artifacts might be committed to the repository.

## Solution

We will move the project-specific state to a global location, outside the project source tree.

### Path Strategy

The state directory will be located at:

```
$XDG_DATA_HOME/locald/projects/<project-name>-<hash>/.locald
```

- **`XDG_DATA_HOME`**: Typically `~/.local/share` on Linux. This respects the XDG Base Directory specification.
- **`<project-name>`**: The directory name of the project (for human readability).
- **`<hash>`**: A SHA-256 hash of the **absolute path** to the project root. This ensures that two projects with the same name but different locations do not collide.
- **`.locald`**: The directory must end with `.locald` to satisfy the security constraints of `locald-shim` (which allows privileged operations only within directories named `.locald`).

### Example

For a project at `/home/user/code/my-app`:

```
/home/user/.local/share/locald/projects/my-app-a1b2c3d4/.locald
```

### Implementation Details

1.  **`locald-utils`**: A new module `project` will provide a `get_state_dir(path: &Path) -> PathBuf` function.
2.  **`locald-server`**: Will use this function to determine where to store container bundles and build artifacts.
3.  **`locald-cli`**: Will use this function for `locald build` output and crash logs.
4.  **`locald-shim`**: No changes required. It already validates that the target path contains `.locald`.

## Benefits

- **Clean Workspace**: No more `.locald` folder in the project root.
- **Watcher Safety**: Build artifacts are outside the watch scope of most tools.
- **Zero Config**: No need to touch `.gitignore`.
- **Security**: Maintains the `locald-shim` security model.

## Migration

Existing `.locald` directories in projects can be manually deleted by the user. `locald` will automatically create the new global directory on the next run.
