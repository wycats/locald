# locald doctor

> **Core focus**: `locald doctor` is the primary recovery tool for the main workflow.

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
- Does the shim version or embedded integrity match what `locald` expects (when provided)?
- Can the shim actually perform privileged work? (smoke test)

### Container Environments

When running inside a container, `locald doctor` reports the environment as **unsupported** and provides guidance to run `locald` on the host instead.

### Cgroup Readiness (Critical for Cleanup)

- Is cgroup v2 available?
- Is the locald cgroup root present?
  - Systemd strategy: `/sys/fs/cgroup/locald.slice`
  - Direct strategy: `/sys/fs/cgroup/locald`

### HTTPS Readiness (Warning)

- Is the local Root CA present (used for HTTPS and trust)?

### Optional Integrations (Verbose Only)

- Is KVM available for VMM-based workflows?

### Platform Support

- On non-Linux platforms, privileged features are marked unsupported.

## Output Explained

### Privilege Mode

The doctor reports the privilege mode in use:

| Mode     | Description                                  |
| -------- | -------------------------------------------- |
| `setuid` | Using the setuid shim binary directly (host) |

### Cleanup Mode

| Mode       | Description                                              |
| ---------- | -------------------------------------------------------- |
| `enabled`  | cgroup-based cleanup available; reliable process killing |
| `degraded` | PID-only cleanup; may not reliably kill subprocess trees |

### Common Problems

#### `shim.present`

The shim binary could not be located.

**Fix**: Run `sudo locald admin setup`

#### `shim.permissions`

The shim exists but is not setuid root.

**Fix**: Run `sudo locald admin setup`

#### `shim.version`

The shim version doesn't match the `locald` binary.

**Fix**: Run `sudo locald admin setup` to reinstall the shim

#### `shim.integrity`

The shim on disk doesn't match the embedded shim bytes.

**Fix**: Run `sudo locald admin setup` to reinstall the shim

#### `shim.usability`

The shim failed its self-check (often due to host policy like SELinux/AppArmor).

**Fix**: Adjust host policy or re-run admin setup

#### `environment.container`

The doctor detected a container environment. The container workflow is no longer supported.

**Fix**: Run `locald` on the host OS. If you need CLI access inside a container, expose the host binary into the container using your container tooling.

#### `cgroup.v2`

cgroup v2 is not available on this system.

**Impact**: Cleanup mode will be `degraded`

#### `cgroup.root_ready`

The locald cgroup root doesn't exist.

**Fix**: Run `sudo locald admin setup`

#### `https.root_ca`

The HTTPS Root CA is missing, so HTTPS may be disabled or untrusted.

**Fix**: Run `locald trust` or `sudo locald admin setup`

#### `integration.kvm` (verbose only)

KVM is unavailable or unusable. VMM features may be disabled.

#### `platform.unsupported`

Non-Linux platforms cannot use privileged features.

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
      "id": "shim.present",
      "severity": "critical",
      "status": "fail",
      "summary": "privileged locald-shim not found",
      "details": "expected locald-shim next to the locald executable",
      "remediation": ["sudo locald admin setup"],
      "evidence": [],
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

- `id`: Stable string identifier (e.g., `shim.present`, `environment.container`)
- `severity`: `"critical"`, `"warning"`, or `"info"`
- `status`: `"pass"`, `"fail"`, or `"skip"`
- `summary`: One-line description
- `details`: Optional longer explanation
- `remediation`: List of commands to fix the problem
- `evidence`: Optional key/value list with collected details
- `fix`: Optional reference to a consolidated fix

### Fix Object

When multiple problems share the same fix (e.g., both shim issues and cgroup issues are fixed by `admin setup`), they reference a consolidated fix:

- `key`: Stable identifier (`run_admin_setup`, `host_policy_blocks_privileged_helper`, `unsupported_environment`)
- `summary`: What the fix does
- `commands`: Exact commands to run

## Container-Specific Behavior

When running inside a container, `locald doctor` reports the environment as unsupported and provides host-only guidance.

Example output:

```
┌  locald doctor
│
◆  Container environment detected
│
├  Problems:
│  ✗ environment.container (critical)
│    locald does not support running inside containers.
│    Run locald on the host OS.
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

- [Container Development Environments](../development/container-environments.md) - Host-first guidance for containerized toolchains
- [Shim Management](../architecture/shim-management.md) - Deep dive on the shim architecture
