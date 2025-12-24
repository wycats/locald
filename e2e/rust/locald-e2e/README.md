# locald-e2e

This crate contains End-to-End (E2E) tests for `locald`.

It provides a `TestContext` harness that:

1.  Creates a temporary directory for each test.
2.  Spawns a sandboxed `locald` daemon instance.
3.  Sets up the environment (`LOCALD_SOCKET`, `LOCALD_SANDBOX_ACTIVE`, `XDG_*_HOME`) to ensure isolation.
4.  Provides helper methods to run CLI commands against the sandboxed daemon.

## Running Tests

To run the E2E tests, you must first build the `locald` binary:

```bash
cargo build --bin locald
cargo test -p locald-e2e
```

## Adding Tests

Create a new test file in `tests/` and use the `TestContext`:

```rust
use anyhow::Result;
use locald_e2e::TestContext;

#[tokio::test]
async fn my_test() -> Result<()> {
    let mut ctx = TestContext::new().await?;
    ctx.start_daemon().await?;

    let output = ctx.run_cli(&["status"]).await?;
    assert!(output.status.success());

    Ok(())
}
```
