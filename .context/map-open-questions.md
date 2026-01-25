# MAP Open Questions

These questions need definitive answers before MAP is complete. Please fill in your answers below each question.

---

## 1. Postgres data location/reset semantics

**Question:** What's the default data location for managed Postgres? How do users reset/clear the database?

**Empirical findings:**

- **Data location:** `~/.local/share/locald/postgres/<project>/` (XDG path)
- **Binaries:** Downloaded to `~/.local/share/locald/postgres-dist/`
- **Reset command:** `locald service reset <name>` exists
- **⚠️ CRITICAL BUG:** `service reset` deletes `.locald/postgres/` (workspace-local) but data actually lives in XDG directory. **Reset doesn't work as documented!** (Conflict C-012)
- **No cleanup on project removal:** Postgres data persists in XDG indefinitely

**Your answer (how should this work?):**

```
We should fix the `locald service reset <name>` command to correctly locate and delete the Postgres data directory in the XDG path. Additionally, we should document the data location clearly in the docs.
```

---

## 2. GC/cleanup behavior

**Question:** When does locald remove state? How does a user trigger cleanup? What gets cleaned up?

**Empirical findings:**

- **Available:** `locald registry clean` removes non-existent unpinned projects from registry
- **Available:** `locald service reset <name>` stops + wipes data for one service
- **Missing:** No `locald gc` command for comprehensive cleanup
- **Missing:** No `locald remove` or `locald unregister` for explicit project removal
- **State locations:**
  - Registry: `~/.local/share/locald/registry.json`
  - Server state: `~/.local/share/locald/state.json`
  - Certs: `~/.locald/certs/`
  - Service data: `~/.local/share/locald/` (postgres, bundles, OCI)
  - Logs: In-memory only (2000 entries, not persisted)
- **No cascade cleanup:** Deleting a project folder leaves registry entries, data dirs, certs behind

**Your answer (what should the cleanup story be for MAP?):**

```
I like the comprehensive ideas, but I think it's important to remember that there are basically two use-cases here:

1. The common case ("apps are on by default and I disable the ones I don't want to be enabled on boot")
2. The manual management case ("apps are off by default and I enable the ones I want to be enabled on boot").

If I recall correctly, we did some significant thinking on the right terminology and modelling for these modes. Can you see if you can find it?
```

---

## 3. WSL domain resolution

**Question:** Does `*.localhost` resolve correctly from a Windows browser into a service running in WSL?

**Empirical findings:**

- **No, it doesn't work** from Windows browsers into WSL without a helper
- WSL and Windows have separate hosts files
- **RFC 0131 + 0133** propose a Windows helper (`locald-helper.exe`) but neither is implemented
- **Current scope:** "Self-contained WSL" only (browser inside WSLg works)
- **Documentation explicitly states:** "Windows browsers are not yet supported end-to-end"
- This is positioned as a **known limitation, not a blocker** for launch

**Status:** ✅ Resolved - Self-contained WSL is the documented scope for MAP. Windows browser support is post-MAP.

---

## 4. Pin vs Monitor vs Disable vocabulary

**Question:** What are the canonical definitions for these concepts?

**Empirical findings:**

- **`locald registry pin/unpin`** exists but is **non-functional** for its stated purpose
- CLI help says "keep it running" but code only sets a boolean flag for GC retention
- **No autostart on daemon boot** is implemented
- **No enable/disable commands** exist
- **Three conflicting meanings of "pin":**
  1. CLI: "keep it running" (implies autostart)
  2. Code: retention only (prevents GC)
  3. Dashboard: focus/display mode (which services to show in Deck view)
- **RFC 0135 proposes:** Pin = runtime policy, Monitor = dashboard focus, Favorite = UI persistence
- **RFC 0112 documents conflicts:** C-007 (pin semantics mismatch), C-008 (two "pin" concepts)

**Your answer (what should the vocabulary be?):**

| Term          | Your Definition |
| ------------- | --------------- |
| **pinned**    |                 |
| **monitored** |                 |
| **disabled**  |                 |
| **enabled**   |                 |

```
#0135 is newer so it's probably canonical. But this is similar to the cleanup question above — we had some deep thinking about the right terminology and mental models here. Can you see if you can find it?
```

---

