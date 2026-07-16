# locald Recovery and Exo Bankruptcy Dossier

**Snapshot date:** 2026-07-16

**Remote baseline:** `wycats/locald@5a2caac5c1364d3573d825186fa197143e4af0df`

**Purpose:** preserve the state that informed the productization reset before any
legacy Exo work is abandoned.

## How to read this record

This dossier separates observed state from the decisions made from that state.
It is a historical recovery artifact, not a second product specification. The
project mission and axioms remain constitutional; effective Stage 4 RFCs are
the normative product canon; and the approved productization plan controls the
next implementation sequence.

Exo bankruptcy retires stale obligations. It does not erase completed history,
promote RFCs, clean Git, apply stashes, merge pull requests, or declare current
implementation behavior canonical. Everything recorded here remains evidence
that future work may reuse, reject, or supersede deliberately.

## Executive finding

locald is not a blank slate. It already has a daemon/CLI architecture, dynamic
internal ports, trusted local certificates, privileged port binding, process
management, health machinery, a reverse proxy, stopped/loading pages,
WebSocket proxying, editor attachments, a dashboard, and substantial public
documentation. Those pieces are enough to evolve rather than restart.

The project-management and design narrative no longer describe one coherent
product, however:

- Exo is executing an old UX phase with six goals and no tasks while several
  unrelated epochs remain unfinished.
- The RFC corpus has 135 records but only 15 Stage 4 RFCs, and several Stage 3+
  documents conflict with implementation or the approved direction.
- `docs/manual`, `locald-docs`, RFC 0071, and the agent instructions disagree
  about which documentation surface is authoritative.
- Lifecycle is path-keyed attachment counting rather than an availability
  model; worktree/resource isolation is largely unimplemented.
- TLS will sign any requested SNI before checking whether locald owns it.
- The privileged helper accepts any non-root local caller and can modify CA
  trust.
- Distribution remains Linux-only and has never produced a GitHub Release,
  while the approved beta is macOS-first and signed/notarized.

The correct recovery is therefore to preserve this evidence, bankrupt the
unfinished Exo plan, and start one productization epoch with three milestone
phases plus a separate RFC/docs canonization epoch.

## Git and workspace snapshot

### Configured checkout

| Item | Observed state |
| --- | --- |
| Checkout | `~/Code/locald` (home directory anonymized) |
| Branch | `main` |
| Local HEAD | `65658089a61150e6cbfc33775af559bd3f38e658` |
| GitHub/default `main` | `5a2caac5c1364d3573d825186fa197143e4af0df` |
| Ahead/behind | `+2/-0` |
| Tracked changes | none |
| Untracked state | `.context/inbox/20260621-locald-answers-external-telemetry-host.md` |
| Worktrees before recovery worktree | one configured checkout |

The untracked context note records a concrete incident in which
`telemetry.vercel.com` resolved to `0.0.0.0` and locald served a certificate for
that external hostname. It is direct evidence for the owned-SNI requirement;
it is preserved in place and is not added by this dossier PR.

### Local-only commits

| Commit | Contents | Disposition |
| --- | --- | --- |
| `99b740562b0b13e3f50bd1c38685d1e9d107547b` — Document reliability model | Documentation, axioms, public docs, and VS Code integration changes | Preserve as design evidence; extract only behavior that agrees with the approved plan |
| `65658089a61150e6cbfc33775af559bd3f38e658` — Plan Always On semantics | A detailed Always On implementation plan | Preserve the workflow questions and invariants; replace attachment-based answers with the availability model |

Together these commits differ from remote `main` across 20 files
(`+550/-121`). They are not an implementation base and must not be merged or
cherry-picked wholesale.

### Stashes

