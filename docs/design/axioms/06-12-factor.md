# Axiom 6: 12-Factor Alignment

**The tool is designed for 12-factor apps.**

## Rationale

The 12-Factor App methodology is the industry standard for building scalable, portable, and resilient web apps. `locald` should encourage and rely on these practices.

## Implications

- **Config via Env**: `locald` configures services primarily by setting environment variables.
- **Logs to Stdout**: `locald` expects services to log to `stdout`/`stderr`. It will not tail log files.
- **Port Binding**: Services must export a service by binding to a port provided by the environment.
- **Concurrency**: `locald` manages concurrency by running processes.
