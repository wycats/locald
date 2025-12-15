---
title: "Hot Reloading & Service Restart"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Hot Reloading
---

# RFC: Hot Reloading & Service Restart

## 1. Summary

This RFC proposes implementing automatic service restarts when a project's `locald.toml` configuration changes. It also addresses a bug where manual restarts fail to pick up configuration changes, ensuring that the "source of truth" is always the file on disk.

## 2. Motivation

Currently, the development loop for configuring a service is high-friction:
1.  User edits `locald.toml` (e.g., adding an environment variable or changing a command).
2.  User saves the file.
3.  Nothing happens.
4.  User runs `locald restart <service>`.
5.  (Bug) The service restarts, but often with the *old* configuration, because the daemon hasn't reloaded the file.

This violates the principle of "Least Surprise". Users expect that saving a configuration file should reflect immediately, or at least upon the next restart. The goal is to make `locald` feel "live" and responsive.

## 3. Detailed Design

### User Experience (UX)

*   **Automatic Restart**: When a user saves `locald.toml`, any running services defined in that file that have changed configuration will automatically restart.
*   **Notification**: The Dashboard and CLI logs should indicate "Configuration changed. Restarting <service>...".
*   **Manual Restart**: Running `locald restart <service>` will explicitly reload the configuration from disk before restarting, ensuring the latest state is used.

### Architecture

The `locald-server` needs to expand its file watching capabilities.

1.  **Watcher Registration**: When a project is registered, we must ensure we are watching its `locald.toml` file.
2.  **Event Handling**: On a file modification event:
    *   Parse the new `locald.toml`.
    *   Identify which services have changed.
    *   Trigger a `Restart` action for those services.
3.  **State Management**: The `Service` struct or the `Project` registry needs to update its internal representation of the configuration. Currently, it seems we might be persisting the config in memory or the state file and not refreshing it eagerly enough.

### Implementation Details

*   **`notify` crate**: We likely already use this. We need to ensure we are watching project-specific config files, not just the global one.
*   **Debouncing**: File system events can be noisy. We need to debounce updates to avoid rapid-fire restarts.
*   **Diffing**: Ideally, we only restart if the *effective* configuration (command, env, port) changes. Changing a comment or a non-functional field shouldn't trigger a restart (though `locald.toml` is mostly functional).
*   **Bug Fix**: The `restart` command handler in the daemon must trigger a reload of the project configuration from disk before executing the restart logic.

## 4. Implementation Plan (Stage 2)

- [ ] **Fix Manual Restart**: Ensure `locald restart` reloads config from disk.
- [ ] **Project Config Watcher**: Implement a watcher for registered project `locald.toml` files.
- [ ] **Hot Reload Logic**: Wire up the watcher to the service restart logic.
- [ ] **Debouncing**: Add debouncing to the watcher.

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/features/configuration.md` to mention hot reloading.
- [ ] Update `docs/agent-context/plan-outline.md`.

## 6. Drawbacks

*   **Restart Loops**: If a config file is saved in a broken state or rapidly, it could cause restart loops. (Mitigation: Debouncing, parsing validation).
*   **State Loss**: Restarting kills the process. If the user was in the middle of a stateful operation (that isn't persisted), they lose it. (Acceptable for a dev tool, but maybe we warn?).

## 7. Unresolved Questions

*   Should we prompt before restarting? (Probably not, "save" implies intent).
*   How do we handle syntax errors in the new config? (We should log an error and *not* restart/crash, keeping the old valid config running until a valid one is saved).

## 8. Future Possibilities

*   **Hot Module Replacement (HMR) for Config**: If we support changing env vars without full restart (unlikely for processes, but maybe for some internal settings).
