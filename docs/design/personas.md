# User Personas

To ensure `locald` serves its users effectively, we design features and documentation with specific personas in mind. These personas represent the primary archetypes of our user base.

## 1. The App Builder ("Regular Joe")

**"I just want to run my app."**

- **Goal**: Develop web applications without fighting with infrastructure.
- **Needs**:
  - Simple, "magic" configuration (zero-config where possible).
  - Clear, actionable error messages.
  - "It just works" local domains (`myapp.local`).
  - Easy access to logs when things break.
- **Frustrations**:
  - Manually editing `/etc/hosts`.
  - Port conflicts ("Address already in use").
  - Complex Docker-compose files for simple apps.
- **Documentation Needs**:
  - Quick Start Guide (5 minutes or less).
  - Copy-pasteable `locald.toml` examples.
  - Troubleshooting guide for common errors.

## 2. The System Tweaker ("Power User")

**"I want to control how it works."**

- **Goal**: Customize the development environment to match specific production constraints or personal preferences.
- **Needs**:
  - Ability to override defaults (ports, domains, environment variables).
  - Support for complex setups (multiple services, dependencies).
  - Scriptability (CLI flags, JSON output).
  - Understanding where files live (logs, state, config).
- **Frustrations**:
  - "Magic" behavior that can't be disabled.
  - Opaque errors or hidden state.
  - Lack of CLI arguments.
- **Documentation Needs**:
  - Full Configuration Reference (`locald.toml` spec).
  - CLI Command Reference.
  - Environment Variable Reference.

## 3. The Contributor ("The Rustacean")

**"I want to improve the tool."**

- **Goal**: Fix bugs, add features, or understand the internal architecture of `locald`.
- **Needs**:
  - Clear architectural boundaries (Client vs. Server vs. Core).
  - Well-documented internal APIs and data structures.
  - Easy development setup (build, test, run).
  - Understanding of the design philosophy (Axioms).
- **Frustrations**:
  - Spaghetti code.
  - Undocumented hacks or side effects.
  - Difficult build process.
- **Documentation Needs**:
  - Architecture Overview (Diagrams, Data Flow).
  - Development Setup Guide.
  - Contribution Guidelines.
