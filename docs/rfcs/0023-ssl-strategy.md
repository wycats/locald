---
title: "SSL Strategy: Pure Rust Stack"
stage: 3
feature: Architecture
---

# RFC: SSL Strategy: Pure Rust Stack

## 1. Summary

Use `rcgen` and `rustls` to implement a pure Rust SSL stack for `.localhost` domains.

## 2. Motivation

We need HTTPS for `.localhost` (Secure Context). We want to avoid external dependencies like `mkcert` or `openssl` binaries to keep the "Single Binary" promise.

## 3. Detailed Design

- Generate a Root CA.
- Install it into the system trust store (`locald trust`).
- Generate leaf certs on-the-fly using `ResolvesServerCert` in `rustls`.

### Terminology

- **Pure Rust**: No C dependencies or external binaries.

### User Experience (UX)

`locald trust` -> HTTPS works.

### Architecture

`CertManager` struct.

### Implementation Details

`rcgen` for generation. `rustls` for serving.

## 4. Drawbacks

- Re-implementing `mkcert` logic.

## 5. Alternatives

- Bundle `mkcert`.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
