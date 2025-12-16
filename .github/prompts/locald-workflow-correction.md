# Anti-Pattern: Manual Process Management via Shell

**Context**: You are likely trying to run a test or verification step involving the `locald` daemon.

**The Anti-Pattern**:
Do **NOT** use shell constructs to manage the daemon's lifecycle. Avoid patterns like:

- Running `server start &` in the background.
- Capturing PIDs (`PID=$!`).
- Using `sleep` to wait for startup.
- Manually `kill`ing processes.

**Why**:
`locald` is designed as a persistent, self-managing background service. It has built-in logic for:

1.  **Auto-Starting**: Clients automatically spawn the daemon if it's not running.
2.  **Readiness Checks**: Commands like `up` wait for the socket to be ready.
3.  **Graceful Shutdown**: `server shutdown` ensures clean state saving and resource release.
4.  **Hot Swapping**: `locald up` detects version changes and automatically restarts the daemon if the binary has changed.

**The Correct Workflow**:

Instead of:

```bash
# BAD: Fragile shell scripting
cargo run ... server start > log 2>&1 &
PID=$!
sleep 5
cargo run ... up examples/foo
kill $PID
```

Use the native commands:

```bash
# GOOD: Native lifecycle management
# 1. Just run 'up'. It handles starting, waiting, and registering.
cargo run ... up examples/foo

# 2. (Optional) Shutdown cleanly if you really need to.
# Usually, you can just leave it running for the next command.
cargo run ... server shutdown
```

**Rule**: Trust the tool's lifecycle management. If you find yourself writing a shell script to manage PIDs, you are doing it wrong.
