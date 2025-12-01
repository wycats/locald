<!-- agent-template start -->

# Ideas

Ideas for future work.

<!-- agent-template end -->

## Health Checks

- **Goal**: Allow `depends_on` to wait until a service is actually ready.
- **Mechanism**: HTTP probe, TCP connect, or command execution.

## Service Types

- **Goal**: Support non-network services (workers, cron jobs) that do not bind a port.
- **Mechanism**: Add `type = "worker"` to service config. Skip port assignment and TCP probe for workers.
