---
title: "Advanced Proxying Strategy"
stage: 3
feature: Networking
---

# RFC: Advanced Proxying Strategy

## 1. Summary

Support path-based routing and modern protocols (H2/H3).

## 2. Motivation

Simple port mapping isn't enough for microservices.

## 3. Detailed Design

Route `/api` to Service A. Support HTTP/2.

### Terminology

- **Path-based Routing**: Routing based on URL path.

### User Experience (UX)

Production-like environment locally.

### Architecture

Proxy logic update.

### Implementation Details

`pingora` or `hyper`.

## 4. Drawbacks

- Complexity.

## 5. Alternatives

- Nginx.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
