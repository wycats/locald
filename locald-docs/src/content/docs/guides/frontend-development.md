---
title: Frontend Development
description: Developing React, Vue, Svelte, and other frontend apps with locald.
---

`locald` is a great companion for modern frontend development. It handles the "glue" between your frontend dev server and your backend services, ensuring everything runs on a consistent `.localhost` domain with valid SSL.

## The Dev Server Pattern

Most modern frontend frameworks (Vite, Next.js, Remix, Astro) come with a fast development server that supports Hot Module Replacement (HMR).

When using `locald`, you simply tell it to run your dev server command.

```toml
[services.web]
command = "npm run dev"
workdir = "./frontend"
```

## Port Configuration

`locald` assigns a random, free port to every service and injects it as the `PORT` environment variable. **Your dev server must listen on this port** to be accessible via the `locald` proxy.

### Vite

By default, Vite ignores the `PORT` environment variable. You need to configure it in `vite.config.ts`:

```typescript
// vite.config.ts
export default defineConfig({
  server: {
    // Use the PORT env var if available, otherwise default to 5173
    port: process.env.PORT ? parseInt(process.env.PORT) : 5173,
    strictPort: true, // Fail if the port is already in use
  },
});
```

### Next.js

Next.js accepts the port as a CLI argument. You can pass the `$PORT` variable in your `locald.toml`:

```toml
[services.web]
# Pass the injected $PORT to the next dev command
command = "npm run dev -- -p $PORT"
```

### Create React App (Webpack)

Create React App respects the `PORT` environment variable automatically, but it might ask for confirmation if the default port is taken. To force it to run without interaction:

```toml
[services.web]
command = "npm start"
env = { BROWSER = "none" } # Prevent opening a new tab
```

## Connecting to Backends

One of the biggest benefits of `locald` is simplified networking.

### Using `.localhost` Domains

If you have a backend service running in `locald` (e.g., `api`), it is accessible at `https://<project-name>.localhost`.

You can configure your frontend to make requests directly to this URL. Since `locald` provides valid SSL certificates, you won't get "Mixed Content" warnings.

```javascript
// In your frontend code
const API_URL = "https://my-app.localhost/api";
fetch(`${API_URL}/users`);
```

**Note on CORS**: Since your frontend (`web.my-app.localhost`) and backend (`my-app.localhost` or `api.my-app.localhost`) are on different subdomains, you will need to enable CORS on your backend.

### Using a Proxy (Recommended)

To avoid CORS issues, you can configure your frontend dev server to proxy API requests to your backend.

**Vite Example:**

```typescript
// vite.config.ts
export default defineConfig({
  server: {
    port: process.env.PORT ? parseInt(process.env.PORT) : 5173,
    proxy: {
      "/api": {
        // Use the backend's service URL
        target: process.env.API_URL || "http://localhost:3000",
        changeOrigin: true,
      },
    },
  },
});
```

In `locald.toml`, inject the backend's URL:

```toml
[services.web.env]
# Inject the backend URL (e.g., http://127.0.0.1:45231)
API_URL = "${services.api.url}"
```

## WebSocket & HMR

`locald` fully supports WebSockets, so Hot Module Replacement (HMR) works out of the box. The proxy correctly upgrades connections for live reloading.

If you encounter issues with HMR, ensure your dev server is configured to use the correct client port (443) since `locald` serves everything over HTTPS.

**Vite HMR Config:**

```typescript
server: {
  hmr: {
    // Force the client to connect via the standard HTTPS port
    clientPort: 443,
  },
}
```
