# Axiom 4: Process Ownership

**`locald` owns the child processes. It is a process manager, not just a proxy.**

## Rationale

To provide a "Heroku-like" or "Vercel-like" local experience, we need control over the lifecycle. We need to be able to restart a crashed service, stop a service to free up resources, or scale it (maybe).

## Implications

- **Supervisor Pattern**: `locald` spawns processes directly (not via a shell if possible, to avoid PID masking).
- **Signal Handling**: `locald` must forward signals (like SIGINT/SIGTERM) correctly or handle them to shut down services gracefully.
- **Environment Injection**: `locald` is responsible for constructing the environment for the child process (injecting `PORT`, `DATABASE_URL`, etc.).
- **Zombie Reaping**: As a parent process, `locald` must properly wait on and reap child processes to avoid zombies.