## 5. `locald try` behavior after session ends

**Question:** When a user runs `locald try pnpm start --port '$PORT'` and then closes their terminal, what happens?

**Empirical findings:**

- **Ephemeral:** `locald try` runs as a foreground CLI child process (not daemon-managed)
- **Dies with terminal:** When terminal closes, process receives SIGHUP and terminates
- **Design intent:** This is intentional - `try` is "Draft Mode" for experimentation
- **Prompts to save:** After exit, offers to save to `locald.toml` for persistence
- **Difference from `locald up`:** `up` sends to daemon (detached, persistent), `try` runs directly

**Status:** ✅ Resolved - Current ephemeral behavior is intentional ("Draft Mode"). This is the right design.

---

## 6. Dashboard as primary vs CLI as primary

**Question:** You said the Dashboard is "almost more important than the CLI" for announcement. Does this mean:

- A) Dashboard is the recommended primary interface, CLI is for automation/power users
- B) Both are equal, but Dashboard better demonstrates the value prop visually
- C) Something else

**Your answer:**

```
I would say that there are two personas, but that (A) is the more common and important one (and the one I personally identify with more). This also means that the GC / cleanup story _must_ have good Dashboard integration, since that's where most users will be managing their services. It also means that the dashboard should probably have a way to reflect the two personas, once we remind ourselves about the details of that thinking.
```

---

## 7. Docker for dependencies

**Question:** You mentioned "Docker for dependencies only, not apps." What's the exact boundary?

**Empirical findings:**

- **Container service type exists and works:**
  ```toml
  [services.redis]
  type = "container"
  image = "redis:7"
  container_port = 6379
  ```
- **Native OCI execution:** Uses libcontainer via locald-shim, **no Docker daemon required**
- **Docker daemon support was removed** (RFC 0142, implemented)
- **Status:** Stable in implementation, but **quarantined from MAP announcement** as "Experimental"
- Users **can** run `redis:latest` and other Docker images as dependencies today

**Your answer (should container services be IN or OUT for MAP announcement?):**

```
Container services are in, but docker is out. We have our own OCI runtime now, and that's what we should be using for containers. But the ability to run containerized dependencies is definitely part of the MAP value proposition.
```

---

## 8. Installation path for announcement

**Question:** What's the minimum viable installation story?

**Empirical findings:**

- **`install.sh`** - Fully configured, downloads from GitHub Releases, supports x86_64/aarch64 Linux
- **`cargo binstall`** - Configured in Cargo.toml with correct mappings
- **GitHub Release workflow** - Automated, builds Linux binaries + checksums
- **⚠️ No releases published yet** - Scripts ready but no v0.1.0+ tag pushed
- **Missing:** macOS binaries (Linux only), `locald selfupgrade`, auto-update
- **Docs outdated:** README still says "built from source"

**Your answer (what's the plan for announcement?):**

- [ ] Push v0.1.0 tag to trigger first release?
- [ ] Update README with install.sh instructions?
- [ ] macOS binaries needed for launch?

```
Basically the first two items, and we _must_ figure out macOS binaries before launch, so that's in scope too.
```

---

## 9. Privileged setup messaging

**Question:** How do we explain "locald needs sudo" without scaring users?

**Your take from notes:**

> "my dev tool needs sudo" is an acceptable ask for this use case, and not that unusual actually. It will only seem unusual if we try to hide it or protest too much.

**What's the one-liner explanation for docs/CLI output?**

**Your answer:**

```
I think this is actually something that an LLM can figure out (and you can give me a recommendation). TL;DR since the value prop is "we make HTTPS and localhost easy", I think the explanation should be something like:
"locald needs elevated privileges to set up trusted HTTPS certificates and bind to standard ports (like 443) on your machine, ensuring a secure and seamless local development experience."

But again, tons of dev tools software asks for password elevation for similar reasons, and most users aren't even going to ask about it. We just need to make sure we're super clear for users who do care.
```

---

## Done?

Once you've answered these, I can:

1. Update RFC 0116 with the definitive answers
2. Create issues/tasks for any implementation work needed
3. Update the vocabulary in RFC 0135 (Dashboard Vocabulary)
4. Fix any critical bugs discovered (like the Postgres reset issue)

Just let me know!
