# Phase 18: Documentation Fresh Eyes & Self-Hosting

## Self-Hosted Documentation

We implemented self-hosted documentation directly within the `locald` binary.

### Implementation Details

1.  **Embedded Assets**: We used `rust-embed` to embed the static documentation site (built with Astro Starlight) into the `locald-server` binary.
2.  **Build Script**: A `build.rs` script in `locald-server` automatically copies the build artifacts from `locald-docs/dist` to `locald-server/src/assets/docs` during the build process.
3.  **Proxy Routing**: The `ProxyManager` in `locald-server/src/proxy.rs` was updated to intercept requests to `docs.localhost` (and `docs.local`) and serve the embedded documentation assets.
4.  **Fallback Handling**: The routing logic handles `index.html` resolution and falls back to `404.html` for missing pages.

### Verification

- Built the documentation using `pnpm build` in `locald-docs`.
- Recompiled `locald` with the embedded assets.
- Verified that `http://docs.localhost:8081` (and port 80 if privileged) serves the documentation correctly.
