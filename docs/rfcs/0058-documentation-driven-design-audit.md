# RFC 0058: Documentation-Driven Design (DDD) Audit

- **Status**: Stage 0 (Strawman)
- **Date**: 2025-12-06
- **Tags**: documentation, refactoring, design, audit

## Summary

We are executing a **Documentation-Driven Design (DDD) Campaign** for the `locald` codebase. The core philosophy is that documentation is a stress test for design. If a component is difficult to document (hard to explain, unsafe to call, or hard to test), it is a **Design Flaw**.

## Motivation

As the codebase grows, we need to ensure that it remains maintainable, secure, and easy to understand for new contributors. A thorough audit using specific "personas" will help us identify friction points and refactor them before they become entrenched technical debt.

## The Persona Library (Audit Lenses)

### 1. The New Hire (Cognitive Load Audit)

_Focus: Naming, Magic Constants, Tribal Knowledge._
**The Friction Test:**

- "Do I need to read the function body to understand the arguments?" -> **Rename Arguments/Types**.
- "Are there boolean flags (`true/false`)?" -> **Refactor to Enums**.
- "Are there 'Magic Numbers'?" -> **Extract to Constants**.

### 2. The Security Auditor (Safety Audit)

_Focus: Invariants, Panics, Invalid States._
**The Friction Test:**

- "Do I need to manually check for empty strings/nulls?" -> **Create Validated Types**.
- "Can I call methods in the wrong order (e.g., `run` before `init`)?" -> **Use TypeState / Builder**.
- "Does it panic on bad input?" -> **Return Result**.

### 3. The Test Engineer (Determinism Audit)

_Focus: Side Effects, Hard Dependencies, Testability._
**The Friction Test:**

- "Does the Doctest require >6 lines of setup?" -> **Add Helpers / Defaults**.
- "Does it implicitly read Files/Env?" -> **Use Dependency Injection**.
- "Is it impossible to mock?" -> **Extract Trait**.

### 4. The SRE (Operability Audit)

_Focus: Failure Modes, Debugging, Metrics._
**The Friction Test:**

- "Are errors just strings?" -> **Use Error Enums**.
- "Does it fail silently?" -> **Add Tracing with Context**.
- "Is the 'Happy Path' the only thing documented?" -> **Document Failure Modes**.

## Execution Protocol

1.  **Audit First:** Do not just write comments. Audit the code using the "Persona Library".
2.  **Refactor Friction:** If the code fails a persona's test, **Refactor the Code** to remove the friction.
3.  **Document Last:** Once the code is clean, write high-quality Rustdoc (`///`) and Doctests (` ```rust `).
