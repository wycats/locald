---
title: "VMM Maturity and Networking"
stage: 0 # Strawman
feature: VMM
---

# RFC 0102: VMM Maturity and Networking

## 1. Summary

This RFC proposes a next phase for `locald-vmm`: introduce an event-driven architecture suitable for asynchronous device I/O and networking, and define the criteria for adopting external virtio device libraries (e.g. `dbs-virtio-devices`).

## 2. Motivation

The current `locald-vmm` design is intentionally minimal: it boots a kernel and services virtio-mmio exits synchronously on the VCPU thread.

This becomes limiting when:

- We add networking (`virtio-net`), where packets arrive asynchronously.
- We run realistic workloads, where blocking host I/O should not “freeze” the guest CPU.
- We add more devices (rng, vsock, fs), where multiplexing device events becomes the core complexity.

The goal is not to prematurely optimize, but to ensure the VMM architecture can scale to the next forcing function: networking.

## 3. Detailed Design

### 3.0. Decision: Use `event-manager` as the reactor

For Phase 102 we standardize on the rust-vmm `event-manager` crate as the device-side reactor.

This choice is intended to cover the foreseeable needs of `locald-vmm`:

- **Networking (TAP)**: readiness notifications for RX on the tap fd.
- **Virtio completion signaling**: eventfd-based wakeups from device backends to the VCPU thread.
- **Timers**: optional timerfd-based periodic work (rate limiting, deferred actions).
- **Scaling to multiple devices**: a consistent “register fd + subscriber callback” model.

We keep a Firecracker reference checkout under `.references/firecracker/`, and that codebase uses `event-manager`, which reduces ecosystem risk and gives us a well-trodden integration reference point.

### Terminology

- **VCPU thread**: the thread calling `KVM_RUN` / `vcpu.run()`.
- **Reactor / event loop**: a `epoll`-backed loop that waits on file descriptors (tap, eventfd, timerfd) and dispatches readiness notifications.
- **Kick**: a guest -> host notification (e.g. queue notify).
- **Completion**: a host -> guest notification (interrupt) that work has completed.

### 3.1. Why a reactor is needed (and what it is _not_)

A reactor is useful for device-side readiness (tap sockets, eventfds, timers). It does **not** automatically eliminate concurrency.

In particular, on Linux/KVM the VCPU execution loop (`KVM_RUN`) is not naturally expressed as “readiness on an FD we can just poll alongside everything else” in a way that replaces the standard model used by Firecracker/Cloud Hypervisor: a dedicated VCPU thread plus a device/event side loop.

### 3.2. Architecture

Proposed baseline architecture:

#### Thread model (intended)

```
                     +--------------------+
                     |  Device reactor    |
                     |  (event-manager)   |
                     |--------------------|
    TAP fd  -->| RX readiness       |
 eventfd(s)-> | completions        |
 timerfd(s)-> | periodic tasks     |
                     +----------+---------+
                                     |
                                     | signal completion (eventfd)
                                     v
                     +--------------------+
                     |   VCPU thread      |
                     | (KVM_RUN loop)     |
                     |--------------------|
                     | MMIO exits         |
                     | queue notify       |
                     | IRQ injection      |
                     +--------------------+

                     +--------------------+
                     |  I/O pool (opt)    |
                     |--------------------|
                     | block read/write   |
                     | (threadpool;       |
                     |  io_uring later)   |
                     +--------------------+
```

1. **VCPU thread**

   - Runs `vcpu.run()`.
   - Handles synchronous exits (MMIO read/write) and forwards device work requests.
   - Injects interrupts when signaled by device backends (or via a shared interrupt controller abstraction).

2. **Device event loop thread**

   - Uses `epoll` via `event-manager`.
   - Watches:
     - TAP fd (for `virtio-net` RX)
     - eventfds used for completion signaling
     - optional timerfds for periodic tasks

3. **I/O execution**
   - For block I/O: dispatch host file reads/writes to:
     - a dedicated thread pool (simple), or
     - `io_uring` (later, if needed).
   - Notify completion via eventfd -> VCPU injects IRQ.

### 3.3. “mio directly” vs higher level

