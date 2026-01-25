---
title: "Language: Rust"
stage: 3
feature: Architecture
---

# RFC: Language: Rust

## 1. Summary

The project shall be written in Rust.

## 2. Motivation

We require a language that offers high performance, memory safety, and the ability to compile to a single, self-contained binary. Rust meets all these criteria.

## 3. Detailed Design

Rust provides zero-cost abstractions, a strong type system, and a robust ecosystem for systems programming.

### Terminology

- **Rust**: The programming language.
- **Cargo**: The package manager and build tool.

### User Experience (UX)

Users will install the tool as a single binary, without needing to manage a runtime environment (like Node.js or Python).

### Architecture

The codebase will be organized as a Cargo workspace.

### Implementation Details

We will use stable Rust.

## 4. Drawbacks

- Steeper learning curve for contributors not familiar with Rust.
- Longer compile times compared to interpreted languages.

## 5. Alternatives

- **Go**: Good for networking, but Rust's type system is preferred for correctness.
- **Node.js**: Easier for web developers, but requires a runtime and has higher resource usage.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
