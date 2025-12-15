---
title: Managed Infrastructure
description: Run databases and queues without Docker or manual installs.
---

Modern applications rarely run in isolation. They need databases, caches, and queues.

Traditionally, setting these up locally meant one of two evils:

1.  **Manual Install**: `brew install postgresql`, then fighting with `pg_hba.conf` and port conflicts.
2.  **Docker Compose**: Writing a 50-line YAML file just to get a database, and then dealing with volume mounting and networking issues.

`locald` offers a third way: **Managed Services**.

## The "Managed" Philosophy

Just as AWS RDS manages a database for you in the cloud, `locald` manages it on your machine.

- **Binary Management**: `locald` downloads the correct binary for your OS/Arch.
- **Lifecycle**: It starts/stops the database with your project.
- **Configuration**: It handles ports, sockets, and data directories.
- **Wiring**: It injects connection strings directly into your app.

## Managed Postgres

The `postgres` service type provides a fully managed PostgreSQL instance.

### 1. Add the Service

You don't need to edit config files manually. Use the CLI to "provision" your database:

```bash
locald service add postgres db
```

This updates your `locald.toml`:

```toml
[services.db]
type = "postgres"
# version = "15" (Optional)
```

### 2. The Magic Wiring

This is the most important part. **Do not hardcode `localhost:5432`**.

`locald` assigns a random, free port to your database to avoid conflicts. It then exposes the connection details via **Variable Interpolation**.

Update your app service to use the `${services.db.url}` variable:

```toml
[services.api]
command = "npm run dev"

[services.api.env]
# This is the magic. locald resolves this at runtime.
DATABASE_URL = "${services.db.url}"
```

When `api` starts, it sees:
`DATABASE_URL=postgres://postgres:password@127.0.0.1:45123/postgres`

### 3. Data Persistence

Your data lives in `.locald/data/postgres-<name>`.
This folder is:

- **Local**: Fast disk access.
- **Isolated**: Specific to this project.
- **Ignored**: Automatically added to `.gitignore`.

### 4. Resetting State

Development often requires a clean slate. Instead of manually dropping tables, you can "nuke" the entire database service:

```bash
locald service reset db
```

This stops the process, deletes the data directory, and restarts it fresh. Perfect for testing migration scripts.

## Supported Services

Currently, `locald` supports:

- **Postgres**: The world's most advanced open source relational database.

_Coming Soon:_

- **Redis**: In-memory data structure store.
- **MySQL**: The world's most popular open source database.
- **MinIO**: S3-compatible object storage.
