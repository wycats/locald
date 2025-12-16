---
title: "Gitignore: Automated Management"
stage: 3
feature: DX
---

# RFC: Gitignore: Automated Management

## 1. Summary

Automatically add `.locald/` to `.gitignore`.

## 2. Motivation

Users forget to ignore local state. We should do it for them.

## 3. Detailed Design

On `init` or `service add`, check `.gitignore`. If `.locald/` is missing, append it.

### Terminology

- **Automated Management**: The tool modifies project files to enforce best practices.

### User Experience (UX)

Less "oops, I committed my logs".

### Architecture

CLI logic.

### Implementation Details

File append.

## 4. Drawbacks

- Modifying user files.

## 5. Alternatives

- Warning only.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
