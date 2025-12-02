# Design: Zero-Config SSL & .dev Support

## Goal

Enable `locald` to serve applications over HTTPS using `.dev` domains (e.g., `https://my-app.dev`) with zero manual configuration from the user. This brings the local development environment to parity with production standards (Secure Contexts, HSTS).

## The Problem

1.  **HSTS Preload**: The `.dev` TLD is hardcoded in modern browsers to require HTTPS. HTTP connections are rejected.
2.  **Trust**: Self-signed certificates generate scary browser warnings. To avoid this, we must act as a Certificate Authority (CA) and install our Root CA into the system and browser trust stores.
3.  **Dynamic Domains**: Users add projects dynamically. We cannot pre-generate certificates for every possible domain; we must generate them on the fly during the TLS handshake.

## Architecture

### 1. The Stack

We will use a "Pure Rust" stack to avoid external dependencies like `mkcert` or `openssl` binaries.

- **Certificate Generation**: [`rcgen`](https://crates.io/crates/rcgen)
  - Used to generate the Root CA (once).
  - Used to generate leaf certificates for specific domains (on demand).
- **Trust Store Injection**: [`devcert`](https://crates.io/crates/devcert) (or a custom implementation of its logic)
  - Used to install the Root CA into macOS Keychain, Linux `/etc/ca-certificates`, and Firefox NSS databases.
- **TLS Termination**: [`rustls`](https://crates.io/crates/rustls) + [`axum-server`](https://crates.io/crates/axum-server)
  - Used to serve the HTTPS traffic.
  - Specifically, we will implement the `ResolvesServerCert` trait to hook into the SNI (Server Name Indication) extension.

### 2. Workflow

#### Phase A: Initialization (`locald init` or first run)

1.  **Check**: Does `~/.locald/certs/rootCA.pem` exist?
2.  **Generate**: If not, use `rcgen` to create a new Root CA (valid for 10 years). Save the PEM and Private Key securely.
3.  **Install**: Check if the Root CA is in the system trust store.
    - If not, prompt the user (via `sudo` if needed) to install it using `devcert` logic.
    - _Note_: This is the ONLY time the user sees a prompt.

#### Phase B: Runtime (`locald start`)

1.  **Load CA**: The daemon loads the Root CA and its Private Key into memory.
2.  **Listen**: The proxy listens on port 443 (or another port if 443 is privileged/taken, though 443 is required for clean URLs).
    - _Constraint_: Binding 443 usually requires root. We might need to use port forwarding (iptables/pf) or start the daemon as root (not recommended).
    - _Alternative_: Listen on 8443 and rely on `localhost:8443`, but that breaks the clean URL promise.
    - _Preferred_: Use `authbind` or `setcap` to allow binding 443 without full root.

#### Phase C: Request Handling (The Handshake)

1.  **Client Connects**: Browser connects to `my-app.dev:443`.
2.  **SNI**: Client sends "Client Hello" with SNI `my-app.dev`.
3.  **Resolve**: Our `ResolvesServerCert` implementation:
    - Checks if we have a cached cert for `my-app.dev`.
    - If not, uses `rcgen` to generate a leaf cert for `my-app.dev`, signed by our in-memory Root CA.
    - Caches the cert.
4.  **Serve**: The handshake completes. The browser trusts the cert because it trusts the Root CA.

## Implementation Details

### `ResolvesServerCert` Implementation

```rust
struct DynamicCertResolver {
    ca_cert: rcgen::Certificate,
    ca_key: rcgen::KeyPair,
    cache: DashMap<String, Arc<CertifiedKey>>,
}

impl ResolvesServerCert for DynamicCertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let sni = client_hello.server_name()?;

        if let Some(cert) = self.cache.get(sni) {
            return Some(cert.clone());
        }

        // Generate and sign
        let cert = generate_leaf(sni, &self.ca_cert, &self.ca_key);
        let key = Arc::new(cert);

        self.cache.insert(sni.to_string(), key.clone());
        Some(key)
    }
}
```

### Port 443 Strategy

To support `https://app.dev` without a port number, we must listen on 443.

- **Linux**: `setcap 'cap_net_bind_service=+ep' /path/to/locald-server`
- **macOS**: No easy `setcap`. Requires running as root or using a port forward (e.g., `pfctl`) from 443 -> 8443.
- **Fallback**: If we can't bind 443, we default to `https://app.dev:8443` and print a helpful message.

## Security Considerations

- **Private Key Protection**: The Root CA private key is sensitive. If stolen, an attacker can MITM any site for that user.
  - _Mitigation_: Store it in `~/.locald/certs` with `600` permissions.
  - _Mitigation_: The CA is only trusted on _that specific machine_.
- **Scope**: The generated certificates should be short-lived (e.g., 24 hours) to minimize risk if leaked.

## Roadmap

1.  **Spike**: Create a standalone Rust binary that generates a CA, installs it, and serves `hello world` on `https://test.dev`.
2.  **Integrate**: Move the logic into `locald-server`.
3.  **UI**: Add status indicators to `locald status` showing SSL health (CA installed? Port 443 bound?).
