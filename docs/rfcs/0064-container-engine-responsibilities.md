---
title: Formalizing Container Engine Responsibilities
stage: 0
---

# Formalizing Container Engine Responsibilities

## Problem

Building a container engine involves a significant amount of "implicit knowledge" that is not strictly defined in the OCI Image Specification (which deals with static artifacts) or the Cloud Native Buildpacks Specification (which deals with the build lifecycle).

When `locald` acts as a container engine (invoking `runc`), it is responsible for bridging the gap between a static root filesystem and a functioning Linux environment. Failing to do so results in brittle failures (e.g., missing DNS, user lookup failures) that appear mysterious to the user.

Currently, these requirements are discovered through trial and error ("hacking it until it works").

## Proposal

We will create a formal **Container Engine Specification** for `locald`. This document will explicitly list the responsibilities `locald` assumes when launching a container, independent of the specific workload (CNB, Service, etc.).

For each requirement, we will document:

1.  **The Action**: What `locald` must do (e.g., "Synthesize `/etc/passwd`").
2.  **The Provenance**: Why this is required and where the requirement comes from (e.g., "POSIX Standard", "Docker Convention", "Linux Kernel Requirement").
3.  **The Implementation**: How `locald` satisfies this requirement.

## Scope

This specification covers:

- **System Files**: `/etc/passwd`, `/etc/group`, `/etc/resolv.conf`, `/etc/hosts`.
- **Mounts**: Standard kernel filesystems (`/proc`, `/sys`, `/dev`, `/dev/pts`, `/dev/shm`).
- **Devices**: Essential devices (`/dev/null`, `/dev/zero`, etc.).
- **Environment**: Standard environment variables (`PATH`, `HOME`, `TERM`).
- **Signals**: PID 1 responsibilities and signal propagation.

## Goals

1.  **Robustness**: Eliminate "it works on my machine" issues by standardizing the runtime environment.
2.  **Clarity**: Transform "tribal knowledge" into explicit engineering requirements.
3.  **Compliance**: Ensure we are meeting the expectations of standard Linux tools and the CNB lifecycle.
