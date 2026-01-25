---
title: "Boot Feedback & Progress UI"
stage: 3 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Experience
---

# RFC 0062: Boot Feedback & Progress UI

## 1. Summary

This RFC proposes a structured User Interface for the `locald up` boot process, adhering to [Axiom 4: Respectful & Relevant Output](../design/axioms/experience/04-output-philosophy.md).

It aims to replace the current opaque or noisy output with a "Dynamic" progress display that folds away upon success, but persists detailed error information upon failure.

## 2. Motivation

Currently, `locald up` performs complex operations (CNB builds, container creation, service startup, health checks) with inconsistent feedback.

- **Silence**: Users don't know if the system is hung.
- **Noise**: Raw logs are often dumped, violating the "App Builder" persona.
- **Confusion**: When errors occur (like the `exit status: 51` reported in the wild), the context is often lost or buried.

## 3. Detailed Design

### The "Boot" Persona

When running `locald up`, the user is in the **App Builder** persona. They care about:

1.  **What** is happening? (Building, Starting, Waiting)
2.  **How long** will it take?
3.  **Did it work?**

### UI Components

We will use a library like `indicatif` to manage terminal output.

#### 1. The Multi-Progress Bar

We will display a tree of tasks.

```text
locald up
├── 📦 shop-backend
│   ├── 🔨 Building... (Pulling builder image)
│   └── ⏳ Waiting for start...
└── 📦 shop-frontend
    └── ✅ Ready
```

#### 2. The "Fold Away" Behavior

As tasks complete successfully, they should simplify or disappear, leaving only a summary.

**During Boot:**

```text
⠋ [shop-backend] Building...
```

**After Success:**

```text
✔ Services up: shop-backend, shop-frontend
```

#### 3. The "Look Away" Failure

If a task fails, the UI must **persist** the failure details.

```text
✖ [shop-backend] Build Failed

Error: Container execution failed (exit status 51)

Details:
  The build container exited unexpectedly.

  Command: runc run --bundle /tmp/.tmpXXXX ...
  Stderr:
    container_linux.go:380: starting container process caused: process_linux.go:545: container init caused: rootfs_linux.go:76: mounting "proc" to rootfs at "/proc" caused: permission denied
```

### Implementation Strategy

1.  **Event Stream**: The `locald-server` should emit structured events (BuildStarted, BuildProgress, BuildFinished, ServiceStarting, ServiceHealthy).
2.  **CLI Renderer**: The `locald-cli` subscribes to these events and renders the TUI.
3.  **Log Capture**: During the "Dynamic" phase, logs (stdout/stderr) are captured in a buffer.
    - If **Success**: Logs are discarded (or written to a file).
    - If **Failure**: Logs are printed to the terminal to provide context.

## 4. Implementation Plan

### 4.1. Core Types (`locald-core`)

- Define `BootEvent` enum in `locald-core`:
  ```rust
  pub enum BootEvent {
      StepStarted { id: String, description: String },
      StepProgress { id: String, message: String },
      StepFinished { id: String, result: Result<(), String> },
      Log { id: String, line: String, stream: LogStream },
  }
  ```

### 4.2. Server Instrumentation (`locald-server`)

- Update `Manager::start` to accept an event sender (e.g., `mpsc::Sender<BootEvent>`).
- Instrument the build process (`Builder::build`) to emit events.
- Instrument the service startup (`Service::start`) to emit events.
- Create a new IPC endpoint (or modify `Start`) to stream these events back to the client.
  - _Option A_: `Start` returns a stream of events.
  - _Option B_: `Start` returns a Job ID, and a separate `Subscribe(JobId)` call streams events.
  - _Decision_: Use a streaming response if the IPC framework supports it, otherwise use a callback mechanism or a dedicated socket. Given our simple JSON-over-Unix-Socket, we might need to send newline-delimited JSON events until a final "Done" event.

### 4.3. CLI Renderer (`locald-cli`)

- Add `indicatif` dependency.
- Implement a `ProgressRenderer` struct that consumes `BootEvent`s.
- Use `MultiProgress` to manage concurrent tasks (e.g., parallel builds).
- Handle the "Fold Away" logic by clearing progress bars on success.

### 4.4. Shim Error Propagation

- Ensure `locald-shim` captures stderr from `runc` and passes it up so it can be included in the `StepFinished` error variant.

## 5. Context Updates

- [ ] Update `docs/manual/features/cli.md`.
