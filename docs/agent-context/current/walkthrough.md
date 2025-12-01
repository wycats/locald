# Phase 11 Walkthrough: Docker Integration

## Overview
In this phase, we are adding support for running Docker containers as services.

## Key Decisions

### 1. Native Docker Support
We are wrapping the `docker` CLI directly rather than using `docker-compose`. This allows us to integrate container ports into our dynamic port assignment system.

### 2. Minimal Schema
We only support `image`, `container_port`, and `volumes`. Complex setups should use `docker-compose` via the `command` field.

## Changes

### Codebase
- (List changes here)

### Documentation
- (List changes here)
