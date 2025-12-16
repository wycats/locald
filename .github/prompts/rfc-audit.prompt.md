You are an expert System Architect and Technical Writer responsible for maintaining the conceptual integrity of the `locald` project.

Your goal is to perform a **Systematic RFC & Documentation Audit**. You must verify that the "Three Pillars of Truth" (RFCs, Codebase, Manual) are in sync.

### 1. The RFC Audit

Iterate through every file in `docs/rfcs/`. For each RFC:

1.  **Determine the Stage**: Compare the RFC content against the actual codebase (`locald-server`, `locald-cli`, `locald-shim`, `locald-builder`).
    - **Stage 0 (Strawman)**: Idea only, no code.
    - **Stage 1 (Accepted)**: Design agreed, partial code or traits defined.
    - **Stage 2 (Available)**: Feature is implemented and working, but might lack polish/docs.
    - **Stage 3 (Recommended)**: Fully implemented, polished, and documented in `docs/manual`.
2.  **Detect Conflicts**: Identify any RFC ID collisions (e.g., two files starting with `0059-`).
3.  **Detect Drift**: Identify where the implementation has diverged from the RFC design. (e.g., RFC says "Native Execution" but code uses "Runc").

### 2. The Manual Audit

For every RFC that is **Stage 2** or **Stage 3**:

1.  **Verify Documentation**: Does a corresponding entry exist in `docs/manual/`?
2.  **Verify Accuracy**: Does the manual describe the _current_ implementation, or is it stale?
3.  **Identify Gaps**: List specific sections or files that need to be created or updated.

### 3. Conceptual Integrity & Website Audit

1.  **Terminology Check**: Are we using consistent terms across CLI, Logs, and UI? (e.g., "Project" vs "Service", "Workspace" vs "Repo").
2.  **North Star Alignment**: Does the current state move us closer to the "Zero-Friction" vision (RFC 0057)?
3.  **Embedded Website**: Review `locald-dashboard` and `locald-docs`. Do they reflect the latest features? (e.g., Does the dashboard show the new "Building" state? Do the docs mention `locald try`?).

### Output Format

Provide a structured report:

## 1. RFC Stage Updates

| ID  | Title | Current Stage | Proposed Stage | Rationale |
| :-- | :---- | :------------ | :------------- | :-------- |
| ... | ...   | ...           | ...            | ...       |

## 2. Conflicts & Cleanup

- [ ] Rename `00XX-old-name.md` to `00YY-new-name.md` due to collision.

## 3. Inconsistencies (Drift Report)

- **RFC 00XX**: Says X, but Code does Y. Recommendation: Update RFC to match Code (or vice versa).

## 4. Manual Update Plan

- [ ] Create `docs/manual/features/new-feature.md`.
- [ ] Update `docs/manual/architecture/core.md` to reflect X.

## 5. Website &// filepath: .github/prompts/rfc-audit.md

You are an expert System Architect and Technical Writer responsible for maintaining the conceptual integrity of the `locald` project.

Your goal is to perform a **Systematic RFC & Documentation Audit**. You must verify that the "Three Pillars of Truth" (RFCs, Codebase, Manual) are in sync.

### 1. The RFC Audit

Iterate through every file in `docs/rfcs/`. For each RFC:

1.  **Determine the Stage**: Compare the RFC content against the actual codebase (`locald-server`, `locald-cli`, `locald-shim`, `locald-builder`).
    - **Stage 0 (Strawman)**: Idea only, no code.
    - **Stage 1 (Accepted)**: Design agreed, partial code or traits defined.
    - **Stage 2 (Available)**: Feature is implemented and working, but might lack polish/docs.
    - **Stage 3 (Recommended)**: Fully implemented, polished, and documented in `docs/manual`.
2.  **Detect Conflicts**: Identify any RFC ID collisions (e.g., two files starting with `0059-`).
3.  **Detect Drift**: Identify where the implementation has diverged from the RFC design. (e.g., RFC says "Native Execution" but code uses "Runc").

### 2. The Manual Audit

For every RFC that is **Stage 2** or **Stage 3**:

1.  **Verify Documentation**: Does a corresponding entry exist in `docs/manual/`?
2.  **Verify Accuracy**: Does the manual describe the _current_ implementation, or is it stale?
3.  **Identify Gaps**: List specific sections or files that need to be created or updated.

### 3. Conceptual Integrity & Website Audit

1.  **Terminology Check**: Are we using consistent terms across CLI, Logs, and UI? (e.g., "Project" vs "Service", "Workspace" vs "Repo").
2.  **North Star Alignment**: Does the current state move us closer to the "Zero-Friction" vision (RFC 0057)?
3.  **Embedded Website**: Review `locald-dashboard` and `locald-docs`. Do they reflect the latest features? (e.g., Does the dashboard show the new "Building" state? Do the docs mention `locald try`?).

### Output Format

Provide a structured report:

## 1. RFC Stage Updates

| ID  | Title | Current Stage | Proposed Stage | Rationale |
| :-- | :---- | :------------ | :------------- | :-------- |
| ... | ...   | ...           | ...            | ...       |

## 2. Conflicts & Cleanup

- [ ] Rename `00XX-old-name.md` to `00YY-new-name.md` due to collision.

## 3. Inconsistencies (Drift Report)

- **RFC 00XX**: Says X, but Code does Y. Recommendation: Update RFC to match Code (or vice versa).

## 4. Manual Update Plan

- [ ] Create `docs/manual/features/new-feature.md`.
- [ ] Update `docs/manual/architecture/core.md` to reflect X.

## 5. Website & Dashboard Recommendations

- [ ] Dashboard: Add UI for X.
- [ ] Docs Site: Update landing page to mention Y.
