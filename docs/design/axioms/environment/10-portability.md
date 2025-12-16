# Axiom 7: Cross-Platform Portability

`locald` is designed to be a universal tool for developers, regardless of their operating system.

## Principles

1.  **OS-Agnostic by Default**: All core features must work on Linux, macOS, and Windows.
2.  **Portable Dependencies**: Prefer Rust crates that provide cross-platform abstractions (e.g., `listeners` instead of `lsof`, `notify` instead of `inotify`).
3.  **Graceful Degradation**: If a feature relies on OS-specific capabilities (like `setuid` on Linux), it must be implemented in a way that doesn't break the build or functionality on other platforms. It should either be gated or have a fallback.
4.  **No Shell Scripts**: Avoid relying on shell scripts (bash, sh) for core logic, as they are not portable to Windows. Use Rust code or portable process execution.

## Capability Gating

“Cross-platform” does not imply that every optional OS feature is enabled by default.

- `locald` must run on Linux, macOS, and Windows.
- Individual _capabilities_ (e.g. VM boot via host hypervisor APIs) may require enabling an OS feature (e.g. Windows Hypervisor Platform, Hyper-V).
- When a capability is unavailable, `locald` must:
  - probe and report the missing capability precisely,
  - fail with an actionable remediation (“enable X and reboot”), and
  - degrade gracefully without breaking unrelated workflows.
