---
stage: 3
---

# RFC 0093: Proxy Loading State

## Summary

This RFC introduces a "Loading State" mechanism in the `locald` reverse proxy. When a service accepts a connection but takes a long time to respond to an HTTP request (common with lazy-compiling web frameworks), `locald` serves a temporary "Building..." page instead of letting the browser hang. This page polls the backend and reloads when the application is ready.

## Motivation

Modern web development tools like **Vite**, **Astro**, and **Next.js** prioritize "Instant Server Start." In development mode (`npm run dev`), the HTTP server starts listening almost immediately, but the application logic (HTML/CSS/JS) is not compiled until the **first request** is received.

This creates a UX gap:

1. `locald` sees the port is open and marks the service as "Ready."
2. The user clicks "Open" in the dashboard.
3. The browser sends a request.
4. The backend framework starts compiling the page.
5. The browser tab spins and hangs, sometimes for 10-30 seconds.
6. The user thinks `locald` or the service is broken.

We need a way to indicate that the system is working and the build is in progress.

## Design

The solution is implemented entirely within the `locald-server` proxy layer (`src/proxy.rs`), requiring no changes to the underlying services.

### 1. The 500ms Rule

When the proxy forwards a request to a backend service, it races the backend's response against a **500ms timer**.

- **Fast Response (<500ms)**: The response is streamed back to the client immediately. This is the standard path for APIs and pre-built pages.
- **Slow Response (>500ms)**: If the backend is still thinking, `locald` assumes it is performing a lazy build.

### 2. Conditional Interception

We only intervene if:

1. The request is for **HTML** (`Accept: text/html`). We don't want to return HTML for a JSON API request or an image load.
2. The request does **not** have the `X-Locald-Passthrough` header (see below).

### 3. The "Building..." Page

If the timeout is hit, `locald` returns a `200 OK` response with a static HTML page. This page contains:

- A visual spinner.
- A message: "Building your application..."
- A small JavaScript snippet for polling.

### 4. Client-Side Polling & Passthrough

The JavaScript on the loading page performs the following loop:

1. `fetch(window.location.href, { headers: { 'X-Locald-Passthrough': 'true' } })`
2. The `X-Locald-Passthrough` header tells the `locald` proxy: **"Do not serve the loading page. Wait as long as it takes."**
3. The proxy forwards this request to the backend and awaits the real response, no matter how long it takes.
4. When the `fetch` completes (meaning the backend finally responded), the JavaScript calls `window.location.reload()`.
5. The browser reloads the page. Since the build is now cached/complete, the backend responds instantly (<500ms), and the user sees their app.

## Implementation Details

### Proxy Logic (`locald-server/src/proxy.rs`)

```rust
// Simplified logic
let is_passthrough = req.headers().contains_key("x-locald-passthrough");
let accepts_html = req.headers().get("accept").contains("text/html");

let backend_future = client.request(req);

if is_passthrough || !accepts_html {
    // Standard behavior: wait forever
    return backend_future.await;
}

// Race against timeout
tokio::select! {
    res = backend_future => res,
    _ = sleep(500ms) => serve_loading_page(),
}
```

### Edge Cases

- **APIs**: API requests usually don't accept `text/html`, so they will just hang until completion (standard behavior).
- **WebSockets**: The upgrade handshake happens before this logic, so WebSockets are unaffected.
- **Errors**: If the polling request fails (500/502), the JavaScript retries after a delay.

## Phase 2: High-Fidelity Build Logs (Refinement)

To align with the "Vercel-class" aesthetic and provide immediate feedback, the loading screen must not just be a spinner. It must show the **real-time build logs** of the service being waited on.

### Design Update

1.  **Log Streaming**: The loading page will open a WebSocket connection to `locald` (e.g., `/_locald/logs?service=<name>`).
2.  **Visuals**:
    - A high-quality terminal view (using `xterm.js` or a styled `pre` tag) will be embedded below the spinner.
    - The terminal should support ANSI colors to preserve the build tool's output fidelity (e.g., Vite's colorful output).
    - The container should be "glassy" or dark-themed to match the dashboard aesthetic.
3.  **Context**: The user should see exactly what the backend is doing (e.g., "Compiling src/App.tsx...").

### Implementation Strategy

- **Frontend**: Update the static HTML string in `proxy.rs` to include the log viewer logic.
- **Backend**: Ensure the proxy has access to the `LogManager` or can proxy the WebSocket to the existing log stream endpoint.

## Future Improvements

- **Configurable Timeout**: Some services might always be slow; we could make the 500ms threshold configurable.

## Phase 3: Smart Status Feedback (Refinement)

We identified a UX gap where the user sees a spinner but doesn't know if the service is actually "up" (TCP connected) or still booting.

### The "Lazy Build" Gap

1.  **System Health (TCP)**: The service binds the port. `locald` marks it "Healthy".
2.  **Application Readiness (HTTP)**: The service (e.g., Vite) receives the request and _starts_ compiling. This takes time.

During this gap, the user sees a generic "Building..." screen. We can do better by exposing the "System Health" status.

### Design Update

1.  **Unified Event Stream**: The WebSocket endpoint (`/api/logs`) is upgraded to stream the full `Event` enum (containing `Log` and `ServiceUpdate` variants), mirroring the SSE endpoint.
2.  **Dual-Channel Feedback**:
    - **WebSocket**: Provides real-time "System Health" updates. When the service binds the port, the loading screen updates to: _"Service is ready (TCP), waiting for HTTP response..."_.
    - **Polling**: Remains the source of truth for "Application Readiness" (when to reload the page).
3.  **Visual Feedback**: The text color changes (e.g., to green) when the TCP connection is established, providing positive reinforcement that the system is working.
