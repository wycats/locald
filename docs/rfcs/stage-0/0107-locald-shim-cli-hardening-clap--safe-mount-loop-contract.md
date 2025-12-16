---
title: locald-shim CLI Hardening (clap) + Safe Mount-Loop Contract
stage: 0
feature: Security
---


# RFC 0107: locald-shim CLI Hardening (clap) + Safe Mount-Loop Contract

## 1. Summary
Add two tightly-scoped hardening improvements to the privileged `locald-shim`:

1) Replace hand-rolled `std::env::args()` parsing with `clap` (derive-based) to reduce parsing edge-cases and make it harder to accidentally broaden the privileged command surface.

2) Define a “safe mount-loop contract” for a future `locald-shim admin mount-loop …` command, allowing `locald` to mount a loop-backed filesystem image (e.g. ext4) deterministically via syscalls (not host binaries), with strict path containment and conservative mount flags.

This RFC is Stage 0 (idea): it defines the intended shape, constraints, and security posture before any privileged filesystem features are implemented.

## 2. Motivation
We expect to add more privileged subcommands over time (e.g., safe mounting for debugging/extraction or buildpack/OCI workflows). The current shim CLI parsing is intentionally small, but it is manual and positional in places, which increases footguns in a setuid context.

Separately, it’s tempting to rely on userland tools like `debugfs`, but these are not baseline on minimal systems (they come from `e2fsprogs`). If we want “baseline Linux” behavior, we should prefer syscalls (`mount(2)`, loop ioctls) and treat userland helpers as optional enhancements.

## 3. Goals
- Make the shim command surface easier to read, review, and extend safely.
- Treat shim CLI changes as an explicit *protocol bump* (not a backward-compat promise).
- Establish a conservative contract for a future mount-related subcommand:
  - strict input validation
  - predictable containment rules
  - conservative mount flags (e.g. `nosuid,nodev,noexec`, and typically `ro`)
  - no dependency on external host binaries.

## 4. Non-Goals
- Implementing the mount-loop behavior immediately.
- Adding general-purpose “run arbitrary host commands” functionality to the shim.
- Providing a cross-platform mount abstraction (this is Linux-focused).
- Preserving `locald-shim` as a stable, user-facing CLI surface: direct invocation by humans/scripts is not a supported interface. The supported interface is `locald` invoking the shim.

## 5. Detailed Design (Proposed)

### 5.1. `clap` in the setuid shim
Add `clap` v4 with a conservative configuration:

- `clap = { version = "4", features = ["derive"], default-features = false }`
- Do not enable features that automatically read environment variables or config files.
- Keep subcommand trees explicit and narrow.

The intent is not “more features”, but “fewer parsing edge cases”:

- required args enforced by the parser
- typed values (`u16`, `PathBuf`, enums) parsed consistently
- consistent help/usage output for privileged commands

### 5.2. Shim argv as protocol (breaking changes allowed)
`locald` already enforces that the installed privileged shim matches the running `locald` binary (via a strict integrity check) and provides a dedicated install/repair path (`sudo locald admin setup`).

Therefore:

- The shim command-line is an internal protocol.
- We do not need to preserve legacy argv forms for humans/scripts.
- We *can* break shim argv freely as long as we update all `locald` call sites in the same change and rely on the existing “shim must be updated” gate.

### 5.3. Shim discovery and verification
`locald` should only discover/verify a *privileged* shim (root-owned + setuid) and must ignore “convenient but incorrect” unprivileged sibling shims produced during development.

This keeps the privileged boundary crisp:

- Unprivileged debug builds must not shadow the configured privileged shim.
- Shim changes are a protocol bump that requires installing the updated setuid shim (e.g. via `sudo locald admin setup`).

### 5.4. Protocol surface (current locald callers)
These entrypoints exist today and are invoked by `locald` itself:

- `--shim-version`
- `bundle run --bundle <PATH> --id <ID>`
- `bind <port> <socket_path>`
- `admin sync-hosts <domain...>`
- `admin cleanup <abs-path>`
- `debug port <port>`

This list should be treated as an internal protocol. Breaking changes are acceptable if they are coupled with the shim update requirement.

### 5.5. Safe mount-loop contract (future `admin mount-loop`)
The mount-loop command is a privileged primitive, not a convenience wrapper.

Proposed shape:

- `locald-shim admin mount-loop --image <abs-path> --target <abs-path> [--fstype ext4] [--readonly]`

Contract requirements:

1) **Containment rules**
   - Both `--image` and `--target` must be absolute.
   - Both paths must be *within a locald-owned directory* (policy TBD, but should be stricter than “contains `.locald` somewhere”).
   - The target should be under a dedicated shim-managed subtree (e.g. `<project>/.locald/mounts/...`) to make cleanup and auditing straightforward.

2) **Symlink safety**
   - Resolve containment without following attacker-controlled symlinks.
   - Prefer `openat2(2)`-style “no symlink escape” semantics when available (or emulate conservatively).

3) **Mount flags (conservative-by-default)**
   - Always apply `nosuid,nodev,noexec`.
   - Default to `ro` unless a narrow use case requires `rw`.

4) **Implementation strategy**
   - Loop device allocation via `/dev/loop-control` + loop ioctls.
   - Filesystem mount via `mount(2)` (via `nix::mount` or direct `libc::mount`).
   - No `Command::new("mount")`, no PATH lookups, no shell.

5) **Lifecycle / cleanup**
   - Provide an unmount companion command (or make mount-loop idempotent and self-cleaning).
   - Ensure failure paths detach loop devices and don’t leave partial mounts.

### 5.6. Relationship to `debugfs` / `e2fsprogs`
If/when we need filesystem introspection:

- Treat `debugfs` as an optional enhancement (not a requirement).
- Baseline behavior should not assume `e2fsprogs` is installed.

## 6. Risks / Drawbacks
- Adding `clap` increases privileged TCB size (dependency tree and code paths).
- Mounting is security-sensitive; even a narrow “loop + ro” contract can be misused without careful path containment and symlink protections.

## 7. Alternatives
- Keep hand-rolled parsing and add new privileged subcommands incrementally.
- Avoid mounting and require `debugfs` / userland tooling (reduces shim scope but harms “baseline Linux” portability).

## 8. Open Questions
- What is the exact “locald-owned path” policy for privileged filesystem ops (project-local only vs global cache)?
- Do we ever need `rw` mounts, or can we design workflows around `ro` + copy-out?
- Should we require a dedicated “shim mount root” directory created with fixed ownership/permissions?