| Stash | Base and contents | Reusable evidence | Rejected authority |
| --- | --- | --- | --- |
| `cbe068226f3bed765ee44f5db32ee8716dd313a7` (`stash@{0}`) | Based on PR #103 head; eight files, `+992/-748`, concentrated in doctor/setup reporting, boot progress, CLI polish, and tests | Setup convergence, diagnostics, progress, and test cases | None of the stash is presumed mergeable without re-evaluation against current `main` |
| `c90688eb894d786d5db2cbe482f2b73d7381cb23` (`stash@{1}`) | Based on local `6565808`; five files, `+837/-71`, including Pin reconciliation, manual-stop behavior, and manager tests | Lifecycle edge cases and test inventory | Pin as a fabricated attachment and attachment-store authority |

Both stashes remain untouched.

## Open pull requests

At snapshot time, all three pull requests were open, ready for review, and
mergeable, but none was a suitable productization baseline.

### [PR #103 — Implement quiet locald up with runtime holds](https://github.com/wycats/locald/pull/103)

- Head: `fc09a4187c09a8a7d595d2f19d205fdab76ea2bd`
- Scope: 12 files, `+756/-112`
- Checks: all recorded checks successful at snapshot time
- Review debt: three unresolved Copilot threads, including a current finding
  that root setup writes/chowns user-controlled LaunchAgent/log paths without
  sufficient symlink containment.

Preserve:

- `locald up` should converge, summarize, and return.
- `locald up --follow` should make log following explicit.
- macOS setup should aggregate required-component failures and verify the
  LaunchAgent is genuinely loaded.
- The invoking GUI user, rather than root's home, is the setup target.
- Focused CLI/setup tests.

Reject:

- An indefinite `AttachmentSource::Runtime` as desired-state or lifecycle
  policy.

### [PR #104 — Add locald docs style guide pages](https://github.com/wycats/locald/pull/104)

- Head: `75c3e4a38943b1652d59b6e28ca337ab6e3fdf08`
- Scope: 46 files, `+7881/-1`
- Checks: all recorded checks successful at snapshot time
- Review debt: 11 unresolved current threads covering component prop
  mismatches, bare-domain links, inert-button accessibility, token drift, and a
  deprecated dependency.

Preserve:

- Design tokens, reusable documentation components, poster-driven pages, and
  the private-poster handoff as candidate material for the public-docs lane.

Do not infer:

- The pages are canon, accepted documentation, or release evidence in their
  current state.

Disposition: park for the RFC Canon and Public Docs lane.

### [PR #105 — Linux desktop tray status surface](https://github.com/wycats/locald/pull/105)

- Head: `b4f5359b2da23f7fa178a719c75d35ded0cfb3d3`
- Scope: 80 files, `+6679/-1856`; contains PR #103 plus a Linux tray/UX stack
- Checks: macOS Build failed while all other recorded checks succeeded at
  snapshot time
- Review debt: unresolved findings include a missing macOS import, inherited
  PATH loss during watched config reload, autostart bypassing PID tracking,
  reused-PID risk, and creation of a runtime hold only after successful startup.

Preserve:

- The need to carry trusted PATH through daemon-managed launches and reloads.
- Status, doctor, IPC reconciliation, and test evidence that can be restated
  under the approved model.

Reject:

- Wholesale adoption of the broad branch.
- Permanent Runtime holds.
- Linux tray work as an inherited beta obligation.

No pull request will be closed or commented on without user-approved wording.

## Exo snapshot

### Active projection

- Epoch: **The Build Era** (`E4`)
- Phase: **UX Polish: User-Visible Improvements**
- Mode: executing
- Goals: zero completed, six pending
- Tasks: zero
- Git projection: one untracked item

### Epoch inventory

Completed history remains valuable and must survive bankruptcy:

- MVP (The Walking Skeleton)
- Refinement & Robustness
- Hybrid Development & Advanced Features (Completed)
- The Perfect Demo (Completed)
- User-Facing Surface Coherence

The unfinished epochs targeted by the approved reset are:

