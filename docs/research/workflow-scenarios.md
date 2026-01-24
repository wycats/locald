# Workflow Scenarios

> First-person narratives describing what "locald working as intended" feels like.
> These are not feature specs—they're phenomenological anchors.

---

## Scenario 0: The Daily Driver (Working Today)

I'm building a full-stack app with a frontend, backend, and Postgres. I open my terminal, `cd` into my project, run `locald up`. A progress UI shows services spinning up—Postgres first (because my API depends on it), then my API. Everything goes green.

I see `https://myapp.localhost`. I click it. HTTPS, no warnings. My app is running.

I need to check something in the API logs. I open `https://locald.localhost`—the dashboard shows my services with status indicators. I click into my API, see the logs streaming live. The error is right there.

Later, I need to run a migration. I type `locald run api -- npx prisma migrate dev`. It runs with the exact same `DATABASE_URL` my running API has—same port, same credentials, same everything. The migration succeeds.

I add Redis to `locald.toml`:

```toml
[services.redis]
type = "container"
image = "redis:7"
```

I save. Redis spins up automatically. I add `REDIS_URL = "${services.redis.url}"` to my API's env, save again. My API restarts with the new env var. No restart command. No ceremony.

End of day: `locald stop`. Tomorrow: `locald up`. Same state, same URLs, same ports. Predictable.

**What this replaces:**

- Remembering "wait, is the API on 3000 or 3001?"
- Running `docker-compose up -d` and hoping the ports don't conflict
- Maintaining a `.env` file that drifts from reality
- Certificate warnings when testing OAuth
- Asking "is my database still running?"

**First-run experience:**

- Running `locald up` in a fresh project prompts for one-time `locald admin setup` (offers `pkexec` if available, `sudo` otherwise)
- This installs the privileged shim AND sets up HTTPS trust—no separate `locald trust` needed
- If trust fails during setup, `locald trust` exists as a standalone fix

**Friction points that exist today:**

- `type = "container"` with a Docker image requires Docker daemon running (OCI buildpack containers don't)
- Application source code hot-reload is your tooling's job (nodemon, air, etc.)—locald only watches `locald.toml`/`Procfile`/`.env`

---

## Scenario 1: Clone-to-Working

I just cloned a repo from a colleague. I've never seen this codebase before. I `cd` into it, run `locald up`. The terminal shows services spinning up—I see "web" and "api" appear, then go green. A URL appears. I click it. The app is running. I didn't install anything. I didn't read a README. I didn't configure my environment. I'm just... working.

**Friction points that would break this:**

- Needing to install language runtimes first
- Needing to run `npm install` or equivalent manually
- Needing to set up a database
- Needing to configure environment variables
- The URL not being clickable or obvious

---

## Scenario 2: Adding a Dependency Mid-Flow

I'm deep in a feature. I realize I need Redis for caching. I don't want to context-switch. I open `locald.toml`, add:

```toml
[services.redis]
plugin = "redis"
```

I save. I glance at the dashboard—Redis is spinning up. A few seconds later, it's green. I check my service's environment: `REDIS_URL` is there. I continue coding. I never left my mental context. No restart. No ceremony.

**Friction points that would break this:**

- Needing to restart the daemon
- Needing to run a command to "apply" the change
- The env var not being automatically injected
- Having to look up how to configure Redis

---

## Scenario 3: Debugging a Service

My API is returning 500s. I open the dashboard, click on "api". I see recent logs. The error is right there—a typo in my database query. I fix it, save. The service restarts automatically. I refresh my browser. Fixed. The cycle was: see error → fix → verify. No digging through terminal history. No `docker logs`. No wondering "which container is this?"

**Friction points that would break this:**

- Logs not being in an obvious place
- Having to figure out which service is which
- Manual restart required after code change
- Logs being truncated or hard to scroll

---

## Scenario 4: Sharing a Running State

A teammate pings me: "I can't reproduce the bug you're seeing." I tell them: "clone main, `locald up`, hit `/api/users`." They do. They see the same thing I see. Our environments are identical—not "similar," not "hopefully the same"—_identical_. The project config is the environment. There's nothing else to sync.

**Friction points that would break this:**

- Per-machine environment variables leaking in
- Different database states
- Services running on conflicting ports
- "Works on my machine" for any reason

---

## Scenario 5: HTTPS Just Works

I'm building an OAuth integration. I need HTTPS. I don't think about it—I just use the `https://` URL that's already in the dashboard. The certificate is valid. My browser doesn't complain. The OAuth callback works. I never ran `mkcert`. I never touched a certificate. It's just... there.

**Friction points that would break this:**

- Certificate warnings
- Needing to run a trust setup command
- Needing to manually configure cert paths
- Different behavior on first run vs. subsequent runs

---

## Scenario 6: The Dashboard as Home Base

I have three projects running. I open the dashboard. I see all three. I click into one—I see its services, their status, their URLs. I click a URL, the site opens. I click "logs," I see logs. I click "stop," it stops. The dashboard is where I go to understand "what's running and how do I interact with it?" It's not a monitoring tool—it's my workspace.

**Friction points that would break this:**

- Dashboard not showing all projects
- Stale state (shows running when stopped)
- No clear way to get to a service's URL
- Needing to use the CLI for basic operations

---

## Scenario 7: Stopping Everything

I'm done for the day. I run `locald down`. Everything stops. Not "some things might still be running." Not "check for zombie processes." Everything. When I run `locald up` tomorrow, it starts fresh. I don't manage processes. locald manages processes.

**Friction points that would break this:**

- Orphaned processes
- Having to hunt for PIDs
- State persisting unexpectedly
- Needing to kill things manually

---

## Scenario 8: A Worker That Runs Alongside

I have a background job processor. It's defined in `locald.toml` as a worker. When I `locald up`, it starts with everything else. It doesn't have a port—it's not a web service—but I can see its logs in the dashboard. If it crashes, I see that too. It's a first-class citizen, not an afterthought.

**Friction points that would break this:**

- Workers needing special configuration
- No visibility into worker status
- Workers not restarting on code change
- Having to run workers separately

---

## Scenario 9: Frontend and Backend Together

My project has a React frontend and a Rust API. They're separate services in `locald.toml`. The frontend proxies API calls through locald—I use relative URLs like `/api/users`. No CORS. No port juggling. When I change the frontend, it hot-reloads. When I change the API, it restarts. They're developed in parallel, in one terminal, with one command.

**Friction points that would break this:**

- Having to configure proxy rules
- Port conflicts
- CORS issues
- Needing multiple terminals

---

## Scenario 10: Onboarding a New Team Member

A new developer joins the team. They clone the repo. They run `locald up`. They're productive in 10 minutes. Not "they can build the project"—they're _productive_. They can make changes, see results, understand the architecture from the dashboard. The README says "run `locald up`" and that's not a lie.

**Friction points that would break this:**

- Additional setup steps
- Tribal knowledge requirements
- "You also need to..."
- Platform-specific instructions

---

## Next Steps

- [ ] Review which scenarios resonate most strongly
- [ ] Identify scenarios that feel aspirational vs. already-working
- [ ] Combine/refine into canonical 3-5
- [ ] Use as regression targets for UX decisions
