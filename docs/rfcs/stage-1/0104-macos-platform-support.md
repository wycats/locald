<!-- exo:104 ulid:01krkpxdv1v9zxe358vtvcxe9f -->

# RFC 104: macOS Platform Support Strategy


# RFC 0104: macOS Platform Support Strategy

## 1. Summary

This RFC defines the comprehensive strategy for macOS platform support in locald, reconciling the competing approaches and providing a clear implementation path.

## 2. Motivation

macOS presents unique challenges for locald:

1. **No native Linux containers**: macOS cannot run Linux binaries (ELF) natively
2. **Certificate trust requirements**: macOS requires special handling for local HTTPS certificates
3. **Privilege escalation**: macOS uses different mechanisms than Linux for privileged operations
4. **File system differences**: Case-insensitive by default, different permission models

Previous RFCs (0047, 0061, 0098, 0101) have addressed pieces of this puzzle. This RFC consolidates and reconciles those visions.

## 3. Design Decisions

### 3.1 Virtualization Strategy: Lima

Per RFC 0047, we adopt **Lima** (Linux Machines) as the virtualization layer for macOS:

- Uses macOS native virtualization (`vz` framework)
- Supports efficient file sharing via `virtiofs`
- Open-source and actively maintained
- No Docker Desktop dependency

### 3.2 Certificate Trust

For local HTTPS development on macOS:

1. **System Keychain Integration**: Use `security add-trusted-cert` for root CA
2. **Per-User Certificates**: Store in `~/Library/Application Support/locald/certs/`
3. **Automatic Trust Prompt**: Guide users through the trust dialog

### 3.3 Privilege Separation

macOS approach differs from Linux:

- No setuid binaries (SIP restrictions)
- Use `launchd` for privileged daemons
- Privileged helper tool pattern for port binding

## 4. Implementation Plan

### Phase 1: Basic macOS Support

- [ ] Detect macOS environment
- [ ] Implement Lima runtime wrapper
- [ ] Basic service execution via Lima VM

### Phase 2: Certificate Integration

- [ ] Implement keychain integration for certificate trust
- [ ] Add `locald admin setup` for macOS

### Phase 3: Privileged Operations

- [ ] Implement privileged helper for port 80/443 binding
- [ ] Add launchd integration for daemon management

## 5. Related RFCs

- RFC 0047: Cross-Platform Container Runtime Strategy
- RFC 0061: Certificate Authority Management
- RFC 0098: Privilege Escalation Patterns
- RFC 0101: Platform Detection

## 6. Unresolved Questions

- Should we support Apple Silicon native containers in the future?
- How do we handle Rosetta 2 for x86_64 workloads?