| Recorded primary ID at snapshot time | Epoch | Derived state | Unfinished phases |
| --- | --- | --- | --- |
| `01ktawhredn9zcgw9evswwcpny` | Product Surface Realization | pending | 4 |
| `01ktawhw51f29zj745jbbc63vc` | VMM Runtime Maturity | pending | 1 |
| `E3` | Hybrid Development & Advanced Features | unfinished epoch | 2 |
| `E5` | The Perfect Demo | unfinished epoch | 1 |
| `E4` | The Build Era | in progress | 3 |

Exo also contains completed rows named `E3-completed` and `E5-completed`.
Read-side `epoch status E3/E5` currently resolves those completed aliases, while
the underlying project state distinguishes the unfinished primary IDs recorded
above. These literals are historical evidence, not authorization to use them as
writer inputs without live verification. The bankruptcy contract below
requires resolving every writer target against current authoritative Exo state,
preserving the completed rows, and honoring Exo's confirmation prompt.

### Every unfinished phase and goal

#### Product Surface Realization

These pending phases have no goals:

- Style Guide Source Lock
- Public Site Direction
- Dashboard Product Target
- Interface Pattern Adoption

#### VMM Runtime Maturity

**Phase 70: VMM Maturity & Networking**

- `102.1` — introduce a reactor/event-loop model.
- `102.2` — move virtio-block I/O off the VCPU thread.
- `102.3` — add a minimal TAP-backed virtio-net device.
- `102.4` — evaluate adopting `dbs-virtio-devices` after the reactor exists.

#### The Build Era

**UX Polish: User-Visible Improvements** (in progress)

- `ux.1` — add service type and connection URL to status.
- `ux.2` — inject `DATABASE_URL` for Postgres dependencies.
- `ux.3` — show accurate service types/connections in the dashboard.
- `ux.4` — standardize CLI errors on `CliError`.
- `ux.5` — synchronize `docs/manual/cli.md`.
- `ux.6` — add `locald add postgres`.

**Phase 33: CNB Library Extraction**

- `65.1` — scaffold `cnb-client`.
- `65.2` — migrate OCI layout/runtime-spec logic.
- `65.3` — update `locald-builder` to use the extracted library.

**Phase 34: Rust CNB Launcher (Research)**

- `66.1` — prototype the launcher and config parsing.
- `66.2` — implement environment merging.
- `66.3` — implement the `execve` strategy.

#### Hybrid Development & Advanced Features (`E3`)

**Phase 59: Interactive Terminal & Theming**

- `28.1` — bidirectional browser PTY.
- `28.2` — light/dark themes.
- `28.3` — command palette.

**Phase 61: Engineering Excellence**

- `31.3` — test documentation samples.

#### The Perfect Demo (`E5`)

**macOS Setup Reliability and Quiet Up**

- `macos-setup-reliability-and-quiet-locald-up` — the broad setup/quiet-up
  outcome represented by PRs #103/#105.

These items are retained here as historical context. Bankruptcy explicitly
rejects their status as active obligations; useful pieces re-enter only through
the new productization phases.

## RFC and documentation snapshot

### RFC corpus

Exo reports 135 RFCs:

| Status | Count |
| --- | ---: |
| Stage 0 | 27 |
| Stage 1 | 12 |
| Stage 2 | 9 |
| Stage 3 | 61 |
| Stage 4 | 15 |
| Superseded | 2 |
| Withdrawn | 9 |

The Stage 4 set is RFCs 0001–0012, 0023, 0087, and 0148. No RFC stage changes
as part of bankruptcy.

### Documentation surfaces

- `docs/manual`: 56 files.
- `locald-docs/src/content/docs`: 77 pages.
- 24 public pages are generated from `docs/design` during the docs build: six
  concept pages, the axioms index, and 17 axiom pages.
- The docs build therefore has derivative public copies as well as separately
  authored public pages.

The authority model is contradictory:

