---
title: "Axiom 10: Structured Service Hierarchy"
---


**Services are not isolated entities; they exist within a context.**

The system must understand and represent the hierarchy of **System > Constellation > Project > Service**. It should prioritize user content while keeping system utilities accessible but distinct.

## Implications

- **Grouping**: Services should be groupable into logical units (Constellations) beyond just the directory they live in.
- **System Services**: The tools that run the environment (Dashboard, Docs) are distinct from the work being done in the environment.
- **Inheritance**: Configuration should cascade down the hierarchy (Global -> Constellation -> Project).
- **Resource Scoping**: Resources (like databases) can be scoped to any level of the hierarchy (Project, Constellation, Global) and attached to multiple services.
- **Navigation**: The UI must reflect this nesting to reduce cognitive load.

