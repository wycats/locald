# CNB Platform Compliance Audit

## Source

- **Spec**: [Platform Interface Specification](https://github.com/buildpacks/spec/blob/main/platform.md) (Targeting API 0.12 compatibility)
- **Implementation**: `locald-server/src/runtime/process.rs` (`start_cnb_container`)

## Execution Environment Requirements (Launch Phase)

| Requirement            | Spec Section                                                                                                      | Implementation Status | Notes                                                                                                                |
| :--------------------- | :---------------------------------------------------------------------------------------------------------------- | :-------------------- | :------------------------------------------------------------------------------------------------------------------- |
| **API Version**        | [Platform API Compatibility](https://github.com/buildpacks/spec/blob/main/platform.md#platform-api-compatibility) | ✅ Compliant          | Sets `CNB_PLATFORM_API=0.12`.                                                                                        |
| **Entrypoint**         | [Launch](https://github.com/buildpacks/spec/blob/main/platform.md#launch)                                         | ✅ Compliant          | Uses `/cnb/lifecycle/launcher` as the entrypoint.                                                                    |
| **Process Selection**  | [Launcher Inputs](https://github.com/buildpacks/spec/blob/main/platform.md#launcher)                              | ✅ Compliant          | **Fixed**: Now invokes `launcher` directly for default process. Previously used manual parsing ("mashing").          |
| **User Environment**   | [Launch Environment](https://github.com/buildpacks/spec/blob/main/platform.md#launch-environment)                 | ✅ Compliant          | User-provided env vars are injected directly into the container environment.                                         |
| **App Directory**      | [Launcher Inputs](https://github.com/buildpacks/spec/blob/main/platform.md#launcher)                              | ✅ Compliant          | Mounts project root to `/workspace` (default `CNB_APP_DIR`).                                                         |
| **Layers Directory**   | [Launcher Inputs](https://github.com/buildpacks/spec/blob/main/platform.md#launcher)                              | ✅ Compliant          | Mounts `.locald/cache` (or similar) to `/layers` (default `CNB_LAYERS_DIR`).                                         |
| **Platform Directory** | [Launcher Inputs](https://github.com/buildpacks/spec/blob/main/platform.md#launcher)                              | ⚠️ Partial            | Mounts `/platform` but does not currently populate `env/` files for build phase (Launch phase uses direct env vars). |
| **Experimental Mode**  | [Experimental Features](https://github.com/buildpacks/spec/blob/main/platform.md#experimental-features)           | ✅ Compliant          | Sets `CNB_EXPERIMENTAL_MODE=warn`.                                                                                   |
| **Registry Auth**      | [Registry Authentication](https://github.com/buildpacks/spec/blob/main/platform.md#registry-authentication)       | ❌ Missing            | `CNB_REGISTRY_AUTH` is not currently handled.                                                                        |

## Remediation: Manual Metadata Parsing

**Status**: Fixed
**Date**: 2025-12-07

We identified that `locald-server` was manually parsing `io.buildpacks.build.metadata` to extract the default process command and executing it via `/bin/sh -c`. This was incorrect because:

1.  It ignored the `direct` flag (forcing shell execution).
2.  It bypassed the `launcher`'s native logic for process selection.
3.  It was a fragile reimplementation of standard CNB behavior.

**Fix**:

- Removed manual parsing logic in `start_cnb_container`.
- Updated execution path to invoke `/cnb/lifecycle/launcher` directly when no command is provided.
- Added a "bridge" mechanism: If `/layers/config/metadata.toml` is missing (which `launcher` requires), we now generate it from the `io.buildpacks.build.metadata` image label. This ensures the `launcher` works in our `runc` environment without needing to "mash" the command.
- Added `CNB_LAYERS_DIR=/layers` and `CNB_APP_DIR=/workspace` environment variables to ensure `launcher` finds the metadata.
