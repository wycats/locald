# Build Performance (Rust)

This repo defaults to **incremental compilation** for development builds.

## Incremental compilation

- We explicitly set `incremental = true` for `dev` and `test` profiles in the workspace `Cargo.toml`.
- You can verify incremental is in effect by running `cargo build -vv` and looking for `-C incremental=` in the `rustc` invocation.

## Faster linking (Linux)

Linking is often the slowest part of a Rust edit-compile cycle.

Recommended options:

- **mold** (fast linker):

  - Install `mold`.
  - Run builds with:
    - `RUSTFLAGS='-C link-arg=-fuse-ld=mold' cargo build`

- **lld** (also fast, often available as `ld.lld`):
  - Install `lld`.
  - Run builds with:
    - `RUSTFLAGS='-C link-arg=-fuse-ld=lld' cargo build`

We do not force a linker in repo config because it would break builds on machines without that linker installed.

## One-command opt-in

Use the helper script which auto-detects common speedups:

- `./scripts/fast-build`
  - Uses `sccache` if available.
  - Uses `mold` (preferred) or `lld` if available.
  - Passes through any extra cargo args, e.g. `./scripts/fast-build check -p locald-server`.

For clippy specifically:

- `./scripts/fast-clippy`
  - Same speedups as `fast-build`.
  - Runs `cargo clippy --workspace -- -D warnings`.

## Compiler cache (sccache)

If you do lots of rebuilds across branches/clean builds, a compiler cache can help:

- Install `sccache`.
- Run builds with:
  - `RUSTC_WRAPPER=sccache cargo build`

As with linkers, we don’t enable this unconditionally in repo config.
