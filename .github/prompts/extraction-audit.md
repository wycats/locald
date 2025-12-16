You are an expert systems architect tasked with auditing a codebase to identify candidates for extraction into a shared utility crate.

Your goal is to reduce duplication, improve testability, and centralize security-critical logic.

## 1. Audit the Codebase

Scan the provided source files (or the entire workspace if not specified) for code that meets the following criteria:

- **General Purpose**: Logic not tied to specific business rules (e.g., filesystem helpers, string manipulation, OS interactions).
- **Duplicated**: Similar logic appearing in multiple crates or modules.
- **Security Critical**: Low-level operations requiring careful auditing (e.g., path sanitization, process spawning, signal handling).
- **Testable**: Pure functions or I/O wrappers that would benefit from isolated unit testing.

## 2. Categorize Candidates

Group your findings into logical modules (e.g., `fs`, `process`, `net`, `cert`, `ipc`).

## 3. Analyze Each Candidate

For each candidate, answer the following:

- **Current Location**: Where does it live now?
- **Description**: What does it do?
- **Why Extract**: Is it duplicated? Security-critical? Hard to test?
- **Novelty Check**:
  - Is this a standard problem solved by an existing crate?
  - If yes, should we use the crate? (Recommend `tokio-retry`, `path-clean`, etc. if appropriate).
  - If no, explain why our needs are unique (e.g., "requires specific integration with our sandbox/logging", "needs to handle symlinks securely in a way standard crates don't").

## 4. Generate Report

Output a structured report (or update an existing RFC) with your findings. Use the following format for each candidate:

### Candidate: [Name]

- **Current Location**: `path/to/file.rs`
- **Description**: ...
- **Why Extract**: ...
- **Novelty/External Crate Analysis**: ...
- **Proposed API**: (Optional) Sketch the public signature.

## 5. Prioritize

Rank the candidates by impact (e.g., "High" for security/circular dependency fixes, "Low" for general cleanup).
