# Redis Stack Distribution

This is an example distribution that demonstrates how to create a project bootstrap kit for locald.

## Contents

- `distribution.toml` - Distribution manifest with metadata and plugin references
- `locald.toml` - Project configuration template with variable substitution
- `scaffold/` - Template and static files for project scaffolding
- `packages/` - Directory for bundled plugin packages (currently empty, uses remote refs)

## Usage

### Creating the Distribution Archive

From this directory:

```bash
locald distribution create
```

This creates `redis-stack-1.0.0.locald-distribution`.

### Initializing a New Project

From anywhere:

```bash
locald init --from-distribution redis-stack-1.0.0.locald-distribution
```

Or using a URL:

```bash
locald init --from-distribution https://example.com/redis-stack-1.0.0.locald-distribution
```

### Options

- `--name <NAME>` - Set project name (skips prompt)
- `--target <DIR>` - Specify target directory
- `--no-scaffold` - Skip scaffold files
- `--offline` - Skip remote plugin fetches
- `--yes` - Accept all defaults without prompting

## Distribution Structure

```
redis-stack-distribution/
├── distribution.toml      # Manifest
├── locald.toml             # Config template
├── packages/               # Bundled packages (optional)
└── scaffold/
    ├── README.md.template  # Template file (uses {{variables}})
    └── .gitignore          # Static file
```