`mio` remains a viable lower-level option, but choosing `event-manager` up front avoids us rebuilding subscriber/dispatch infrastructure ourselves.

- **If we used `mio` directly**: we would still need to design and maintain our own registration, dispatch, lifecycle, and “which fd belongs to which device” conventions.
- **With `event-manager`**: we adopt an existing abstraction that is already shaped around VMM-style “subscribers” and fd-driven device backends.

### 3.4. Library adoption criteria (`dbs-virtio-devices` etc.)

Adopting an external virtio device library becomes net-positive when:

- We already have the reactor architecture in place (so we can integrate their evented devices without a total rewrite).
- We want more than one complex device (net + block + vsock) and expect virtio spec correctness to be a recurring cost.
- We can accept the dependency surface and their device model constraints.

Non-goals:

- Replacing the VMM core with Dragonball/Firecracker wholesale.
- Adopting a library _just_ to reduce today’s small amount of custom code.

#### Adoption checklist (concrete)

We should plan to adopt an external virtio device library once most of these are true:

- [ ] We have a stable `event-manager` reactor integration (at least TAP + eventfd + timers).
- [ ] We have a stable interrupt delivery abstraction (eventfd -> VCPU -> `set_irq_line` or equivalent).
- [ ] We have at least two virtio devices where spec-correctness is a recurring tax (e.g. net + block).
- [ ] The library’s device model fits our MMIO/transport approach (or we are willing to adapt ours).
- [ ] We have tests that will catch regressions during the swap (boot test + basic net + basic block).

## 4. Implementation Plan (Stage 2)

- [ ] Introduce an interrupt delivery abstraction (eventfd + IRQ injection boundary).
- [ ] Add a device-side reactor thread (mio/event-manager) and a minimal registration API.
- [ ] Convert virtio-block to off-VCPU execution (thread pool) with completion notifications.
- [ ] Implement minimal `virtio-net` with TAP RX/TX, integrated with the reactor.
- [ ] Evaluate `dbs-virtio-devices` integration on top of the new reactor.

Note: with the decision above, “device-side reactor thread” refers to `event-manager`, but the registration API should be designed so we could swap implementations later if needed.

### Phase 102 success criteria

We consider Phase 102 “done enough” when:

- The VM can boot and remain responsive while block I/O is in flight (no “world freeze” on the VCPU thread).
- A minimal `virtio-net` can:
  - receive a packet from the host (tap RX) and deliver it to the guest, and
  - transmit a packet from guest to host (tap TX).
- The reactor loop is the single place where device fds are polled (no ad-hoc polling loops).
- The interrupt/completion path is explicit and testable (eventfd triggers -> IRQ injected).

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/architecture/` with the VMM threading model (VCPU + reactor).
- [ ] Add `docs/manual/features/vmm.md` (or similar) describing supported devices and limitations.
- [x] Update `docs/agent-context/plan-outline.md` to include Phase 102.

## 6. Drawbacks

- Increases architectural complexity (threads + signaling).
- Harder debugging than a single synchronous loop.
- Adds a long-term commitment to an event model API (mio or event-manager).

Mitigation: keep `locald-vmm`’s internal reactor-facing traits small and local, so the majority of the codebase depends on _our_ abstractions rather than directly on `event-manager`.

## 7. Alternatives

- Keep everything synchronous until networking is required, then refactor.
- Use a fully-featured VMM (Firecracker/Cloud Hypervisor) as a subprocess rather than embedding a VMM.
- Use `tokio` as the reactor (via `AsyncFd`) but still keep the VCPU thread; treat tokio as a convenience layer.

We explicitly avoid committing to tokio inside `locald-vmm` at this stage.

## 8. Unresolved Questions

- Should the device reactor live in `locald-vmm` or be shared with other crates?
- Do we want `tokio` in `locald-vmm`, or keep it runtime-agnostic?
- What is the preferred interrupt injection mechanism/abstraction boundary?

Additionally:

- Which minimal subset of `event-manager` APIs do we wrap so swapping reactors remains feasible?

## 9. Future Possibilities

- Switch block I/O to `io_uring` once correctness is locked.
- Add vsock for host/guest IPC.
- Add snapshots and fast boot (for demo workflows).
