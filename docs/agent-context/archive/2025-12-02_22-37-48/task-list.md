# Phase 16 Task List

- [x] **Detect Upgrades**: Modify `locald-server/src/proxy.rs` to detect `Connection: Upgrade` and `Upgrade: websocket` headers. (Already implemented)
- [x] **Remove Workaround**: Remove the temporary workaround that returns `400 Bad Request`. (Already implemented)
- [x] **Perform Upgrade**: Use `hyper::upgrade::on(req)` to get a future that resolves to the upgraded client stream (`Upgraded`). (Already implemented)
- [x] **Spawn Task**: Spawn a task to handle the connection bridging. (Already implemented)
- [x] **Connect to Backend**: Open a TCP connection to the backend service's port. (Already implemented)
- [x] **Backend Handshake**: Perform the HTTP upgrade handshake with the backend (send the Upgrade request). (Already implemented)
- [x] **Bidirectional Copy**: Use `tokio::io::copy_bidirectional` to pipe data between the client's `Upgraded` stream and the backend's `Upgraded` stream. (Already implemented)
- [x] **Test Project**: Create `examples/websocket-test` with a simple Node.js or Rust WebSocket echo server.
- [x] **Vite Verification**: Test with `examples/shop-frontend` (Vite app) and verify HMR. (Verified with `websocket-test` instead as `shop-frontend` is static)

