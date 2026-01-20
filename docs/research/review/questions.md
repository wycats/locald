Questions to Answer Before Launch Review

1. What is your 1-sentence pitch for locald?

locald manages all of your local development environments, giving them local domains and SSL certificates automatically, booting them and keeping them up, and giving you a single CLI and Dashboard to manage them all. It's like Heroku for local development.

2. Are there features you already know should be OUT for v1.0?

I'm more interested in finding a well-defined scope to talk about then specifically _culling_ features. Once we know the pitch and refine the core, we can decide whether to remove things, tag things as experimental, hide them from docs/CLI, or what have you. But for now, I want to focus on defining _and refining_ the core so I can start teasing it and ultimately make an announcement.

3. What is your target timeline for an initial announcement?

This week. I start a new job next week and want to get this out before then. The key is not that it's a "big bang" release, but rather that I am able to share the workflow that's working decently well for me and get feedback from others.

---

4. Which OS targets must the initial announcement support (Linux only, or macOS too)?

Linux (Fedora and Debian, active versions and most recent LTS), MacOS, and WSL in Windows (but not Windows itself for v1). I'd be interested to think through the implications of a WSL-only approach for domain-name management and SSL certs. It would be a good targeted investigation for a subagent.

5. What’s the minimum “works for me” workflow you want others to replicate in week one?

TL;DR "instead of running `pnpm start` or `docker-compose up`, you run `locald try pnpm start --port '$PORT'`" and it Just Works™ with a local domain and SSL cert. You get logs for free, and you don't need to worry about port conflicts or keeping the service up.

I think that the embedded postgres story is not fundamental, but it's a nice way to demonstrate the potential for future built-in features, which we'll want to tease.

6. Do you want the Dashboard to be part of the announcement story, or CLI-first?

I think the Dashboard is almost more important than the CLI for the announcement story, since it shows the value prop of managing multiple local environments more clearly. I personally use the Dashboard as the primary way to see and manage my local environments.

---

Follow-up questions from quick codebase/RFC survey

Top blockers (clarify before full review)

1. What is the exact platform promise in the announcement (Linux-only vs Linux+macOS+WSL; for WSL, what “working” means for domains/certs)?

- Linux
  - Debian: latest stable, latest LTS
  - Fedora: latest stable, latest LTS
  - Ubuntu: latest stable, latest LTS
- macOS: latest 2 major versions
- WSL: same support as Linux, but only if locald is installed in WSL (not Windows natively)

"Working" means:

- Domain names resolve correctly from the place the user is likely to have their browser installed (WSL: Windows; macOS: macOS; Linux: Linux). This is possibly a killer for WSL. We need to investigate.

2. What is the canonical “Hello World” workflow to promote (locald try vs locald up with config)?

`locald try` is the canonical "hello world" workflow, not just for the announcement but for the long term. It's the easiest way to get started and demonstrates the value prop most clearly.

3. What is the minimum scope definition for v1.0 (the core story, even if we don’t cull features yet)?

Can you review the codebase again and give me a list of features that you think are definitely in, and a list of the features that you're not sure about? I'll use that to answer this question precisely.

4. What doc/CLI drift issues are must-fix before announcement?

The docs should be trimmed down to the core story, with a section on future roadmap/tease and another on experimental features if we have time and motivation. The CLI help output should match the trimmed docs, hiding experimental features and focusing on the core workflow.

Next tier (important for expectations)

5. CNB: shipped, experimental, or hidden for the announcement?

Experimental. I think we want a section in the docs teasing the full goal of having locald fully replace docker, docker-compose, k8s and `pack` for local development, but right now we're really focused on running local development servers idiomatically (e.g. `pnpm start`, `rails server`, etc) with local domains and SSL certs.

We support Docker as a way to run existing, published servers in locald that your app depends on (to support things like redis, etc.), but not as a way to run your _app_. Support for repeatable development environments ("give me the right version of node") is a goal, and one that could be aided by CNB support, but it's not in scope for v1.0.

6. Installation strategy: is source-only acceptable, or must we ship binaries this week?

I think we should at least get cargo binstall working and ship binaries to github releases. If we have time to set up a proper install.sh script that downloads the right binary for your platform, that would be even better, but it's not critical.

7. Privileged ops/shim story: what’s the official guidance for setup/upgrade?

TL;DR locald only makes sense as a privileged service, since it gives you SSL certs and local domains running on port 443. The official guidance is: when you run `locald up`, it will prompt you for sudo access to set up a privileged shim that will manage locald services for you, and you should approve it. We should document the rationale clearly, but "my dev tool needs sudo" is an acceptable ask for this use case, and not that unusual actually. It will only seem unusual if we try to hide it or protest too much.

8. Container runtime: do we promise “Docker not required,” or avoid that claim?

We promise "docker not required" since we don't use or interact with docker. It just works.

Nice to have (can be noted as limitations)

9. Dashboard vocabulary: “pin” vs “monitor.”

I think we _really_ need to pin this down (no pun intended) and have it consistent in the docs, CLI, and Dashboard UI before announcement. It's related to "keep this app running all the time" and "disable", and I think we need to focus a little more on getting the wording right. I also think that the lack of clear vocabulary has caused previous agents to create multiple incoherent features that overlap in confusing ways, and we should make sure we're clear on what concepts are needed.

We need to think about the workflows, as the core value prop is "it stays up and you don't have to think about it," but at the same time, people aren't always working on all of their apps and the use resources. I think that a small handful of clear concepts with good affordances and documentation would help people quickly understand and get value from their new locald-managed environments and workflows.

By the way: eliminating the need for agents to have to run and manage local servers and ports is a _huge_ improvement for AI-assisted development workflows, since it means that the AI doesn't have to reason about port conflicts, whether the server is running, etc. So getting this right has implications beyond just human users.

10. Postgres data location/reset semantics for v1.

Great question. I could use advice here.

11. GC/cleanup behavior expectations.

This is very related to the pin/monitor vocabulary question. I think we need to define clear concepts for "keep this app running all the time," "stop this app when I'm not using it," and "remove all traces of this app from my system." I could use some advice on where to point and how to clean things up safely without surprising users.

12. Doctor coverage scope for v1.

I think the doctor should focus on the core workflows for v1: installation, setting up the privileged shim, running `locald try`, and basic networking (domains and SSL certs). Anything beyond that can be deferred to future versions.

We should also focus on clear, actionable error messages that help users recover from common issues without needing to run the doctor too often.

13. Release channels messaging for the announcement.

I think we should announce with release channels, but say that right now we've released 1.0-beta, and are working towards 1.0 stable. We should also announce a monthly release cadence for stable releases and an intent to use an annual release cadence for major versions.
