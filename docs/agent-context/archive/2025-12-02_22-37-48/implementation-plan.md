# Phase 16 Implementation Plan: WebSocket Support

**Goal**: Enable full WebSocket support in the reverse proxy to support modern web apps and dev tools (HMR).

## 1. Upgrade Handling

- [ ] **Detect Upgrades**:
  - Modify `locald-server/src/proxy.rs` to detect `Connection: Upgrade` and `Upgrade: websocket` headers.
  - Remove the temporary workaround that returns `400 Bad Request`.
- [ ] **Perform Upgrade**:
  - Use `hyper::upgrade::on(req)` to get a future that resolves to the upgraded client stream (`Upgraded`).
  - Spawn a task to handle the connection bridging.

## 2. Connection Bridging

- [ ] **Connect to Backend**:
  - Open a TCP connection to the backend service's port.
  - Perform the HTTP upgrade handshake with the backend (send the Upgrade request).
  - _Implementation Detail_: We might need to manually construct the upgrade request to the backend or use `hyper`'s client upgrade capabilities if available/exposed in `legacy::Client`.
- [ ] **Bidirectional Copy**:
  - Use `tokio::io::copy_bidirectional` to pipe data between the client's `Upgraded` stream and the backend's `Upgraded` stream.

## 3. Verification

- [ ] **Test Project**:
  - Create `examples/websocket-test` with a simple Node.js or Rust WebSocket echo server.
- [ ] **Vite Verification**:
  - Test with `examples/shop-frontend` (Vite app).
  - Verify HMR works and the browser console doesn't show 400 errors for WS connections.

## 4. User Verification

- [ ] **Manual Check**:
  - Run `locald run` on `examples/websocket-test`.
  - Open a browser console or use `wscat -c wss://websocket-test.localhost`.
  - Send a message and verify it is echoed back.
- [ ] **Vite HMR Check**:
  - Run `locald run` on `examples/shop-frontend`.
  - Open the app in the browser.
  - Edit a file in `examples/shop-frontend`.
  - Verify the change is reflected in the browser without a full reload (HMR).
