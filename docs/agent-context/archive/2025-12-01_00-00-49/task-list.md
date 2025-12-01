# Phase 12 Task List

- [x] **Research**

  - [x] Evaluate `bollard` crate.
  - [x] Define `ServiceConfig` schema for containers.

- [x] **Core**

  - [x] Add `image` field to `ServiceConfig`.
  - [x] Add `container_port` field to `ServiceConfig`.

- [x] **Server Implementation**

  - [x] Refactor `ProcessManager` to support multiple runtimes (or add `ContainerManager`).
  - [x] Implement Docker lifecycle (pull, run, stop).
  - [x] Implement Docker log streaming.

- [x] **Refactor: Self-Daemonization**
  - [x] Add `daemonize` crate.
  - [x] Implement self-forking and PID file management.
  - [x] Add `--foreground` flag.
  - [x] Redirect logs to `/tmp/locald.out`.

- [x] **Verification**
  - [x] Test with Redis container.
  - [x] Test mixed workload (Process depends on Container).
