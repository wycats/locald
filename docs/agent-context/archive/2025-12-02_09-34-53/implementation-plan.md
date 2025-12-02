# Phase 14 Implementation Plan: Dogfooding & Polish

## Goal

Smooth out the rough edges before broader adoption. Focus on error messages, CLI output, and common workflow friction. Ensure the "Zero-Config" promise holds up in real-world usage.

## User Requirements

- **App Builder**: "I want to start using this for my real projects today without fighting the tool."
- **Power User**: "I want clear error messages when things go wrong, not stack traces."
- **Dogfooder**: "I want to run `locald` in multiple projects and have `.dev` domains just work."

## Strategy

1.  **Installation Experience**:
    - Rename `locald-cli` binary to `locald` so users type `locald`.
    - Ensure `cargo install --path locald-cli` and `cargo install --path locald-server` places binaries correctly in `~/.cargo/bin`.
    - Verify the CLI can find the server when both are installed.
2.  **Papercut Pass**: Review and improve CLI output for all common commands. Add colors and better formatting.
3.  **Workflow Verification**:
    - Verify the "start server in each project" mental model. Ensure `locald start` in a new project directory works seamlessly with the background daemon.
    - Investigate and document the `.dev` domain workflow (SSL implications).
4.  **Error Handling**: Improve error messages for common failure modes (Docker missing, port conflicts, invalid config).

## Step-by-Step Plan

### Step 1: Installation & Naming

- [ ] Configure `locald-cli` to produce a binary named `locald`.
- [ ] Verify `cargo install` workflow puts both binaries in the same place.
- [ ] Verify `locald` finds `locald-server` in the installation directory.

### Step 2: CLI Polish

- [ ] Review `locald status` output (alignment, colors).
- [ ] Review `locald logs` output (prefixing, colors).
- [ ] Review `locald start/stop` feedback.

### Step 2: Workflow Validation

- [ ] Verify `locald start` in a new project registers it correctly.
- [ ] Verify `locald stop` stops the project.
- [ ] Test `.dev` domain configuration and document findings/limitations.

### Step 3: Error Handling

- [ ] Test behavior when Docker is down.
- [ ] Test behavior when ports are in use.
- [ ] Test behavior with invalid `locald.toml`.

### Step 4: Documentation

- [ ] Update "Troubleshooting" guide with new findings.
- [ ] Add a "Dogfooding" section or notes for early adopters.
