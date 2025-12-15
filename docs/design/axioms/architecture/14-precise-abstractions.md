# Axiom 14: Precise Abstractions

## The Principle

We leverage the Rust ecosystem to avoid reinventing the wheel, but we **never compromise our architecture to fit a mismatched abstraction**.

## The Context

Rust has a vast ecosystem of high-quality crates. It is tempting to reach for a "solution in a box" for every problem (e.g., "just use the systemd crate"). However, libraries often come with opinionated abstraction layers that may conflict with `locald`'s specific constraints (e.g., Single Binary, Zero Runtime Dependencies, Setuid Shim).

## The Rule

1.  **Search First**: Always perform a deep search for existing crates before building custom logic.
2.  **Evaluate the Abstraction**: Does the crate's mental model match ours?
    - _Good_: `libcontainer` (Youki) exposes the raw OCI primitives we need to build a custom runtime.
    - _Bad_: A high-level "container manager" crate that assumes it owns the entire process lifecycle and requires a background daemon.
3.  **Prefer Primitives**: We prefer crates that provide **mechanisms** (e.g., `zbus` for raw DBus, `cgroups-rs` for raw cgroup writes) over crates that enforce **policies** (e.g., a crate that "manages systemd services" but forces a specific unit naming convention).
4.  **Own the Glue**: It is better to write 100 lines of "glue code" on top of a low-level primitive than to fight a high-level abstraction that is 90% correct and 10% fatal.

## Example: Systemd Integration

We need to create a cgroup hierarchy.

- **Option A (Mismatched)**: Use a high-level `systemd-manager` crate. It might force us to create full `.service` files for every process, introducing DBus latency and "package manager" complexity.
- **Option B (Precise)**: Use the "Anchor" strategy. We use the standard file system APIs (mkdir) inside a delegated subtree. We only touch systemd _once_ (via a raw config file write) to establish the delegation. This uses the _primitive_ (Delegation) without the _abstraction_ (Unit Management).
