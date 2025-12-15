# Phase 16 Walkthrough: WebSocket Support

**Goal**: Enable full WebSocket support in the reverse proxy to support modern web apps and dev tools (HMR).

## Changes

<!-- agent-template start -->
### Verification of Existing Implementation

Upon inspection, the WebSocket upgrade logic was already present in `locald-server/src/proxy.rs`. This likely happened during the "Polish" phase or when merging binaries.

The implementation correctly:
1. Detects `Upgrade: websocket` headers.
2. Upgrades the client connection using `hyper::upgrade::on`.
3. Connects to the backend and performs the upgrade handshake.
4. Bridges the two upgraded streams using `tokio::io::copy_bidirectional`.

### Test Project

Created `examples/websocket-test` to verify the functionality.
- `server.js`: A simple Node.js WebSocket echo server.
- `client.js`: A Node.js WebSocket client that connects to `wss://websocket-test.localhost`.

Verified that the client can connect, send a message, and receive an echo, confirming that the proxy correctly handles the WebSocket upgrade and data transfer.

### Cleanup

- Updated `examples/shop-frontend/locald.toml` to use `.localhost` domain instead of `.local`.
<!-- agent-template end -->
