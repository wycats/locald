This project uses **locald** for local development services.

- Services are running and managed automatically. Do not suggest `npm start`, `docker-compose up`, `rails server`, or similar manual startup commands.
- Run `locald project status . --json` to check service state (health, URLs, ports).
- Service URLs use HTTPS on `*.localhost` domains with a trusted local CA. They work in the integrated browser without certificate warnings.
- The integrated browser can reach these URLs directly — use it to preview changes.
- If a service is unhealthy, try `locald project start .` to restart it.
- Log output is available via `locald logs` (use `--no-follow` for a snapshot).
