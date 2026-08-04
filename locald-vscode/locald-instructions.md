This project uses **locald** for local development services.

- locald-managed services run automatically. Externally published services keep their process lifecycle in the owning workflow; follow the publication state and next-step guidance instead of trying to start or restart them through locald.
- Do not suggest `npm start`, `docker-compose up`, `rails server`, or similar manual startup commands unless a published service's locald guidance explicitly identifies that owning workflow.
- Use the locald service inspection tool to check service state, health, and semantic URLs.
- Service URLs use HTTPS on `*.localhost` domains with a trusted local CA. They work in the integrated browser without certificate warnings.
- The integrated browser can reach these URLs directly — use it to preview changes.
- Before browser or testing work that requires a live service, use `locald_ensure` to wait for authoritative managed readiness and receive semantic origins plus publication state. Use `locald_open` when the next action is visual verification in the integrated browser; an unavailable published origin explains its current state and next step.
- Reading status or historical logs is observational and does not start a paused project.
