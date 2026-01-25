---
title: "Remove DockerRuntime"
stage: 3
feature: Core Architecture
---

# RFC 0142: Remove DockerRuntime

## Status

- **Status**: Recommended (Stage 3)
- **Implemented**: PR #58

## 1. Summary

Remove the `DockerRuntime` abstraction and all Docker daemon dependencies from `locald`, fully transitioning to the OCI + libcontainer path for container execution.

## 2. Motivation

`locald` originally supported two container execution paths:

1. **DockerRuntime**: Relied on the Docker daemon via the `bollard` crate
2. **OCI/libcontainer**: Direct container execution without external dependencies

The DockerRuntime path created several problems:

- **External Dependency**: Required users to have Docker installed and running
- **Licensing Concerns**: Docker Desktop licensing for commercial use
- **Complexity**: Two parallel container execution paths to maintain
- **Inconsistency**: Different behavior between Docker and native execution

The OCI/libcontainer path is now mature enough to be the sole execution strategy.

## 3. Changes

### Files Deleted

- `crates/locald-server/src/runtime/docker.rs` - DockerRuntime implementation
- `crates/locald-server/src/service/docker.rs` - DockerController/DockerFactory

### Dependencies Removed

- `bollard = "0.19.4"` from `crates/locald-server/Cargo.toml`

### Structural Changes

- Removed `docker` module from runtime and service
- Removed `DockerRuntime` field from `Runtime` struct
- Removed `docker` parameter from `ProcessManager::new()`
- Removed `docker` field from `HealthMonitor` and `spawn_docker_monitor`
- Removed `HealthSource::Docker` variant from state
- Removed `image` and `container_port` from `ExecServiceConfig`
- Removed `check_docker_integration` from privileged.rs doctor checks

## 4. Migration

### Breaking Change

The `image` and `container_port` fields on Exec services are no longer supported. Use `type = "container"` services instead.

**Before:**

```toml
[services.redis]
command = "redis-server"
image = "redis:7"
```

**After:**

```toml
[services.redis]
type = "container"
image = "redis:7"
command = "redis-server"
```

## 5. Impact

- **~560 lines removed** (net reduction)
- **Binary size reduced** (bollard crate removed)
- **Faster compilation** (fewer dependencies)
- **Simpler architecture** (single container execution path)