- `AGENTS.md` treats Stage 3+ RFCs as implemented law.
- Stage 3 RFC 0071 says manual/design content will migrate and the public docs
  site will become authoritative.
- `docs/manual/vision.md`, `locald-docs/README.md`, and the public RFC index
  still name `docs/manual` as operational truth.
- The public RFC section contains eight design-note pages, not the 135-RFC
  corpus.

Concrete canon divergences include:

- Stage 0 RFC 0147 is partially implemented but encodes path identity,
  branch-derived domains, ref-counted attachments, permanent Pin, PID-bound CLI
  lifetime, port-bearing status/tools, and destructive Remove behavior. The
  approved plan replaces all of those contracts.
- Stage 3 RFC 0095 promises TTL/root-set mark-sweep GC and audit behavior;
  current cleanup deletes missing unpinned projects and generated state.
- Stage 3 RFC 0100 specifies unified dashboard Pin vocabulary; the dashboard
  currently uses monitor/monitored and Add to Deck.
- Stage 4 RFC 0010 describes the Linux setuid-shim/SCM_RIGHTS architecture but
  does not describe the current macOS LaunchDaemon/XPC helper.

The public site also has stale `ykatz/dotlocal` and `wycats/dotlocal` links, a
development-only `http://docs.localhost` canonical URL, no production
deployment URL or Vercel configuration recorded in the repository, and
Linux/WSL claims inconsistent with the approved macOS-only beta.

The existing docs verifier runs in CI but misses staged RFC directories and
does not enforce RFC metadata, external links, manual retirement, or stale
manual references.

## Implementation reality at the reset boundary

### Lifecycle and readiness

Observed:

- `attachments.json` is the authority, keyed by canonical path.
- Sources are Editor PID/window metadata, CLI PID, and Pin, with a separate
  path-keyed `manually_stopped` set.
- Persistence is unversioned direct JSON writing without an atomic journal.
- First attachment starts; last attachment stops immediately.
- Attach clears manual stop, and failed startup removes the attachment/demand.
- `locald up` attaches, starts through a second IPC path, and follows logs until
  interrupted unless a hidden test flag is used.
- Runtime snapshots restart previously Running services without consulting
  current demand or manual-stop policy.
- Registry pin changes cleanup metadata but does not keep a project running.
- A 30-second reaper handles dead PID/editor attachments; there are no leases,
  generations, cooldown, or conversation demand.
- TCP probe machinery exists, but exec readiness can report Healthy as soon as
  the child is alive.
- There is no per-project convergence lock or persisted trusted launch PATH.
- Status is service-centric and normally exposes ports/PIDs rather than
  availability reasons.
- Corrupt registry/attachment stores can be treated as empty instead of
  blocking startup.

Decision:

- Replace attachment counting with persisted availability generations,
  renewable demand, Always On policy, generation-scoped pause, real readiness,
  transition serialization, trusted launch context, and explanatory status.

### Domains and TLS

Observed:

- The certificate resolver signs and caches a certificate for any received SNI.
- Routing checks Host later and returns 404 for unknown domains, after TLS has
  already succeeded.
- Routing is rebuilt by scanning string-keyed services; no persistent atomic
  domain-ownership index exists.
- Duplicate domains are tolerated rather than treated as conflicts.
- Configuration accepts arbitrary domain strings, including non-`.localhost`
  examples.
- Reload applies registry/hosts/service changes incrementally and can leave a
  partial new state after failure.
- Stopped/loading pages and WebSocket proxying already exist and are useful
  precursors.
- Worktree-domain helpers exist but live routing/status do not consistently
  pass worktree context.

Decision:

- Introduce one owned `DomainIndex` shared by HTTP, WebSockets, TLS, status, and
  stopped/loading pages; validate and replace claim sets atomically; reject
  unknown SNI and conflicting ownership.

### Privileged setup and helper

Observed:

