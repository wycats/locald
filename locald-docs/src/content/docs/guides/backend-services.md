---
title: Backend Services
description: Running APIs, workers, and background jobs with locald.
---

`locald` is designed to run your backend services in a production-like environment on your local machine. It manages ports, environment variables, and dependencies so you can focus on writing code.

## The Golden Rule: Bind to `$PORT`

The most important requirement for any network service running in `locald` is that **it must listen on the port specified by the `PORT` environment variable**.

`locald` assigns a random, free port to your service at runtime. This prevents port conflicts (the "Port 3000 problem") and allows you to run multiple projects simultaneously.

### Examples

**Node.js (Express):**

```javascript
const port = process.env.PORT || 3000;
app.listen(port, () => {
  console.log(`Server running on port ${port}`);
});
```

**Rust (Axum):**

```rust
let port = std::env::var("PORT").unwrap_or("3000".to_string());
let addr = format!("0.0.0.0:{}", port);
let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
```

**Python (FastAPI/Uvicorn):**

```toml
# locald.toml
[services.api]
command = "uvicorn main:app --port $PORT"
```

**Go:**

```go
port := os.Getenv("PORT")
if port == "" {
    port = "8080"
}
http.ListenAndServe(":"+port, nil)
```

## Configuration & Environment

Stop hardcoding secrets and config in your code. `locald` lets you inject environment variables directly into your service.

```toml
[services.api]
command = "./api"
env = { RUST_LOG = "debug", API_KEY = "secret" }
```

### Variable Interpolation

You can reference configuration from other services. This is perfect for connecting to databases or other APIs.

```toml
[services.api.env]
# Connect to the managed Postgres service
DATABASE_URL = "${services.db.url}"
# Connect to another internal service
AUTH_SERVICE_URL = "http://${services.auth.host}:${services.auth.port}"
```

## Health Checks

`locald` needs to know when your service is ready to accept traffic. This ensures that dependent services (like your frontend) don't start until the backend is actually up.

### HTTP Check (Recommended)

If your service has a health endpoint, use it!

```toml
[services.api]
health_check = { type = "http", path = "/healthz" }
```

### TCP Check

If you just want to know when the port is open:

```toml
[services.api]
health_check = { type = "tcp" }
```

### Command Check

For more complex logic, run a script:

```toml
[services.api]
health_check = "curl -f http://localhost:$PORT/ready"
```

## Background Workers

Not all services need a port. Background workers, cron jobs, and queue consumers should be defined with `type = "worker"`.

Workers:

- Do **not** get a `PORT` assigned.
- Are **not** reachable via the proxy.
- Are monitored for exit codes (restarted on failure).

```toml
[services.worker]
type = "worker"
command = "celery -A proj worker"
depends_on = ["redis"]
```

## Logging

`locald` captures `stdout` and `stderr` from your services.

- **View Logs**: Run `locald logs <service>` or use the Dashboard.
- **Stream Logs**: `locald logs -f <service>`.
- **Structured Logging**: If your app outputs JSON logs, `locald` will (in the future) parse and format them nicely. For now, ensure your logger writes to `stdout`.

## Hot Reloading

For backend development, you often want the server to restart when you change code. `locald` runs whatever command you specify, so use a file watcher or dev tool.

**Node.js (Nodemon):**

```toml
command = "nodemon server.js"
```

**Rust (Cargo Watch):**

```toml
command = "cargo watch -x run"
```

**Go (Air):**

```toml
command = "air"
```
