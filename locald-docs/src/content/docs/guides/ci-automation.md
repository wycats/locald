---
title: CI Automation & Sandboxing
description: Using locald in Continuous Integration environments.
---

`locald` is designed to be used in CI/CD pipelines. Its "Sandbox Mode" allows you to run isolated instances of the daemon that don't interfere with the global user configuration.

## Sandbox Mode

To run `locald` in a sandbox, use the `--sandbox` flag. This tells `locald` to use a separate set of directories for configuration, data, and sockets.

```bash
locald --sandbox=ci-job-123 up
```

This is crucial for CI environments where multiple jobs might be running on the same machine, or when you want to run tests in parallel without port conflicts.

## Preflight: validate the host

Before starting services, it can be useful to validate that the runner is ready (especially if you expect container features or cgroup-based cleanup).

```bash
# Human-readable output
locald doctor

# Machine-readable output (useful for gating in CI)
locald doctor --json
```

If critical checks fail, the report will usually recommend running:

```bash
sudo locald admin setup
```

## Scripting with `locald`

You can use `locald` to orchestrate your test environment.

```bash
# Start the environment in the background
# Note: --detach isn't strictly necessary if you run it as a background job
locald --sandbox=test up &
PID=$!

# Wait for services to be ready (simple sleep for now)
sleep 5

# Run tests inside the service context
locald --sandbox=test exec web npm test

# Teardown
kill $PID
```

## GitHub Actions Example

Since `locald` is currently built from source, a CI workflow involves building it first.

```yaml
steps:
  - uses: actions/checkout@v3

  - name: Install Rust
    uses: dtolnay/rust-toolchain@stable

  - name: Build locald
    run: cargo install --path locald-cli

  - name: Start Services
    run: |
      # Start locald in the background with a sandbox
      locald --sandbox=ci up &

      # Wait for it to initialize
      sleep 5

  - name: Run Tests
    run: locald --sandbox=ci exec api npm test
```
