# Phase 13 Task List

- [x] **Core**

  - [x] Update `ServiceConfig` struct in `locald-core`.
  - [x] Update `ServiceState` struct to include health metadata.

- [x] **Server: Notify Socket**

  - [x] Implement `NotifyServer` (Unix Datagram listener).
  - [x] Inject `NOTIFY_SOCKET` into process environment.
  - [x] Wire up `READY=1` to service state.

- [x] **Server: Docker Health**

  - [x] Check `bollard` container config for `Healthcheck`.
  - [x] Implement health polling for Docker containers.

- [x] **Server: TCP Probe**

  - [x] Implement default TCP connect loop for services with ports.

- [x] **Server: Logic**

  - [x] Update `start` flow to wait for health before unblocking dependents.

- [x] **CLI**
  - [x] Update `status` command to show Health Status and Source.
