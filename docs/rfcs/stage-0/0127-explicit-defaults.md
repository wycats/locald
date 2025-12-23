---
title: "Explicit Defaults vs. Services"
stage: 0
feature: Configuration
exo:
    tool: exo rfc create
    protocol: 1
---

# RFC 0127: Explicit Defaults vs. Services

## 1. The Problem

In the current configuration hierarchy (RFC 0026), a service defined in `locald.workspace.toml` is ambiguous:

```toml
# locald.workspace.toml
[services.web]
command = "npm run dev"
```

This can be interpreted in two ways:

1.  **Template (Inheritance)**: "Any project that defines a `web` service should use this command by default."
2.  **Singleton (Runtime)**: "There is a shared `web` service running in the workspace root."

Currently, `locald` treats it as a **Template** if a child project also defines `[services.web]`, but as a **Singleton** if a child project `depends_on = ["web"]` without defining it. This implicit behavior is confusing ("Magic").

## 2. Proposed Solution: `[defaults]` Section

We propose introducing a dedicated `[defaults]` section for templates.

### Syntax

```toml
# locald.workspace.toml

# 1. PURE TEMPLATE
# Does NOT run as a service.
# Only used to populate defaults for child projects.
[defaults.services.web]
command = "npm run dev"
port = 3000

# 2. SHARED SERVICE
# Runs as a service in the workspace.
# Can be depended upon by child projects.
[services.db]
image = "postgres:15"
```

### Behavior

1.  **`[defaults.services.NAME]`**:
    *   Merged into any project that defines `[services.NAME]`.
    *   Never runs on its own.
    *   Cannot be referenced in `depends_on` (unless a project instantiates it).

2.  **`[services.NAME]`**:
    *   Runs as a service in the scope it is defined.
    *   Can be referenced in `depends_on`.
    *   **Does NOT** cascade/merge into child projects with the same name? (Open Question)
        *   *Option A*: It still cascades (current behavior + explicit defaults).
        *   *Option B*: It does NOT cascade. If you want cascading, use `[defaults]`.

## 3. Evaluation

### Pros
*   **Clarity**: Removes ambiguity between "I want to configure how `web` runs everywhere" and "I want to run a shared `web`".
*   **Safety**: Prevents accidental execution of templates.

### Cons
*   **Verbosity**: Adds another top-level section.
*   **Migration**: Existing configs need to be updated.

## 4. Recommendation

We should adopt this pattern. The ambiguity of the current model is a long-term liability.

### Transition Plan
1.  Support `[defaults.services]` in the parser.
2.  Deprecate "Template" behavior of `[services]` in parent scopes.
3.  Warn when a parent service is used as a template.