- The helper rejects UID 0 but accepts any other local caller.
- It does not verify the console user, code identity, or protocol version.
- It exposes both `bind` and `setup`; `setup` trusts caller-selected per-user CA
  material into the System keychain.
- Bind is correctly restricted to ports 80 and 443.
- Standard server startup generally fails closed when certificate creation or
  port binding fails, and sandbox mode is explicit.
- Standard-mode port environment overrides remain an unguarded bypass.
- macOS setup performs trust, LaunchAgent install, and helper install, but lacks
  a final protocol/version/port/trust convergence check.
- Current teardown removes the LaunchAgent and helper but not CA trust/key
  material and does not distinguish generated state from mutable resources.

Decision:

- Make sudo setup the sole trust-mutation path, reduce the long-running helper
  to authenticated privileged binding, verify the whole installed system, and
  preserve the no-fallback product axiom.

### Worktree identity and resources

Observed:

- No repository, worktree, logical-project, or project-instance IDs exist.
- Registry and attachment authority are path-keyed.
- Worktree support derives branch-templated domains; it has no persistent slug.
- Services, health, logs, and sticky ports use `project-name:service` string
  keys, so same-named worktrees collide.
- Postgres storage is derived from the same global service string.
- Generated build/container state is path-hash based.
- Cleanup can recursively remove missing-project generated state; there is no
  Missing/forget/purge-resources distinction.

Decision:

- Introduce Git-admin identities, typed instance-scoped service/resource keys,
  persistent domain slugs, safe migration, and explicit resource deletion.

### Installation and release

Observed:

- `install.sh` explicitly rejects macOS and installs Linux tarballs only.
- `selfupgrade` selects Linux artifact names from GitHub `releases/latest` and
  has no beta channel, macOS embedded-component refresh, safe restart, or
  post-replacement rollback.
- The release workflow builds unsigned Linux x86_64/ARM64 archives only.
- No GitHub Releases exist.
- Tags `v0.1.0` and `v0.1.0-test.1` both produced failed Linux ARM64 release
  runs; x86_64 was cancelled and release creation was skipped.
- macOS CI builds/tests, and the build embeds an ad-hoc-signed helper, but there
  is no Developer ID signing, notarization, or provenance.
- There are no root MIT/Apache license files, workspace license metadata,
  `SECURITY.md`, issue templates, or beta support/release policy.

Decision:

- Treat current release automation as historical evidence. The beta release
  path is signed/notarized macOS first, dual MIT/Apache-2.0 licensed, with
  verified install, upgrade, uninstall, security, and public docs.

## Useful precursors to preserve

The reset should build on, rather than discard:

- Daemon + CLI architecture and Unix-domain-socket IPC.
- Dynamic internal port allocation.
- Standard-mode bind failure behavior and explicit sandbox mode.
- Reverse proxy and conventional service domains.
- TCP probe machinery.
- Disabled/stopped/loading pages.
- WebSocket proxying.
- PID/window attachment plumbing as migration evidence.
- Runtime-state atomic temp/rename behavior where already used.
- VS Code session IDs and attach/detach plumbing.
- macOS agent/helper embedding and macOS build CI.
- Doctor, status, progress, and test ideas in PRs/stashes.

These are mechanisms, not proof that their current contracts are canonical.

## Productization direction carried forward

### Milestone A — Minimum Awesome Daily Driver

- Stable project identity foundation and owned exact domains.
- Secure privileged setup.
- Availability generations, renewable demands, Always On, pause, readiness,
  trusted PATH, and explanatory status.
- Prove the workflow on `word-explorer`, `color`, `stfc-static-data`, and
  `axiomatic-color`.

### Milestone B — Chat = Worktree for v0

- Instance-scope every runtime resource.
- Allocate persistent worktree domains and explicit wildcard service claims.
- Resolve Codex/VS Code context ambiently through daemon APIs and a thin MCP
  adapter.
