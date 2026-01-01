# Process Group Handling Analysis: Linux vs macOS

**Status**: Research Complete  
**Date**: 2025-12-31  
**Risk Level**: LOW (with architectural caveat)

---

## Executive Summary

After thorough analysis of the codebase, **process group handling is NOT a high-risk area for macOS support**. The implementation uses standard POSIX APIs through the `nix` crate, which abstracts platform differences correctly. However, there is an **architectural caveat**: the current macOS strategy (per RFC 0061) runs all processes inside a Lima VM, meaning the process group handling code will execute in a Linux context even on macOS.

---

## 1. Current Architecture

### 1.1 Process Spawning

Processes are spawned via `portable-pty` crate, which uses platform-native PTY implementations:

**File**: [crates/locald-server/src/runtime/process.rs#L46-L56](crates/locald-server/src/runtime/process.rs#L46-L56)

```rust
fn create_pty() -> Result<portable_pty::PtyPair> {
    let pty_system = NativePtySystem::default();
    pty_system.openpty(PtySize { rows: 24, cols: 80, ... })
}
```

When `portable-pty` spawns a command on Unix systems (Linux and macOS), it:

1. Calls `libc::openpty()` to create master/slave PTY pair
2. Uses `setsid()` in the child's `pre_exec` hook to make it a **session leader**
3. Sets the PTY as the controlling terminal via `TIOCSCTTY`
4. Resets signal dispositions to `SIG_DFL`

**Key insight**: Since each spawned process becomes a **session leader**, it's also the **process group leader** (PGID == PID).

### 1.2 Process Group Signal Delivery

**File**: [crates/locald-utils/src/process.rs#L20-L49](crates/locald-utils/src/process.rs#L20-L49)

```rust
pub async fn terminate_gracefully(child: &mut Box<dyn Child + Send>, name: &str, signal: Signal) {
    let pid = child.process_id().unwrap();
    let pid_i32 = i32::try_from(pid).unwrap_or(i32::MAX);

    // Send signal to process group (negative PID)
    if let Err(e) = kill(Pid::from_raw(-pid_i32), signal) {
        if e != nix::errno::Errno::ESRCH {
            error!("Failed to send {:?} to {}: {}", signal, name, e);
        }
    }
    // ... wait loop with 5s timeout, then SIGKILL
}
```

The code uses **negative PID** with `nix::sys::signal::kill()`, which is the POSIX idiom for sending a signal to all processes in a process group. This works identically on Linux and macOS.

### 1.3 Shutdown Flow

1. **IPC/Signal triggers shutdown** ([crates/locald-server/src/lib.rs#L423-L435](crates/locald-server/src/lib.rs#L423-L435))
2. **Manager stops all services in parallel** ([crates/locald-server/src/manager.rs#L1409-L1437](crates/locald-server/src/manager.rs#L1409-L1437))
3. **Each controller sends stop signal** ([crates/locald-server/src/service/exec.rs#L219-L241](crates/locald-server/src/service/exec.rs#L219-L241))
4. **Cgroup cleanup (Linux only)** ([crates/locald-server/src/service/exec.rs#L254-L287](crates/locald-server/src/service/exec.rs#L254-L287))

### 1.4 Container Signal Forwarding

For container workloads, the shim forwards signals to the container init process:

**File**: [crates/locald-shim/src/main.rs#L300-L316](crates/locald-shim/src/main.rs#L300-L316)

```rust
let mut signals = signal_hook::iterator::Signals::new([
    SIGTERM, SIGINT, SIGQUIT, SIGHUP,
])?;

std::thread::spawn(move || {
    for sig in signals.forever() {
        unsafe { libc::kill(init_pid_raw, sig); }
    }
});
```

---

## 2. Platform Differences Analysis

### 2.1 APIs Used

| API                             | Linux | macOS | Cross-Platform? |
| ------------------------------- | ----- | ----- | --------------- |
| `nix::sys::signal::kill()`      | ✅    | ✅    | Yes             |
| `kill(negative_pid)` for PGID   | ✅    | ✅    | Yes (POSIX)     |
| `portable_pty::NativePtySystem` | ✅    | ✅    | Yes             |
| `libc::setsid()`                | ✅    | ✅    | Yes (POSIX)     |
| `libc::waitpid()`               | ✅    | ✅    | Yes (POSIX)     |

### 2.2 `nix` Crate Platform Support

The `nix` crate (v0.30.1) fully supports macOS for all functions we use:

- `kill()`, `killpg()` - Available on all Unix platforms
- `Signal` enum - Compatible across platforms
- `Pid` type - Works identically

**Feature flags used** ([crates/locald-utils/Cargo.toml#L10-L15](crates/locald-utils/Cargo.toml#L10-L15)):

```toml
nix = { version = "0.30.1", features = [
  "process", "signal", "socket", "user", "fs",
] }
```

All these features are available on macOS.

### 2.3 `portable-pty` Platform Behavior

The `portable-pty` crate (from wezterm) has identical behavior on Linux and macOS:

- Both use `libc::openpty()` to create PTYs
- Both call `setsid()` to create a new session
- Both set `TIOCSCTTY` for controlling terminal
- Child processes inherit no file descriptors above stderr

### 2.4 Semantic Differences

| Behavior                | Linux                      | macOS   | Impact    |
| ----------------------- | -------------------------- | ------- | --------- |
| `setsid()`              | Creates new session & PGID | Same    | None      |
| `kill(-pgid)`           | Signals all procs in group | Same    | None      |
| Orphan process adoption | PID 1 (systemd/init)       | launchd | See notes |
| Process groups in PTY   | PGID = session leader      | Same    | None      |

**Orphan Handling**: When a parent process dies, orphaned children are re-parented to PID 1 (init/systemd on Linux, launchd on macOS). This is a slight semantic difference, but:

- Our code doesn't rely on this behavior
- We signal the entire process group, so children get signals before becoming orphans

---

## 3. Risk Assessment

### 3.1 What Works Identically

| Feature                           | Risk    | Notes                      |
| --------------------------------- | ------- | -------------------------- |
| Session creation via `setsid()`   | ✅ None | POSIX standard             |
| Signal delivery to process groups | ✅ None | POSIX standard             |
| PTY-based process spawning        | ✅ None | wezterm uses this on macOS |
| Graceful shutdown with timeout    | ✅ None | Pure Rust/POSIX logic      |
| SIGTERM → SIGKILL escalation      | ✅ None | Works identically          |

### 3.2 Linux-Only Features

| Feature                           | Current Status                | macOS Impact              |
| --------------------------------- | ----------------------------- | ------------------------- |
| Cgroup cleanup                    | `#[cfg(target_os = "linux")]` | Compile-time gated        |
| Notify socket (`sd_notify`)       | Linux-only code path          | Already handles non-Linux |
| `locald-shim` container execution | Uses `libcontainer` (Linux)   | VM isolation solves this  |

### 3.3 Architectural Caveat

Per **RFC 0061**, macOS execution uses an **Embedded Lima VM**. This means:

- The `locald` CLI runs on macOS (native binary)
- The `locald-server` daemon runs **inside the Linux VM**
- All process spawning happens in Linux context
- Process group handling code never runs on Darwin kernel

**Implication**: The "cross-platform" concern for process groups is **academic** - the code will always run on Linux.

---

## 4. What Could Actually Break on macOS

### 4.1 If We Ever Run Natively on macOS

If future architecture changes run `locald-server` natively on macOS:

1. **`setsid` command in CLI** ([crates/locald-cli/src/utils.rs#L58](crates/locald-cli/src/utils.rs#L58))

   - macOS **does not have** the `setsid` command by default
   - Fallback code already exists and works

2. **Cgroup cleanup** - Not available on macOS, already gated

3. **Notify socket** - Already handles non-Linux platforms

### 4.2 Edge Cases to Consider

1. **Child ignores SIGTERM**: Current 5-second timeout then SIGKILL handles this correctly on both platforms.

2. **Process forks additional children**:

   - If children are in same process group (normal case): signals reach them
   - If children call `setsid()`: they escape the group, but this is rare in web apps
   - On Linux, cgroup cleanup catches escapees
   - On macOS (if ever native), this could be a leak

3. **Daemon restart with running services**:
   - State file tracks PIDs ([crates/locald-server/src/manager.rs#L600-L625](crates/locald-server/src/manager.rs#L600-L625))
   - Cleanup attempts SIGTERM on old PIDs
   - Works identically on both platforms

---

## 5. Recommendations

### 5.1 No Changes Needed

The current implementation is sound. The `nix` crate and `portable-pty` handle platform differences correctly.

### 5.2 If Native macOS Becomes a Goal

If we ever want to run `locald-server` directly on macOS (not in VM):

1. **Replace `setsid` command usage**

   ```rust
   // Instead of shelling out to `setsid`
   #[cfg(unix)]
   unsafe { libc::setsid(); }  // Works on Linux and macOS
   ```

2. **Add macOS-specific leak detection**

   - Use `sysctl` or `ps` to find orphaned locald processes
   - Track by environment variable marker (e.g., `LOCALD_SERVICE=project:service`)

3. **Consider process tracking file**
   - Write PID to `.locald/services/<name>/pid`
   - On restart, kill any surviving processes

### 5.3 Test Strategy

**Current E2E tests are sufficient** for the Lima-based macOS architecture. If native macOS becomes a goal:

| Test Case                    | Current Coverage         | macOS Native      |
| ---------------------------- | ------------------------ | ----------------- |
| Basic start/stop             | ✅ `daemon_lifecycle.rs` | Same              |
| Service with child processes | ⚠️ Implicit only         | Add explicit test |
| Service ignores SIGTERM      | ⚠️ `signal-test` example | Promote to E2E    |
| Daemon restart recovery      | ⚠️ Manual testing        | Add E2E test      |
| Multiple services shutdown   | ✅ Implicit              | Same              |

---

## 6. Conclusion

**Process group handling is NOT a blocking concern for macOS support.**

The implementation uses standard POSIX APIs that work identically on Linux and macOS. The current architectural decision to run inside a Lima VM means this code will always execute in a Linux context, making macOS-specific behavior irrelevant.

If future plans include running `locald-server` natively on macOS, minimal changes would be needed, and those changes are well-understood.

### Summary Table

| Concern                 | Risk Level | Action                          |
| ----------------------- | ---------- | ------------------------------- |
| `kill(-pgid)` semantics | 🟢 None    | No change                       |
| PTY session creation    | 🟢 None    | No change                       |
| Graceful shutdown       | 🟢 None    | No change                       |
| Cgroup cleanup          | 🟢 None    | Already gated                   |
| Orphan process escape   | 🟡 Low     | Monitor if native macOS planned |
| `setsid` command        | 🟡 Low     | Fallback exists                 |
