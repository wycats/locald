<!-- exo:37 ulid:01krkpxdvwfs6xxrk3b6z03m05 -->

# RFC 37: Sandbox Environments: Explicit Isolation


# RFC: Sandbox Environments: Explicit Isolation

## 1. Summary

Implement `--sandbox <NAME>` to isolate environments for testing and development.

Sandbox mode is the **only sanctioned way** to run `locald` without full privileged helper access.

## 2. Motivation

Testing `locald` shouldn't break the user's real setup.

Additionally, some users may want to experiment with `locald` before committing to the full privileged setup. Sandbox mode provides a path for this—but with explicit opt-in rather than silent degradation.

## 3. Detailed Design

Isolate `XDG_*` dirs. Panic if `LOCALD_SOCKET` is set without sandbox.

### Terminology

- **Sandbox**: An isolated environment with separate state (config, data, socket).

### Sandbox Mode and Privileges

Sandbox mode is for **testing and isolation**, not for running production workloads without privileges.

**Key behaviors in sandbox mode:**

- Shim verification is skipped (allows testing without `sudo locald admin setup`)
- State is isolated to `~/.local/share/locald/sandboxes/<name>/`
- Custom socket path prevents conflicts with main locald

**What sandbox mode is for:**

- ✅ Testing changes without affecting real state
- ✅ Running multiple isolated locald instances in parallel
- ✅ Experimenting with locald before full setup
- ✅ CI/CD environments without privileged access

**What sandbox mode is NOT for:**

- ❌ Running production dev environments (use full setup for that)
- ❌ A "lite mode" with silent feature degradation

### Error-Not-Warning Policy

Outside of sandbox mode, `locald` requires privileged helper access. If the shim is unavailable:

- `locald up` will **error with actionable guidance**, not silently continue
- Users are guided to either fix the setup or use sandbox mode explicitly
- There is no "degraded mode" that silently skips features

This ensures users are never confused about why features (hosts sync, cgroup isolation, privileged ports) aren't working.

### User Experience (UX)

Safe testing with explicit opt-in.

```bash
# Test locald without any privileged setup
locald --sandbox test up

# Run multiple isolated instances
locald --sandbox project-a up
locald --sandbox project-b up
```

### Architecture

Startup logic checks for sandbox environment variables.

### Implementation Details

Env var manipulation.

## 4. Drawbacks

- Complexity in tests.

## 5. Alternatives

- Docker (slow).
- Silent "degraded mode" (rejected: confusing to users, features silently broken).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