- Prove several natural v0 worktrees concurrently without visible ports.
- Keep local-sandbox exploration bounded to one demonstrated ownership seam.

### Milestone C — Vercelian Pilot and Public Beta

Here, **Vercelian** means a Vercel employee; the approved milestone name is
retained while making the intended pilot cohort explicit.

- Dual MIT/Apache-2.0 license.
- Signed/notarized Apple Silicon and Intel macOS beta artifacts.
- Verified installer, beta upgrade, uninstall, checksums, and provenance.
- Public docs and security/support policy.
- Five-person Vercelian pilot followed by the same qualified public beta.

### Separate RFC Canon and Public Docs lane

- Audit all Stage 4 RFCs and all manual files.
- Make Stage 4 the normative maintainer canon.
- Make `locald-docs` the user-facing manual.
- Reconcile implementation divergences through repair or explicit
  erratum/supersession.
- Delete `docs/manual` only after every file has a terminal disposition and CI
  proves the public/canon surfaces are complete.

## Bankruptcy contract

After this dossier is merged, resolve every target against live authoritative
Exo state, then bankrupt in this order. Every literal ID below is a snapshot
value that must still be reverified before use:

1. `01ktawhredn9zcgw9evswwcpny`
2. `01ktawhw51f29zj745jbbc63vc`
3. **Resolve live:** primary writer ID for the unfinished
   `Hybrid Development & Advanced Features` row
4. **Resolve live:** primary writer ID for the unfinished `The Perfect Demo`
   row
5. `E4`

The recovery snapshot recorded `E3` and `E5` as the two unfinished rows'
primary writer IDs, but those values are intentionally not presented as
copyable run-list entries. The inspected write path matches the command input
directly against the epoch primary ID and then against
`epochs_data.text_id`; that path is separate from the read-side alias lookup
that currently misresolves `epoch status E3/E5`. Immediately before execution,
reverify that writer contract in the installed Exo build and bind each
placeholder to the corresponding live unfinished row. If an authoritative Exo
surface cannot demonstrate that mapping, stop rather than using the snapshot
value by assumption.

For each command:

- Use the live unfinished row's verified primary writer ID, not a read-side
  alias or an unverified value copied from this dossier.
- Review and approve the Exo confirmation prompt explicitly.
- For the two placeholders, compare the prompt's epoch ID, title, and current
  status against the unfinished rows from `epoch list`; it must target the
  unfinished `Hybrid Development & Advanced Features` or `The Perfect Demo`
  row, never the completed `E3-completed` or `E5-completed` alias.
- Do not bypass Exo or edit SQLite/projections.
- Stop if Exo resolves either writer target ambiguously or appears to target a
  completed alias; do not guess or substitute a different ID.

After bankruptcy, verify:

- All five targeted epochs derive as bankrupt/abandoned.
- All unfinished phases/goals above are abandoned.
- Completed phases/goals and the five completed epochs remain completed.
- RFC counts/stages are unchanged.
- Git, local commits, stashes, and `.context/` are unchanged.
- No stale phase/goal remains pending or in progress.

Then create:

- **Locald Productization** with phases A, B, and C.
- **RFC Canon and Public Docs** with the four canonization phases.

Populate and start only Milestone A. B, C, and canonization remain placeholders
until their entry reviews refresh live state. No phase/goal completion is
recorded without user outcome approval.

## Recovery boundary

This dossier completes preservation, not implementation. The next irreversible
action is Exo bankruptcy, and it remains gated on:

1. this docs-only PR merging;
2. verifying that the installed Exo binary contains the current sidecar-safety
   behavior;
3. running Exo project resolution and sidecar-status checks;
4. identifying the canonical writer for the machine/repository state;
5. verifying that the remote sidecar contains the current `projects/locald`
   state before any link or import;
6. stopping if writer ownership, remote state, or any writer-target mapping is
   ambiguous; and
7. explicit confirmation of each bankruptcy command.
