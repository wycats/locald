---
title: "Dependency Resolution: Topological Sort"
stage: 3
feature: Architecture
---

# RFC: Dependency Resolution: Topological Sort

## 1. Summary

Use a topological sort to determine the startup sequence of services based on dependencies.

## 2. Motivation

Services often depend on others (e.g., API depends on DB). Starting them in random order fails.

## 3. Detailed Design

Build a dependency graph from the `depends_on` config. Sort it topologically. Start services in that order.

### Terminology

- **Topological Sort**: A linear ordering of vertices such that for every directed edge uv, vertex u comes before v.

### User Experience (UX)

Services start in the correct order.

### Architecture

`DependencyGraph` struct.

### Implementation Details

Kahn's algorithm.

## 4. Drawbacks

- Circular dependencies are impossible (must detect and error).

## 5. Alternatives

- Retry loops (eventual consistency).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Parallel startup for independent branches.
