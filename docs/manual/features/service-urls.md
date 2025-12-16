# Service URLs & Routing

`locald` automatically assigns URLs to your services based on your `locald.toml` configuration and the service type. This ensures that your development environment mirrors production routing conventions without manual configuration.

## The Project Domain

Every project has a base domain. You can explicitly define it in `locald.toml`, or let `locald` generate a default.

```toml
[project]
name = "shop"
# Optional. Defaults to "shop.localhost"
domain = "shop.localhost"
```

## URL Assignment Logic

`locald` uses the **Service Name**, **Project Domain**, and **Service Type** to determine the URL.

### 1. The "Main" Service (Root Domain)

The service named **`web`** (or a service with the same name as the project) is considered the "Main" service. It is mapped directly to the project domain.

- **Service Name**: `web`
- **Project Domain**: `shop.localhost` (Explicit or Default)
- **Assigned URL**: `http://shop.localhost`

### 2. Auxiliary Services (Subdomains)

Other web-facing services are mapped to subdomains of the project domain.

- **Service Name**: `docs`
- **Assigned URL**: `http://docs.shop.localhost`

- **Service Name**: `api`
- **Assigned URL**: `http://api.shop.localhost`

### 3. Data Services (Protocol URLs)

Services with specific types (like `postgres`, `redis`) are assigned protocol-specific URLs. These are connection strings rather than web links.

- **Service Name**: `db` (`type = "postgres"`)
- **Assigned URL**: `postgres://postgres:postgres@localhost:5432/postgres`
- **Dashboard Behavior**: These URLs are displayed for reference but do not open in a browser.

### 4. Workers (No URL)

Services that do not bind a port, or are explicitly marked as workers, have no public URL.

- **Service Name**: `worker` (`type = "worker"`)
- **Assigned URL**: _None_

## HTTPS & Port Handling

- **HTTPS**: If the `locald` proxy is handling SSL (port 443), URLs will default to `https://`.
- **Direct Ports**: If the proxy is bypassed or unavailable, the URL will fall back to the direct localhost port (e.g., `http://localhost:3000`).
