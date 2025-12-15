# Vision

`locald` treats the local machine as a first-class platform.

In practical terms, the project aims to make "clone → `locald up`" feel as coherent as a cloud platform:

- Services are managed citizens (not random ports and terminals).
- `*.localhost` routing is the default UX (not memorizing port numbers).
- Observability is built-in (logs, status, and a dashboard workspace).
- The development loop prioritizes fast iteration while preserving correctness and safety.

## Relationship to Design Docs

- The design-oriented vision statement lives in [docs/design/vision.md](../design/vision.md).
- The manual is the source of truth for "how it works now"; if there is drift, update the manual.
