# Axiom 6: 12-Factor Alignment

**The tool is designed for 12-factor apps.**

## Rationale

The 12-Factor App methodology is the industry standard for building scalable, portable, and resilient web apps. `locald` should encourage and rely on these practices.

## Implications

- **Config via Env**: `locald` configures services primarily by setting environment variables.
- **Logs to Stdout**: `locald` expects services to log to `stdout`/`stderr`. It will not tail log files.
- **Port Binding**: Services must export a service by binding to a port provided by the environment (`PORT`).
  - **Internal vs. External**: The service only cares about the `PORT` env var. It does _not_ know about its public domain (`app.local`).
  - **Routing**: `locald` handles the mapping from `app.local` (External) to `localhost:PORT` (Internal). This separation of concerns is critical.
- **Concurrency**: `locald` manages concurrency by running processes.
