This project uses **locald** for local development services.

- Services are running and managed automatically. Do not suggest `npm start`, `docker-compose up`, `rails server`, or similar manual startup commands.
- Use the locald service inspection tool to check service state, health, and semantic URLs.
- Service URLs use HTTPS on `*.localhost` domains with a trusted local CA. They work in the integrated browser without certificate warnings.
- The integrated browser can reach these URLs directly — use it to preview changes.
- Before browser or testing work that requires a live service, use `locald_ensure` to wait for authoritative readiness and receive semantic URLs. Use `locald_open` when the next action is visual verification in the integrated browser.
- Reading status or historical logs is observational and does not start a paused project.
