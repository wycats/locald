# Axiom 13: The Development Loop

**The Build is Isolated; The Runtime is Interactive.**

To satisfy [Axiom 1: Zero-Friction Start](../experience/01-zero-friction-start.md) and [Axiom 7: Ephemeral Runtime](../architecture/07-ephemeral-runtime.md), `locald` distinguishes between the **Construction** of a service and its **Execution**.

## 1. The Build Phase: Isolation & Reproducibility

When building a service (converting source code into an OCI image), the environment must be **hermetic**.

- **Mechanism**: **Snapshot**.
- **Process**: `locald` copies the source code into a temporary workspace _before_ the build starts.
- **Why**:
  - **Safety**: Prevents the build process from accidentally modifying source files (e.g., `npm install` modifying `package.json` or `node_modules` in a way that breaks the host).
  - **Reproducibility**: Ensures the build depends only on the source at that moment, not on uncommitted or ignored files (unless explicitly included).
  - **Performance**: On systems like Lima or WSL, this snapshot can be moved entirely into the VM's filesystem, making the heavy I/O of a build (compilation, dependency installation) significantly faster than running over a cross-OS mount.

## 2. The Run Phase: Interactivity & Feedback

When running a service for development, the environment must be **permeable**.

- **Mechanism**: **Bind Mount**.
- **Process**: `locald` mounts the live source directory from the host directly into the running container (e.g., at `/workspace`).
- **Why**:
  - **Hot Reloading**: Changes made in the editor must be immediately visible to the running process (e.g., `nodemon`, `flask run --reload`).
  - **Debuggability**: Developers need to inspect the exact state of the code running in the container.

## 3. The Cross-Platform Implication

This distinction is critical for non-Linux platforms (macOS/Lima, Windows/WSL).

- **Builds**: Should happen "inside" the VM. We copy the source _once_ (or sync it), then the heavy I/O happens on the VM's native filesystem.
- **Runs**: Must bridge the gap. We accept the performance penalty of a cross-OS bind mount to gain the interactivity of hot reloading.

## 4. The Conflict (Overlay Mounts)

This duality creates a conflict: The **Run Phase** needs the _source_ from the Host, but the _dependencies_ (e.g., `node_modules`) from the Build (which are Linux-native).

`locald` resolves this by overlaying the build artifacts onto the bind-mounted workspace. (See [RFC 0059](../../rfcs/0059-live-bind-mounts.md)).
