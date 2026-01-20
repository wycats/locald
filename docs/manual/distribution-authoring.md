# Distribution Authoring Guide

This guide explains how to create, distribute, and use locald distributions.

## What is a Distribution?

A **distribution** is a project bootstrap kit that bundles plugins and project configuration. Distributions enable "clone → locald up" workflows for teams by packaging:

- One or more plugin packages (bundled or remote)
- A starter `locald.toml` with service definitions
- Optional project scaffolding (templates, example code)

| Concept          | Purpose                | Contains                    | Installed To          |
| ---------------- | ---------------------- | --------------------------- | --------------------- |
| **Package**      | Single plugin delivery | 1 WASM + assets             | Plugin directory      |
| **Distribution** | Project bootstrap kit  | N packages + project config | New project directory |

## Distribution Structure

A `.locald-distribution` file is a gzip-compressed tar archive:

```
redis-stack-1.0.0.locald-distribution
├── distribution.toml         # Distribution metadata (required)
├── locald.toml               # Project config template (required)
├── packages/                 # Bundled plugin packages (optional)
│   ├── redis-plugin-1.0.0.locald-package
│   └── postgres-plugin-2.0.0.locald-package
└── scaffold/                 # Project scaffolding (optional)
    ├── .gitignore
    ├── README.md.template
    └── docker-compose.yml
```

## Creating a Distribution

### 1. Create the Distribution Directory

```bash
mkdir my-distribution
cd my-distribution
```

### 2. Create `distribution.toml`

```toml
[distribution]
name = "redis-stack"
version = "1.0.0"
description = "Redis development stack"
license = "MIT"
authors = ["Your Name <you@example.com>"]

[compatibility]
locald_min = "0.2.0"

[plugins]
# Bundled plugins (files in packages/ directory)
bundled = [
    "redis-plugin-1.0.0.locald-package",
]

# Remote plugins (fetched during init)
remote = [
    "https://plugins.locald.dev/postgres-plugin.locald-package",
    { url = "https://example.com/custom.locald-package", sha256 = "abc123..." }
]

[scaffold]
# Template files to render (support variable substitution)
templates = ["README.md.template"]

# Files to copy as-is
files = [".gitignore"]

# Variables available in templates
[scaffold.variables]
project_name = { prompt = "Project name", default = "my-project" }
database_name = { prompt = "Database name", default = "app_development" }
```

### 3. Create `locald.toml` Template

```toml
[project]
name = "{{project_name}}"

[[services]]
name = "redis"
kind = "redis"

[[services]]
name = "db"
kind = "postgres"
config.database = "{{database_name}}"
```

### 4. Add Scaffold Files

Create the `scaffold/` directory with templates and static files:

```bash
mkdir scaffold
echo "# {{project_name}}" > scaffold/README.md.template
echo ".locald/\nnode_modules/" > scaffold/.gitignore
```

### 5. Add Bundled Plugins (Optional)

```bash
mkdir packages
cp /path/to/redis-plugin-1.0.0.locald-package packages/
```

### 6. Build the Distribution

```bash
locald distribution create
```

This creates `redis-stack-1.0.0.locald-distribution`.

#### Build Options

```bash
# Show what would be packaged without creating archive
locald distribution create --dry-run

# Include remote plugins in the archive (for offline use)
locald distribution create --include-remote

# Overwrite existing output file
locald distribution create --force

# Show detailed output
locald distribution create --verbose
```

## Using a Distribution

### Initialize from Local File

```bash
locald init --from-distribution redis-stack-1.0.0.locald-distribution
```

### Initialize from URL

```bash
locald init --from-distribution https://example.com/redis-stack-1.0.0.locald-distribution
```

### Options

```bash
# Set project name (skips prompt)
locald init --from-distribution my.locald-distribution --name my-app

# Specify target directory
locald init --from-distribution my.locald-distribution --target ./projects/new-app

# Skip scaffold files
locald init --from-distribution my.locald-distribution --no-scaffold

# Use only bundled plugins (skip remote fetches)
locald init --from-distribution my.locald-distribution --offline

# Accept all defaults without prompting
locald init --from-distribution my.locald-distribution --yes
```

## Template Syntax

Templates use `{{variable_name}}` syntax for variable substitution:

```markdown
# {{project_name}}

Welcome to your new project!

Database: {{database_name}}
```

Variables are defined in `distribution.toml` under `[scaffold.variables]`:

```toml
[scaffold.variables]
project_name = { prompt = "Project name", default = "my-project" }
database_name = { prompt = "Database name" }
```

## Remote Plugin References

You can reference remote plugins in `locald.toml` directly:

```toml
[project]
name = "my-app"

[plugins]
# Simple URL reference
redis = "https://plugins.locald.dev/redis-plugin-1.0.0.locald-package"

# URL with checksum verification
postgres = { url = "https://plugins.locald.dev/postgres-plugin.locald-package", sha256 = "abc123..." }

# Local path reference (for development)
custom = { path = "../my-custom-plugin/target/plugin.wasm" }

[[services]]
name = "redis"
kind = "redis"
```

## Best Practices

1. **Version your distributions**: Use semantic versioning for compatibility.
2. **Include checksums**: Add SHA-256 checksums for remote plugins for security.
3. **Document variables**: Use clear prompts that explain what each variable is for.
4. **Test offline mode**: Ensure bundled plugins work without network access.
5. **Keep it minimal**: Include only essential files in the scaffold.

## Example: Full Distribution

See the example distribution in `examples/redis-stack-distribution/` for a complete working example.
