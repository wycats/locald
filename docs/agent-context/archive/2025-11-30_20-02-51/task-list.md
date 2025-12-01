# Phase 10 Task List

- [x] **Schema Update**
  - [x] Add `depends_on` to `ServiceConfig` in `locald-core`.

- [x] **Dependency Logic**
  - [x] Implement topological sort in `locald-server`.
  - [x] Handle cycle detection.

- [x] **Process Manager**
  - [x] Update `start()` to respect startup order.

- [x] **Verification**
  - [x] Verify startup order with unit tests.
  - [x] Verify startup order with a test project (manual verification).
  - [x] Verify cycle detection with unit tests.
