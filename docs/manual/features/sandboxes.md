# Sandboxes

> **Core focus**: Sandboxes provide explicit, isolated environments for testing. They are not a production mode.

`locald` sandboxes isolate state so you can experiment without touching your primary setup. Sandboxing is the only supported way to run `locald` without full privileged helper access.

## Purpose

Sandboxes exist to:

- Test changes without affecting real services or configuration
- Run multiple isolated `locald` instances in parallel
- Try `locald` before completing privileged setup
- Use `locald` in CI environments where elevated privileges are unavailable

## Usage

Pass the sandbox name with `--sandbox`:

```bash
# Run an isolated instance named "test"
locald --sandbox test up

# Run multiple isolated instances in parallel
locald --sandbox project-a up
locald --sandbox project-b up
```

## Isolation Guarantees

When sandboxing is active, `locald` ensures:

- **Separate state**: Each sandbox has its own config, data, and runtime state
- **Separate socket**: The IPC socket is namespaced per sandbox
- **Explicit opt-in**: If `LOCALD_SOCKET` is set without `--sandbox`, `locald` errors with guidance

Sandbox mode skips shim verification so you can test without running `sudo locald admin setup`.

## XDG Path Implications

Sandboxed runs rewrite XDG base directories to isolate state under a sandbox root. The root is:

```
~/.local/share/locald/sandboxes/<name>/
```

From that root, `locald` sets per-sandbox defaults like:

- `XDG_CONFIG_HOME`: `<root>/config`
- `XDG_DATA_HOME`: `<root>/data`
- `XDG_STATE_HOME`: `<root>/state`
- `XDG_CACHE_HOME`: `<root>/cache`
- `LOCALD_SOCKET`: `<root>/locald.sock`

These overrides are scoped to the `locald` process and its children, keeping your primary environment untouched.
