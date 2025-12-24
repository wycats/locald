---
title: "Axiom 4: Process Ownership"
---


**`locald` owns the child processes. It is a process manager, not just a proxy.**

## Rationale

To provide a "Heroku-like" or "Vercel-like" local experience, we need control over the lifecycle. We need to be able to restart a crashed service, stop a service to free up resources, or scale it (maybe).

## Implications

- **Supervisor Pattern**: `locald` spawns processes directly.
- **Signal Handling**: `locald` implements a robust graceful shutdown protocol:
  1.  **Process Groups**: Services are spawned in their own process groups. This ensures that signals propagate to the entire process tree (e.g., shell wrappers and the actual app).
  2.  **Graceful Termination**: When stopping a service, `locald` sends `SIGTERM` to the process group.
  3.  **Timeout**: `locald` waits up to **5 seconds** for the process to exit voluntarily.
  4.  **Force Kill**: If the process does not exit within the timeout, `locald` sends `SIGKILL` to the process group to ensure cleanup.
  5.  **Parallelism**: During daemon shutdown or updates, all services are signaled in parallel to minimize wait time.
- **Environment Injection**: `locald` is responsible for constructing the environment for the child process (injecting `PORT`, `DATABASE_URL`, etc.).
- **Zombie Reaping**: As a parent process, `locald` must properly wait on and reap child processes to avoid zombies.

