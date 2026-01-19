# locald doctor

The `locald doctor` command diagnoses host readiness for running `locald` and prints actionable fixes.

## Usage

```bash
# Human-readable output
locald doctor

# Machine-readable JSON output (for CI/automation)
locald doctor --json

# Verbose mode with extra debugging details
locald doctor --verbose
```

## Exit Codes

- **0**: All critical checks pass
- **Non-zero**: One or more critical checks failed

## What It Checks

### Shim Readiness (Critical)

- Can we locate a privileged shim using strict discovery rules?
- Is it root-owned and setuid?
- Does the shim version match what `locald` expects?
- Can the shim actually perform privileged work? (smoke test)

### Socket Connectivity (Critical in Container Environments)

When running inside a container (Toolbx, Distrobox, Docker):

- Is the shim socket available at `~/.locald/shim.sock`?
- Can we successfully connect and communicate with the daemon?
- Is the protocol version compatible?

### Cgroup Readiness (Critical for Cleanup)

- Is cgroup v2 available?
- Is the locald cgroup root present?
  - Systemd strategy: `/sys/fs/cgroup/locald.slice`
  - Direct strategy: `/sys/fs/cgroup/locald`

### Runtime Basics (Non-critical)

- Is the daemon reachable?
- Sandbox mode identity (if applicable)

## Output Explained

### Privilege Mode

The doctor reports which privilege mode is in use:

| Mode     | Description                                          |
| -------- | ---------------------------------------------------- |
| `setuid` | Using the setuid shim binary directly (host)         |
| `socket` | Using the shim daemon socket (container environment) |

### Cleanup Mode

| Mode       | Description                                              |
| ---------- | -------------------------------------------------------- |
| `enabled`  | cgroup-based cleanup available; reliable process killing |
| `degraded` | PID-only cleanup; may not reliably kill subprocess trees |

### Common Problems

#### `shim.not_found`

The shim binary could not be located.

**Fix**: Run `sudo locald admin setup`

#### `shim.not_setuid`

The shim exists but is not setuid root.

**Fix**: Run `sudo locald admin setup`

#### `shim.version_mismatch`

The shim version doesn't match the `locald` binary.

**Fix**: Run `sudo locald admin setup` to reinstall the shim

#### `socket.unavailable`

The shim daemon socket is not available. This is common inside containers when the host daemon isn't running.

**Fix**:

1. Start the daemon on the host: `sudo locald-shim serve`
2. Or configure `host_exec` in your config to enable auto-start

#### `socket.connection_failed`

The socket exists but connection failed.

**Fix**:

1. Check if the daemon is running: `cat ~/.locald/shim.pid`
2. Restart the daemon: `rm ~/.locald/shim.sock && sudo locald-shim serve`

#### `socket.permission_denied`

Connected to socket but UID validation failed.

**Fix**: Ensure the daemon was started by the same user (or root on behalf of that user)

#### `cgroup.v2_unavailable`

cgroup v2 is not available on this system.

**Impact**: Cleanup mode will be `degraded`

#### `cgroup.root_not_ready`

The locald cgroup root doesn't exist.

**Fix**: Run `sudo locald admin setup`

## JSON Output Schema

When using `--json`, the output follows this structure:

```json
{
  "strategy": {
    "cgroup_root": "systemd",
    "why": "systemd is PID 1"
  },
  "mode": "enabled",
  "problems": [
    {
      "id": "shim.not_found",
      "severity": "critical",
      "status": "fail",
      "summary": "Privileged shim not found",
      "details": "...",
      "remediation": ["sudo locald admin setup"],
      "fix": "run_admin_setup"
    }
  ],
  "fixes": [
    {
      "key": "run_admin_setup",
      "summary": "Install privileged components",
      "commands": ["sudo locald admin setup"]
    }
  ]
}
```

### Strategy Object

- `cgroup_root`: Either `"systemd"` or `"direct"`
- `why`: Human-readable explanation of strategy selection

### Problem Object

- `id`: Stable string identifier (e.g., `shim.not_found`, `socket.unavailable`)
- `severity`: `"critical"`, `"warning"`, or `"info"`
- `status`: `"pass"`, `"fail"`, or `"skip"`
- `summary`: One-line description
- `details`: Optional longer explanation
- `remediation`: List of commands to fix the problem
- `fix`: Optional reference to a consolidated fix

### Fix Object

When multiple problems share the same fix (e.g., both shim issues and cgroup issues are fixed by `admin setup`), they reference a consolidated fix:

- `key`: Stable identifier (`run_admin_setup`, `host_policy_blocks_privileged_helper`, `unsupported_environment`)
- `summary`: What the fix does
- `commands`: Exact commands to run

## Container-Specific Behavior

When running inside a container, `locald doctor` adapts its checks:

1. **Socket-first**: Checks for socket connectivity before checking setuid shim
2. **Container detection**: Reports that you're in a container environment
3. **Actionable guidance**: Provides host-specific instructions

Example output in a container without shim daemon:

```
┌  locald doctor
│
◆  Container environment detected (Toolbx)
│
├  Privilege Mode: unavailable
│
├  Problems:
│  ✗ socket.unavailable (critical)
│    The shim daemon socket is not available.
│    Start the daemon on your host:
│      sudo locald-shim serve
│
└  1 critical issue found
```

## Integration with CI

Use `locald doctor --json` in CI pipelines to gate deployments:

```bash
# Exit non-zero if not ready
locald doctor --json || exit 1

# Or parse the JSON for more control
if ! locald doctor --json | jq -e '.mode == "enabled"' > /dev/null; then
    echo "locald is in degraded mode"
    exit 1
fi
```

## See Also

- [Container Development Environments](../development/container-environments.md) - Full container setup guide
- [Shim Management](../architecture/shim-management.md) - Deep dive on the shim architecture
