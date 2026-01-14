# Plugin Packaging Guide

This guide explains how to create, distribute, and install locald plugins using the `.locald-package` format.

## Overview

A `.locald-package` is a gzip-compressed tar archive containing:

- `manifest.toml` — Package metadata (required)
- `plugin.wasm` — WASM component (required)
- `assets/` — Optional bundled assets (templates, configs, etc.)

## Creating a Plugin Package

### 1. Set Up Your Plugin Directory

```
my-plugin/
├── manifest.toml      # Required: Package metadata
├── plugin.wasm        # Required: Compiled WASM component
└── assets/            # Optional: Bundled files
    └── templates/
        └── config.tmpl
```

### 2. Write the Manifest

Create `manifest.toml` with your plugin's metadata:

```toml
[package]
name = "redis-plugin"                    # Required: lowercase + hyphens only
version = "1.0.0"                        # Required: semver format
description = "Redis service support"   # Optional: human-readable description
license = "MIT"                          # Optional: SPDX identifier
repository = "https://github.com/..."   # Optional: source URL
authors = ["Your Name <you@example.com>"] # Optional

[plugin]
component = "plugin.wasm"               # Required: path to WASM file
service_kinds = ["redis"]               # Required: service kinds this plugin handles

[compatibility]
locald_min = "0.2.0"                    # Optional: minimum locald version
ir_version = 1                          # Required: IR version (currently 1)

[capabilities]
required = ["oci_pull"]                 # Capabilities plugin requires
optional = ["cache_dir"]                # Capabilities plugin can use if available
```

### 3. Build the Package

```bash
# From your plugin directory
locald plugin create ./my-plugin

# Or specify output path
locald plugin create ./my-plugin --output my-plugin-1.0.0.locald-package

# Preview without creating
locald plugin create ./my-plugin --dry-run

# Overwrite existing package
locald plugin create ./my-plugin --force
```

### 4. Verify the Package

```bash
# List archive contents
tar -tzvf my-plugin-1.0.0.locald-package

# Should show:
# manifest.toml
# plugin.wasm
# assets/... (if present)
```

## Installing Packages

### Local Installation

```bash
# Install to project scope (default: .locald/plugins/)
locald plugin install my-plugin-1.0.0.locald-package

# Install to user scope ($XDG_DATA_HOME/locald/plugins/)
locald plugin install my-plugin-1.0.0.locald-package --user

# Force overwrite existing plugin
locald plugin install my-plugin-1.0.0.locald-package --force
```

### Remote Installation

```bash
# Install from URL (shows security warning)
locald plugin install https://example.com/plugins/redis-1.0.0.locald-package
```

### Installation Output

```
→ Extracting package...
→ Checking compatibility...
   ✓ locald version 0.2.1 meets minimum 0.2.0
   ✓ IR version 1 supported
   ⚠ Requires capability: oci_pull (will be requested at runtime)
→ Installing to .locald/plugins/redis-plugin.wasm
   ✓ Installed 3 asset files
→ Done.
```

## Manifest Reference

### `[package]` Section

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Package name. Must match `^[a-z][a-z0-9-]*$`, max 64 chars |
| `version` | Yes | Semantic version (MAJOR.MINOR.PATCH) |
| `description` | No | Human-readable description |
| `license` | No | SPDX license identifier |
| `repository` | No | Source repository URL |
| `authors` | No | List of author strings |

### `[plugin]` Section

| Field | Required | Description |
|-------|----------|-------------|
| `component` | Yes | Path to WASM file within package |
| `service_kinds` | Yes | List of service kinds this plugin handles |

### `[compatibility]` Section

| Field | Required | Description |
|-------|----------|-------------|
| `locald_min` | No | Minimum locald version (semver). Install fails if current < min |
| `ir_version` | Yes | IR version the plugin produces. Must be supported by host |

### `[capabilities]` Section

| Field | Required | Description |
|-------|----------|-------------|
| `required` | No | Capabilities the plugin requires to function |
| `optional` | No | Capabilities the plugin can use if granted |

## Available Capabilities

| Capability | Description |
|------------|-------------|
| `oci_pull` | Pull OCI container images |
| `cache_dir` | Access to shared cache directory |

## Version Compatibility

### Package Versioning

Follow semver conventions:

| Change | Version Bump |
|--------|--------------|
| Bug fix in plan generation | PATCH (0.0.x) |
| New service kind supported | MINOR (0.x.0) |
| Breaking behavior change | MAJOR (x.0.0) |
| IR version bump required | MAJOR (if breaking) |

### IR Version Compatibility

The `ir_version` field declares which Plan IR version the plugin produces. Currently only IR version 1 is supported. If you upgrade to a new IR version:

- Keep supporting the previous version if possible
- Use a MAJOR version bump if dropping old IR support
- Check `host_capabilities.supported_ir_versions` in your plugin

## Best Practices

1. **Keep packages small** — Only include necessary files
2. **Use descriptive names** — `redis-plugin` not `my-plugin`
3. **Document service_kinds** — List all service kinds your plugin can handle
4. **Declare capabilities** — Be explicit about what your plugin needs
5. **Set locald_min** — Prevent installation on incompatible versions
6. **Test before distributing** — Use `locald plugin validate` and `locald plugin inspect`

## Troubleshooting

### "Invalid package name"

Package names must:
- Start with a lowercase letter
- Contain only lowercase letters, numbers, and hyphens
- Be 64 characters or less

### "Incompatible IR version"

Your plugin produces a Plan IR version not supported by the installed locald. Update your plugin to target a supported IR version, or ask users to upgrade locald.

### "Plugin already installed"

Use `--force` to overwrite:

```bash
locald plugin install my-plugin.locald-package --force
```

### "Package missing manifest.toml"

Ensure your package creation included the manifest:

```bash
locald plugin create ./my-plugin --verbose
```

## See Also

- [RFC 0129: WASM Plugins as Plan Transforms](../rfcs/stage-1/0129-wasm-plugins-as-plan-transforms.md)
- [Plugin Development Guide](plugin-development.md) *(coming soon)*
