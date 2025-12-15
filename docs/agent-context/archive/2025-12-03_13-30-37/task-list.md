# Task List - Phase 20: Builtin Services

- [x] Add `postgresql_embedded` dependency.
- [x] Refactor `ServiceConfig` to support `type` field (defaulting to "exec").
- [x] Create `ServiceRunner` trait/enum in `locald-server`.
- [x] Implement `PostgresRunner` using `postgresql_embedded`.
- [x] Implement data directory management for builtin services.
- [x] Implement `DATABASE_URL` injection for dependent services.
- [x] Update `locald add` to support `postgres` type.
- [x] **Testing**: Add `assert_cmd`, `predicates`, `tokio`, `sqlx` (dev) dependencies.
- [x] **Testing**: Implement integration test for Postgres service.
- [x] Verify with a test project.
