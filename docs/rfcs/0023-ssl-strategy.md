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

- **Pure Rust**: No C dependencies or external binaries for cert generation/serving.

### User Experience (UX)

`locald trust` -> HTTPS works.

### Architecture

`CertManager` struct.

### Implementation Details

- **Generation**: `rcgen` for certificate generation.
- **Serving**: `rustls` for TLS termination.
- **Trust Installation (Platform-Specific)**:
  - **Linux**: Copy cert to `/usr/local/share/ca-certificates/`, run `update-ca-certificates`
  - **macOS**: Use `security-framework` crate for native Keychain API access. Specifically, `TrustSettings::set_trust_settings_always()` to mark the root CA as trusted.

### macOS Certificate Trust

**Implementation Decision**: Use the `security-framework` crate instead of shelling to the `security` CLI.

```rust
#[cfg(target_os = "macos")]
fn trust_root_ca(cert: &Certificate) -> Result<()> {
    use security_framework::trust_settings::{TrustSettings, Domain};
    use security_framework::certificate::SecCertificate;
    
    let sec_cert = SecCertificate::from_der(cert.to_der()?)?;
    TrustSettings::set_trust_settings_always(&sec_cert, Domain::Admin)?;
    Ok(())
}
```

**Rationale**:
- **Type Safety**: Native API returns structured errors
- **Reliability**: No shell escaping or output parsing
- **Consistency**: Matches Rust ecosystem patterns (cf. `native-tls`)

## 4. Drawbacks

- Re-implementing `mkcert` logic.

## 5. Alternatives

- Bundle `mkcert`.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
