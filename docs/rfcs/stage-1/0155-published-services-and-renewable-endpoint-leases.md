<!-- exo:155 ulid:01kyx1veakdkwffa8j9xgcb6y4 -->

# RFC 155: Published Services and Renewable Endpoint Leases

## 1. Summary

This RFC proposes a declared `published` service whose stable identity and HTTPS origin are owned by locald while its loopback runtime is owned by another local process running as the same user.

A project opts in by declaring the service in `locald.toml`:

```toml
[services.workbench]
type = "published"
```

The declaration creates the instance-scoped `ServiceKey` and normal domain claims before any publisher exists. A kernel-identified same-user publisher may then acquire a short, renewable lease by transferring an ownership-preserving capability for one private loopback listener. locald owns TLS, routing, endpoint health, unavailable surfaces, address-takeover prevention for the active binding, and atomic upstream replacement. The publisher retains process lifecycle and application authorization.

The user experiences this as a stable service, not as a lease or port. The first motivating consumer is Exo's lane workbench: one declared project instance in one physical worktree has one stable workbench origin. The focused lane is mutable Exo state inside that worktree and never enters the hostname.

This proposal defines the public direction. Implementation and later RFC-stage transitions remain separately reviewed and approval-gated.

## 2. User Experience

### 2.1 Everyday Exo workflow

Once a repository declares the published workbench service, the normal flow is:

1. The user opens or invokes the Exo workbench for a physical worktree.
2. Exo validates that exact workspace, resolves its focused lane and locald project instance, and begins publication to receive the stable semantic origin.
3. Exo installs that origin in the workbench's workspace authorization, then starts or reuses its loopback host and acquires the declared service with the bound listener.
4. locald verifies the declaration, observes endpoint health, and makes the stable trusted origin ready.
5. Exo mints a fresh workspace-and-origin-bound ticket and returns the stable-origin launch URL.

For a worktree with slug `proposal-session` and project base `exo.localhost`, the user sees:

```text
https://workbench.proposal-session.on.exo.localhost
```

The user never selects, remembers, or passes the private loopback port. Changing the focused lane does not change the hostname. Restarting Exo may briefly show locald's loading or unavailable surface, but successful republication restores the same origin.

### 2.2 Interaction with ordinary locald lifecycle

Published services are provider-activated. locald cannot honestly start an externally owned process, so an absent publisher or unhealthy endpoint does not prevent generic `locald up` or `ensure_available` from making locald-managed services ready. The published service remains independently visible as waiting, checking, unhealthy, paused, or ready. Successful generic ensure output must report every published service whose publication state is not `ready`, including `waiting_for_publisher`, `checking_endpoint`, `endpoint_unhealthy`, and `route_paused`; it must not present a bare `Ready` that conceals any of them.

A consumer that needs the published service must use its owning workflow. For the first consumer, `exo workbench launch` begins publication to obtain the origin, configures and starts or reuses the host, acquires the service, waits for locald-observed readiness, and only then returns the launch URL.

Project-level pause remains authoritative. Pausing a project suppresses the published route without signaling the external process. The publisher may maintain its lease while paused, but passive renewal cannot clear the pause or restore routing. Explicit project Resume creates a fresh probe generation and restores the route only after that post-Resume probe succeeds.

The initial version does not pretend that locald can stop, restart, or reset an externally owned runtime. Service-level stop, restart, and reset commands return an actionable result explaining that the service is externally managed and that no route, binding, process, or external data was changed. The external owner remains the place to control that runtime and its data.

### 2.3 User and maintainer impact

Users gain stable trusted worktree-scoped origins and locald observability for tools that already own their runtimes, without learning port or lease machinery. Missing providers remain understandable rather than turning into unknown domains or silent direct-port fallbacks.

Maintainers gain one deliberately narrow external-fulfillment mode rather than a generic reverse-proxy escape hatch. The design reuses existing service identity, domain ownership, status, proxy, and agent-context concepts while keeping external process lifecycle out of locald's controller and recovery machinery.

## 3. Motivation

locald already gives a physical project instance a stable identity, a persistent worktree slug, and semantic HTTPS service origins. Today every implemented service type is fulfilled by a runtime locald prepares and controls. Some development systems already own a long-lived runtime because they must enforce application-specific invariants that locald should not absorb.

Exo's workbench is one such runtime. Exo owns:

- exact workspace and project-state validation;
- the focused lane and authoritative lane state;
- browser enrollment, one-time tickets, sessions, and grants;
- workbench commands and the loopback host lifecycle.

Moving those responsibilities into locald would duplicate product semantics and weaken Exo's authority. Leaving the workbench only on a random loopback port forfeits locald's stable worktree identity, trusted TLS, routing, health, and ambient discovery.

The required seam is therefore narrower than external process adoption: a declared locald service identity may be fulfilled by a renewable lease over an externally owned loopback endpoint.

## 4. Relationship to Current Canon and RFC 0147

This proposal complements RFC 0147, `Attachments, Worktrees, and Editor Integration`. It relies on the stable project-instance identity and persistent `<slug>.on.<project-domain>` origins implemented after RFC 0147, but it does not redefine worktree naming, attachment lifecycle, or editor integration.

RFC 0147 remains a Stage 0 historical umbrella. Its branch-template domain design predates the landed persistent-slug namespace and will be reconciled separately. This RFC neither edits nor supersedes it.

The proposal aligns with these existing rules:

- project declarations live in `locald.toml`;
- services occupy the `System > Constellation > Project > Service` hierarchy;
- `ServiceKey` is `ProjectInstanceId + ServiceName`;
- locald owns domain claims, trusted TLS, reverse routing, and privacy-safe status;
- runtime state is ephemeral while identity and configuration persist;
- agents receive semantic origins without ports, PIDs, or private owner identifiers.

### 4.1 Process Ownership

The Process Ownership axiom remains authoritative: locald owns the child processes it manages and never masquerades as a process manager for a process it did not create.

For managed service types, locald spawns, supervises, signals, restores, and supplies the runtime environment. A `published` service is the sole explicit external-fulfillment variant. It has no locald-owned child process. locald owns its declared service identity, semantic origin, route, and observed endpoint health; the external publisher owns its process.

Using the service namespace therefore does not weaken process ownership. It recognizes that the user-visible project service and the mechanism fulfilling it are separate concepts. A parallel `[endpoints]` hierarchy would duplicate service identity, domains, status, dashboard, and agent abstractions without making the process boundary more accurate.

## 5. Detailed Design

### 5.1 Terminology

- **Declared service identity**: the configured service name scoped by the physical `ProjectInstanceId`. It exists independently of a runtime.
- **Published service**: a declared service whose endpoint is externally fulfilled and whose process is not spawned, signaled, recovered, or logged by locald.
- **Publisher principal**: the kernel-identified same-user process that owns one publication session, identified by UID, PID, and platform process-birth identity.
- **Publisher lease**: ephemeral authority for one publisher principal to fulfill one exact published `ServiceKey`.
- **Acquisition attempt**: one short-lived, opaque server-issued capability for a publisher principal to acquire one exact published `ServiceKey`. It reserves one per-service publication generation and carries the acquisition snapshot, semantic origin, daemon epoch, and attempt deadline.
- **Acquisition snapshot**: the exact project configuration generation plus the resolved published declaration's service type, primary semantic origin, and effective health-policy revision under which an acquisition attempt was prepared.
- **Declaration authority**: the configuration generation, `type = "published"`, exact primary semantic origin, and effective health-policy revision admitted for a live lease. The lease, its traffic scopes, and every route authorization carry this authority until a serialized configuration transition either transfers or retires them.
- **Listener capability**: a kernel-backed reference to an already-bound private loopback listener, transferred by the authenticated publisher and retained by locald so the address cannot be reassigned while that binding remains active.
- **Binding revision**: the compare-and-swap generation of the current loopback upstream within a publisher lease.
- **Traffic scope**: one monotonically revised, cancelable generation of proxy requests, health probes, streams, upgrade bridges, and upstream connection pools under a binding. Each scope is either `probe_only` or `routable`. Suspending a route retires its current scope without retiring the binding or its listener capability; recovery creates a fresh scope revision.
- **Origin acknowledgement**: the publisher's assertion, bound to an exact server-issued attempt and listener capability, that it installed the daemon-derived primary semantic origin in the candidate application's authorization state before asking locald to probe or route that candidate.
- **Route authorization**: immutable eligibility for one exact declaration authority, origin acknowledgement, `routable` traffic-scope revision, binding revision, and health-policy revision. It is created only by an atomic promotion from a successfully committed post-acknowledgement health result or by a serialized declaration-authority transfer that proves the routing-relevant declaration unchanged. It cannot be inherited by another scope, origin, or policy.
- **Semantic origin**: the stable locald-owned HTTPS origin derived from the project instance, service name, configured domains, and persistent worktree slug.

The term attachment is not used. Attachments and project availability demands describe user or tool interest in a project; publication describes runtime fulfillment of a service identity.

### 5.2 Identity and naming

The service identity is always:

```text
ServiceKey {
  instance: ProjectInstanceId,
  name: ServiceName,
}
```

The publisher never supplies a project-instance ID, hostname, slug, branch, lane ID, or task title as authority. It supplies a project locator and service name; the daemon resolves the exact current project instance and verifies the declaration.

Normal locald domain rules apply. A lane change, branch rename, rebase, detached HEAD, or worktree move does not alter the origin. Removing and recreating the worktree creates a new project instance and therefore a new identity.

One external runtime endpoint may legitimately fulfill separate service identities for several worktrees. Each identity has its own lease, origin, health state, and application authorization.

### 5.3 Configuration

The initial schema is intentionally narrow:

```toml
[services.workbench]
type = "published"

# Optional existing relative-domain syntax:
domains = ["workbench"]

[services.workbench.health_check]
type = "http"
path = "/"
```

Omitting `domains` uses the existing conventional exact service domain. An explicit domain list must contain at least one exact claim; `domains = []` and wildcard-only lists are invalid for a published service because they cannot supply its canonical semantic origin. The first exact claim remains the primary origin under existing domain rules. That primary is the only publisher-authorized application origin and the only usable origin returned by acquisition preparation, launch, status, and agent context. Every other exact or wildcard claim is a redirect-only ingress alias: locald redirects a concrete request to the primary semantic origin, preserving path and query, before selecting any published upstream. The redirected canonical request is a separate request subject to the application's normal origin authorization; aliases are not transparent API or WebSocket origins. Supporting several independently routable application origins would require an explicit multi-origin authorization contract outside this initial proposal.

Omitting `health_check` uses an HTTP request to `/`. The initial published-service schema accepts only HTTP health checks. TCP health checks are insufficient because locald's retained listener capability can continue completing handshakes after the application stops accepting requests; command health checks are also rejected because locald does not own the publisher's execution context.

A published service has no configured port. The initial schema rejects process-owned fields such as `command`, `workdir`, `build`, `env`, `listeners`, `generated`, `depends_on`, and `stop_signal`.

The initial whole-project validator rejects every dependency edge involving a published service in either direction: a published service cannot depend on another service, and a managed service cannot depend on a published service. This preserves provider-activated readiness instead of making generic convergence wait for a runtime locald cannot start. Stable `${services.workbench.origin}` interpolation is valid; private `port`, `host`, and raw `url` interpolation is not.

A Procfile cannot declare a published service.

### 5.4 Ownership boundary

locald owns:

- `ServiceKey`, worktree slug, semantic origin, and domain claims;
- trusted TLS and host routing;
- same-user publication leases and their generations;
- endpoint health and routing eligibility;
- atomic upstream rebinding;
- stable unavailable, loading, degraded, and paused-route surfaces;
- privacy-safe human, dashboard, and agent status.

The external publisher owns:

- process creation, shutdown, crash recovery, and logs;
- creation of the private loopback listener and transfer of an ownership-preserving listener capability to locald;
- application-specific workspace and authorization rules;
- any mutable state served through the endpoint.

locald never adopts, signals, kills, or restores the external process. Lease expiry, release, configuration removal, project pause, daemon shutdown, explicit project forgetting, and automatic missing-instance pruning affect locald reachability only.

### 5.5 Publication protocol

The daemon exposes a narrow host-only IPC protocol whose conceptual operations are:

```text
BeginPublishedEndpointAcquisition(observed_daemon_epoch, expected_project_instance_id, project_locator, service_name, replace_terminal_attempt_handle?)
AcquirePublishedEndpoint(acquisition_attempt_handle, acknowledged_origin, listener_capability)
RenewPublishedEndpoint(lease_handle)
BeginPublishedEndpointRebind(lease_handle, expected_binding_revision, replace_terminal_attempt_handle?)
RebindPublishedEndpoint(rebind_attempt_handle, acknowledged_origin, listener_capability)
WaitPublishedEndpoint(lease_handle, expected_binding_revision)
ReleasePublishedEndpoint(lease_handle)
```

The exact request names and encoding remain Stage 2 details. The public contract is:

1. Every operation obtains the Unix-socket peer identity from the kernel. The authenticated IPC handshake also returns the daemon's random current epoch; beginning acquisition echoes that observed epoch, and a daemon rejects a request from any other epoch before resolving or mutating project state.
2. The peer UID must equal the daemon UID.
3. The daemon obtains the peer PID from kernel credentials and captures a high-resolution process-birth identity.
4. Beginning acquisition is the preparation phase. The publisher first obtains the exact `ProjectInstanceId` through locald's ambient project-resolution surface and carries it as `expected_project_instance_id` across every retry; the filesystem locator remains only a routing hint. The first registry transition resolves that locator server-side, requires the result to equal the expected instance, requires `type = "published"` for the named service, captures the acquisition snapshot, and reserves the service's single attempt slot before host convergence or other preparation work begins. A delayed request for a forgotten or pruned instance therefore fails instead of retargeting a replacement instance at the same path. Concurrent same-principal begins join that operation; a competing principal receives `acquisition_in_progress`. The owning operation completes the standard-mode domain bootstrap before returning the server-issued acquisition-attempt handle, exact primary semantic origin, and remaining relative lifetime. Registration may publish the persistent declaration and claims first, but preparation and ordinary configuration application share one host-convergence serialization boundary. Within it, preparation revalidates the configuration generation immediately before atomically synchronizing the complete current host set through the normal privileged setup contract, then records that exact generation as host-synchronized. An older generation therefore either writes before a newer configuration convergence—which then wins—or observes the newer generation and performs no stale write. A synchronization failure preserves or restores the last complete mapping, records one terminal failure in the bounded attempt slot, installs no lease, leaves the declaration in `waiting_for_publisher`, and returns actionable `sudo locald admin setup` or claim-conflict guidance; it is never reduced to a logged warning.
5. The endpoint input is an ownership-preserving capability for an already-bound TCP listener. On supported Unix hosts, the publisher transfers a duplicate listener file descriptor over the authenticated IPC connection. locald validates that it is a listening socket bound exclusively to `127.0.0.1` on a nonzero port, derives the private port from the socket, and retains its duplicate for the binding's lifetime. Where network namespaces can differ, locald also obtains the kernel-observed namespace identity of the transferred socket itself and requires it to equal the daemon's network namespace; checking only the publisher PID is insufficient. A mismatch, or inability to prove equivalence on such a platform, is rejected. A raw port number is never publication authority. The retained descriptor therefore protects the address that locald's own health and proxy connections will reach if the publisher closes its copy; locald never accepts application traffic from its copy. Exact ancillary-message encoding and platform socket checks remain Stage 2 details, but implementations must preserve these kernel-backed ownership and namespace properties.
6. Acquisition and rebind reject a listener whose derived port equals any current standard or sandbox HTTP or HTTPS front-door listener, preventing a published route from recursively targeting locald itself. Hostnames, arbitrary IPs, URLs, Unix sockets as upstreams, caller-selected schemes, and socket configurations that permit an unrelated listener to share the binding are rejected by construction.
7. Before acquisition, the publisher installs the returned exact semantic origin in the candidate application's authorization state. `AcquirePublishedEndpoint` echoes that origin as an explicit acknowledgement and transfers the listener capability under the matching server-issued attempt. The daemon treats the echoed value only as a publisher assertion and requires byte-for-byte equality with the daemon-derived current primary origin; the publisher never selects origin authority. No health probe, traffic scope, or route authorization exists before this acknowledged request.
8. Acquisition installs the lease only in a registry transition serialized with configuration application, project pause and Resume, project-instance retirement, and the wake barrier. It revalidates the exact server-issued attempt, publisher principal, listener identity, acquisition snapshot, pause state, acknowledged origin, active attempt deadline, and matching host-synchronized generation immediately before commit. A configuration generation change invalidates the pending attempt and requires a fresh preparation, even when the newly derived origin happens to be textually identical. If pause commits first, acquisition installs only a paused `probe_only` scope with no route authorization; if acquisition commits first, the pause transaction immediately fences and suppresses it. If Resume commits first, acquisition observes the unpaused state; if acquisition commits first while paused, Resume replaces its paused scope before probing. Only after this commit may locald begin health evaluation, and every resulting health fence carries the exact origin acknowledgement. The response returns the redacted opaque lease handle, publisher-private binding revision, and server-selected relative renewal schedule; the semantic origin remains the one returned by preparation and acknowledged by acquisition.
9. The owning workflow uses the bounded, cancelable lease-handle-and-binding-revision-scoped readiness operation before returning a user-facing launch URL. It succeeds only while that exact lease and binding remain active and possess current route authorization derived from the exact acknowledged origin, declaration authority, primary origin, and health policy. It remains pending through checking, observed unhealthiness, ordinary renewal, an alias-only or unrelated configuration transfer, and a health-policy reprobe. It terminates on timeout, cancellation, pause, lease expiry or release, binding replacement, declaration removal, type or primary-origin change, project-instance forgetting or pruning, or daemon-epoch change; it never observes a successor lease or binding as success. Normal status remains redacted and is not a substitute for this authority-scoped wait. Renewal never self-attests readiness.
10. Renewal extends only the exact current lease and returns its replacement relative renewal schedule. It does not revive an expired lease, change a binding, change origin acknowledgement, clear project pause, or restore routing while paused.
11. Rebind uses a separate server-issued attempt scoped to the exact lease and expected binding revision. Beginning rebind returns the current primary semantic origin and one bounded attempt handle. Before transferring the candidate, the publisher installs that origin in the candidate application's authorization state; the rebind request echoes it as an acknowledgement bound to the exact candidate listener identity. Only then does locald probe the candidate. A failed or stale candidate is closed and preserves the last healthy upstream. A successful candidate compare-and-swaps the binding, origin acknowledgement, and terminal attempt result in one registry transition, returns the new publisher-private binding revision, invalidates the old binding and its traffic scope, closes binding-scoped idle connection pools, and cancels binding-scoped work. The old listener capability remains retained until every request, probe, stream, upgrade bridge, and other task that could initiate or reuse upstream I/O has quiesced. Only then may locald release the final old capability guard.
12. Release removes only the exact current lease.
13. Every asynchronous mutation compares the fence appropriate to the authority it can invalidate, as defined below.

The protocol uses six purpose-specific fences:

- the **acquisition fence** is the server-issued attempt, publisher principal, listener identity, acquisition snapshot, acknowledged origin, and attempt deadline that must still match at lease-install commit;
- the **lease identity fence** contains the random daemon epoch, monotonically increasing per-service lease generation, unguessable token, and exact publisher principal;
- the **binding fence** adds the binding revision and identifies one exact retained listener capability;
- the **origin-authorization fence** adds to the binding fence the exact primary semantic origin and server-issued attempt whose request acknowledged it;
- the **health fence** adds to the origin-authorization fence the exact declaration authority, health-policy revision, and traffic-scope revision under which a probe began;
- the **expiry fence** adds the renewal revision and expiry deadline to the lease identity fence.

Each configured published `ServiceKey` owns one bounded registry slot. It is either vacant with its last publication generation, one pending or in-flight acquisition attempt, one terminal attempt result awaiting bounded replay or explicit replacement, or one live lease with at most one current rebind-attempt record. There is no collection of retired attempt handles, nonces, or tombstones. Acquisition preparation while the slot contains a live lease returns `already_published` and directs that principal to renew or rebind; it never allocates parallel state. A beginning operation issues an opaque, unguessable handle bound to the daemon epoch, exact kernel-observed principal, `ServiceKey`, reserved generation, current snapshot or binding revision, and a short suspend-inclusive monotonic deadline. Repeating begin from the same principal while that attempt is pending, in flight, or terminal returns or joins the same current handle, exact semantic origin, remaining lifetime, and any terminal result without extending its deadline or advancing its generation; another principal receives `acquisition_in_progress` or `rebind_in_progress`. Replacing a terminal attempt is an explicit compare-and-swap naming the current terminal attempt handle. Exactly one replacement advances the attempt generation and installs a fresh server-issued attempt; a delayed or duplicate replacement naming a stale handle fails. At the exact attempt deadline, pending or in-flight work is canceled and can no longer commit; the slot becomes eligible for a fresh attempt. Unknown handles and handles that are expired, replaced, from another principal or epoch, or no longer current for that one slot are rejected and can never be interpreted as fresh operations. This bounds retained state and expensive in-flight work to constant space per declared published service. A same-user process may still occupy that service's slot until its short deadline or proven process death, matching the accepted same-service denial boundary without creating cross-project memory growth.

A lease or attempt deadline exists only in daemon memory and is measured against one daemon-lifetime, suspend-inclusive monotonic clock, never wall-clock `SystemTime`. The acquisition lease deadline is its commit time plus the server-selected TTL; every successful renewal replaces it with the renewal commit time plus that TTL, rather than adding TTL to the old deadline. Route selection, health commit, wait, renew, rebind, release, attempt expiry, and lease expiry all consult this same clock. Wall-clock correction cannot extend or shorten publication authority. On wake, a serialized registry barrier runs before publication operations, route selection, or existing traffic scopes may perform upstream I/O: it establishes elapsed suspend time, reads the suspend-inclusive clock, expires elapsed attempts and leases, cancels every pre-suspend traffic and candidate scope, and gives each still-active binding a fresh `probe_only` scope with no route authorization. The barrier cannot race configuration transfer or revocation, pause or Resume, acquisition, or rebind commit against a stale registry snapshot. An unexpired publisher therefore keeps its lease but must pass health again; an expired publisher must prepare and acquire again. If the platform or a wake transition cannot establish trustworthy suspend-inclusive elapsed time, locald retires every pending attempt, live publication lease, binding, traffic scope, candidate, and route authorization. Authority-scoped waits terminate as lease lost, unpaused projects return `waiting_for_publisher`, paused projects remain `route_paused` and become `waiting_for_publisher` only after explicit Resume, and publishers must obtain fresh server-issued attempts. locald never substitutes wall time or preserves authority with an unknowable deadline.

A lease is active only while its current monotonic deadline is strictly later than the current lease-clock value. Each complete expiry fence arms a deadline task. At the deadline, that task enters the registry transition, confirms the complete expiry fence is still current and elapsed, withdraws route authorization, cancels whatever traffic and candidate scopes are then current for that lease—including all live WebSocket, upgraded, streaming, pooled, probe, and delayed-connect work—and retires the lease and binding. The root listener capability remains guarded only until the canceled work quiesces as described below. This deadline transition ends both new and already-selected reachability; a later cleanup reaper only reclaims quiesced state and does not define when authority ends. Renewal compares the lease identity fence, atomically confirms the lease is still active, and replaces the renewal revision, monotonic deadline, and armed deadline task as one transition. The existing scopes retain their cancellation handles, so the replacement task can cancel them if they remain current; a task captured before successful renewal fails its expiry-fence comparison and cannot cancel or expire the renewed lease.

Every successful acquisition and renewal response includes publisher-private relative `renew_after` and `expires_in` durations computed from the current monotonic deadline when the response is serialized. The server chooses `renew_after` strictly before expiry with enough policy-defined margin for ordinary scheduling jitter; the exact values remain Stage 2 details. Before sending either request, the publisher records its own suspend-inclusive monotonic request-start instant and schedules the next renewal no later than `request_start + renew_after`; if that instant has already passed when the response arrives, it renews immediately. The publisher's timer must advance across system suspend. If its platform cannot provide such a timer, the publisher must observe every wake and attempt renewal immediately, before its ordinary timer fires or it returns another stable-origin launch; an expired lease then fails and requires fresh preparation rather than being treated as renewed. A suspend-pausing monotonic timer without this wake rule is nonconforming. This conservatively charges IPC delay and suspend against the returned margin without comparing clocks across processes. Publishers never infer the policy from wall-clock timestamps or hard-coded intervals. A lost renewal response is retried with the same lease handle: if that exact lease remains active, the retry performs an ordinary fresh renewal whose deadline is its own commit time plus TTL and returns a new schedule; if authority has actually expired or retired, it fails as lease lost. An exact acquisition replay recomputes the remaining schedule without extending the deadline. While the lease and original acquisition binding remain active, insufficient normal margin produces `renew_after = 0` plus the true positive `expires_in`; acquisition replay reports expiry only after the monotonic deadline has elapsed or the lease has been atomically retired.

The first acquisition request for a current server-issued attempt records the exact listener-capability identity before lease-commit work begins. Concurrent exact duplicates join that one operation and receive its terminal result; reuse with another listener, request, principal, service, origin acknowledgement, or attempt generation is rejected. On success, the one per-service slot becomes the live lease and retains only the attempt fingerprint, acknowledged origin, and original listener identity needed for exact response replay. A lost response may be replayed only while that exact lease and original acquisition binding remain current; it returns the same lease handle, original binding revision, and recomputed remaining schedule without renewing or changing lifecycle state. Replay never substitutes a later rebind's revision and fails as superseded after binding replacement. Expiry, release, publisher death, incompatible configuration invalidation, project-instance retirement, takeover, or daemon restart removes the live replay tuple. A delayed old request then sees no matching current attempt or lease and fails as stale or lease lost; because only a current server-issued attempt can acquire, it can never become a new lease. A different publisher may take over immediately only after the daemon proves the previous publisher process is gone; otherwise it waits for expiry or an explicit future handoff mechanism. Successful takeover begins with a fresh server-issued attempt and reserved lease generation.

The first rebind request for a current server-issued attempt records its exact candidate listener identity and acknowledged origin before candidate work begins. Concurrent exact duplicates join one probe and compare-and-swap and receive one terminal result; mismatched reuse is rejected immediately. A successful binding swap, replacement origin acknowledgement, and terminal success result commit in the same registry transition; a terminal failure is likewise recorded before joined callers are released. A joined or later replay returns that result without renewing the lease, reproving health, or changing any scope. If the committed binding revision is still current, replay returns that installed revision; if a later rebind, expiry, release, configuration retirement, project-instance retirement, or daemon restart superseded its authority, replay fails as superseded or lease lost rather than reporting another binding. A recorded failure replays without another probe. Starting a replacement server-issued attempt atomically invalidates the prior handle, and an old attempt can never affect its successor.

Health results compare the health fence and confirm that the current lease is still active at commit time; ordinary renewal does not make an otherwise current health result stale. No health work begins before the exact binding's origin acknowledgement is committed. Changing the effective health policy advances both the health-policy and traffic-scope revisions, cancels the prior scope, marks the binding `checking_endpoint`, and makes it ineligible for routing until a probe in the replacement `probe_only` scope succeeds. A result from an older policy, origin acknowledgement, or canceled scope cannot commit after that transition. A successful current probe may authorize routing only through an atomic promotion transaction that verifies its complete health fence, active lease, origin acknowledgement, and declaration authority, observes the project unpaused, advances the traffic-scope revision, installs the successor `routable` scope, and creates route authorization keyed to that successor. Advancing the revision makes every other callback from the source scope stale; health evidence is never copied outside this verified transaction. A candidate rebind probe runs in its own `probe_only` scope and captures the current acknowledged origin, health-policy revision, declaration authority, and pause state. Every project pause or Resume transition cancels all candidate scopes, so a candidate begun across either boundary cannot commit. The rebind commit compares the lease identity fence, expected binding revision, server-issued attempt, candidate origin acknowledgement, captured declaration authority, candidate scope identity, and pause state, then reconfirms that the lease, attempt, and candidate scope remain active. If unpaused, it atomically installs the candidate binding, origin acknowledgement, fresh `routable` traffic scope, and route authorization derived from that exact successful candidate result. If paused, it installs the candidate binding and acknowledgement with a fresh `probe_only` scope and no route authorization. A candidate probed under a replaced declaration authority, wrong origin, canceled scope, changed pause state, or expired lease or attempt cannot commit. Release and process-exit reaping compare the exact current lease identity.

Configuration application, acquisition preparation, and project-instance retirement share the host-convergence boundary; configuration application, lease installation, project pause and Resume, project-instance retirement, and the wake barrier share the registry transition boundary. Host convergence writes only the complete set for the exact catalog/domain generation and instance set revalidated immediately before the write and records successful synchronization for that generation. A retirement holds the host-convergence boundary from that final revalidation through its complete-set host update and exact-instance registry commit. An older preparation therefore either writes first and is superseded by retirement's complete set, or observes the retired instance and fails before writing. A failed retirement host update preserves or restores the last complete mapping and commits no catalog, domain, or publication removal. Failed or canceled older work can never supersede a newer mapping. Each live lease and route authorization carries the admitted configuration generation, published type, exact primary origin, origin acknowledgement, and effective health-policy revision. A route may be selected only when that declaration authority and acknowledgement match the current configuration and the concrete host is its canonical primary origin. Configuration transitions apply these rules atomically:

- An alias-only or unrelated reload may preserve the lease and binding only after proving type, primary origin, and effective health policy unchanged. It transfers the lease to the new declaration authority and, if routing was authorized, replaces the old immutable authorization with one for the same binding and traffic scope under that authority.
- A health-policy change preserves the lease and binding but transfers them to the new declaration authority, withdraws route authorization, cancels old ordinary and candidate scopes, and starts a fresh `probe_only` scope under the replacement policy. Routing resumes only after that policy succeeds.
- Declaration removal, a change away from `type = "published"`, or a primary-origin change retires every pending attempt, lease, binding, scope, origin acknowledgement, and authorization. Removing and re-adding textually identical configuration is a new declaration authority and cannot revive an old attempt or lease.

Explicit project forgetting and automatic missing-instance pruning are project-instance retirement transitions even though neither is a configuration reload. Each resolves and then revalidates one exact `ProjectInstanceId`; a filesystem path remains only a locator. While holding the shared host-convergence boundary, retirement revalidates that exact instance and catalog/domain generation, synchronizes the complete host set without that instance, and commits the matching registry transition that removes the instance and its domain claims from the catalog. That transition also retires every pending, in-flight, candidate, or live publication whose `ServiceKey` belongs to the instance. It withdraws route authorization, cancels ordinary and candidate traffic scopes and authority-scoped waits, invalidates attempt and lease handles, and moves retained listener capabilities into the normal quiescing-retirement path. Catalog or domain removal is never visible while publication authority for the removed instance remains live, and no delayed preparation can rewrite a host set containing the retired instance after commit.

Acquisition preparation and commit, renewal, rebind, release, replay, expiry, health completion, forget, and pruning serialize so that either the publication mutation commits first and is immediately retired or instance retirement commits first and the mutation fails as lease lost. A stale forget or pruning snapshot must fail or restart when the exact catalog identity changed; it cannot retire a replacement instance that later occupies the same path. Failure before the retirement commit preserves both catalog and publication authority. After commit, delayed host cleanup or other housekeeping cannot restore any attempt, lease, binding, acknowledgement, route, or replay authority. The external process remains untouched.

Delayed invalidation compares the generation and declaration then current, so older work cannot revoke a newly installed or transferred lease. locald never exposes the old binding under a new origin. The publisher must prepare again, install and acknowledge the new origin, acquire, and pass health before routing resumes. Any delayed work tied to an endpoint also compares its binding revision and origin acknowledgement. A stale or expired publisher cannot regain authority, mutate a successor, or publish delayed health for it.

The initial threat boundary is same-daemon-user local development. UID, kernel-observed PID, process-birth identity, and unguessable server-issued handles prevent cross-user and stale-process mutations. They do not authorize one same-user program over another: any process already executing as the daemon user may attempt to claim an available published declaration. Consequently, another same-user program can race or squat an available declaration, deny service, falsely assert origin installation, or serve content under its semantic origin. This proposal accepts that boundary for the first consumer and does not describe the publisher as trusted. Origin acknowledgement establishes fail-closed protocol ordering for a cooperating publisher; it is not cryptographic proof of application behavior. Stronger code-signing allowlists or per-project capabilities are future hardening.

The transferred listener capability proves that the authenticated peer possesses the selected loopback listener without requiring the peer process itself to accept application traffic; external owners may intentionally hand the publisher-side copy to a child server. The accepted same-user boundary means a publisher can deliberately delegate that copy to another same-user process, but an unrelated process cannot take over the address merely because the serving child exits. Network-namespace proof is attached to the socket capability rather than inferred from that process relationship, so a listener transferred from a rootless container or other foreign namespace cannot designate a same-numbered host listener. locald independently probes health before routing. HTTP probes connect only through the selected loopback binding, send the exact semantic `Host` authority—hostname plus the advertised port when non-default—and send daemon-generated public-origin forwarding metadata. They never follow redirects. Redirect responses are unhealthy in the initial protocol rather than permission to probe another endpoint.

### 5.6 Lifecycle and availability

Published-service fulfillment is held in a separate ephemeral registry keyed by `ServiceKey`. It does not reuse attachment state or the current key-addressed availability lease.

The declaration and domain claims persist. Active publisher leases and upstream bindings do not persist across daemon restart.

Publication renewal is passive runtime maintenance:

- it does not change Exo lane focus;
- it does not create an editor, agent, or CLI attachment;
- it does not start, restore, or keep locald-managed sibling processes alive;
- it does not clear project pause or restore routing while paused.

Published-service readiness is independent from generic project readiness. `locald up` and `ensure_available` start and wait for the services locald can ensure. Every non-ready published state remains visible in project status and successful ensure output but does not turn the generic ensure into an impossible request. The consumer-specific launch workflow waits for its published service to become ready.

Project pause immediately suppresses the published route but never signals the external runtime. The pause transaction cancels the current traffic scope, route authorization, and every candidate rebind scope, then advances the scope revision and installs a paused `probe_only` scope for any retained binding. An existing publisher may continue renewing while paused. Project pause persists across daemon restart under the existing availability contract; a freshly prepared and acquired lease remains suppressed until explicit Resume. Resume atomically clears pause, cancels the paused scope and every candidate scope, advances the scope revision, installs a fresh unpaused `probe_only` scope for the exact current binding, and schedules an immediate probe. Only a success from that post-Resume scope may promote routing; otherwise the stable origin continues to explain what is missing.

The initial version does not implement service-level route suppression or external process control. `locald service stop <published-service>`, `locald service restart <published-service>`, and `locald service reset <published-service>` leave the route, binding, external process, and external data unchanged and explain that the service is externally managed. This avoids both a hidden pause policy that automatic publisher reacquisition could defeat and a destructive reset that locald has no authority to perform.

A publication-preparation request can load and register the declared project without first running ordinary `EnsureProject`. This avoids a bootstrap cycle:

1. the external owner validates its workspace and obtains the exact locald project-instance identity;
2. it prepares the declared publication with that expected identity and receives the server-issued attempt plus exact semantic origin;
3. it installs that origin in the application's authorization state, then starts or reuses its loopback endpoint;
4. it acquires with an exact origin acknowledgement and listener capability;
5. if the project is paused, the owning workflow stops immediately with actionable `locald up` or Resume guidance; publication does not clear the pause;
6. locald probes only the acknowledged binding;
7. the owner waits for the published service to become ready;
8. the owner returns the semantic-origin launch URL.

### 5.7 Lifecycle matrix

| Event | locald route and status | External process | Recovery |
|---|---|---|---|
| Declaration, no publisher | Stable unavailable surface; `waiting_for_publisher` | Unknown to locald | Start through the owning workflow |
| Lease acquired, probe pending | Stable loading surface; `checking_endpoint` | Untouched | Wait for locald-observed health |
| Probe fails | Stable degraded surface; `endpoint_unhealthy` | Untouched | External owner repairs or rebinds; locald keeps probing |
| Probe succeeds | Proxy exact binding; `ready` | Untouched | No action |
| Lease expires or publisher dies | Stable unavailable surface; `waiting_for_publisher`, or `route_paused` while the project remains paused | Never signaled | Publisher prepares and acquires a new lease generation |
| Project pause | Stable paused surface; `route_paused` | Untouched; lease may renew | Explicit project Resume |
| Project resume | Fresh probe-only scope; route restored only after its exact binding passes a post-Resume probe | Untouched | Otherwise wait for publisher or current-scope health |
| Service stop, restart, or reset command | Actionable externally-managed result; no route, binding, or data change | Untouched | Use the owning workflow |
| Daemon restart | Project pause persists; declaration and origin remain; attempts and lease are absent | Untouched | Publisher prepares and acquires with the new daemon epoch; route remains paused until Resume |
| Declaration removed or type changed | Domain and publication removed through normal config application | Untouched | Restore a valid declaration before publishing |
| Primary semantic origin changed | Old lease and binding revoked; new origin waits for a publisher | Untouched | Publisher prepares again, authorizes the new origin, acquires, and passes health |
| Project forgotten or missing instance pruned | Catalog declaration and domain claims removed; every publication under the exact instance is retired and live traffic is canceled | Untouched | Re-register or recreate the project and prepare a fresh acquisition; a recreated worktree is a new instance |

### 5.8 Endpoint and health states

locald exposes publication state without claiming process knowledge:

| Declaration | Lease | Probe | Route | Public state |
|---|---|---|---|---|
| present | absent | none | stable unavailable surface | `waiting_for_publisher` |
| present | active | pending | stable loading surface | `checking_endpoint` |
| present | active | failed | stable degraded surface | `endpoint_unhealthy` |
| present | active | healthy | proxy exact binding | `ready` |
| present | any | any | stable paused surface | `route_paused` |

Health is daemon-observed and continuous. Renewal never self-attests health. A current-scope healthy-to-unhealthy result atomically records the failure, withdraws the upstream from routing, cancels the current traffic scope and its route authorization, and installs a fresh `probe_only` scope revision while preserving the lease, binding, listener capability, acknowledged origin, and semantic origin. A late result from the canceled scope cannot restore routing. Repeated failures may remain within the current `probe_only` scope. When the project is not paused, a successful current `probe_only` result performs the atomic promotion described above: it cancels the probe scope, installs a fresh `routable` scope, and creates authorization for that exact new scope from the just-verified success. While paused, successful probes may update observed health but cannot promote or create route authorization. Resume replaces the paused scope with a new revision before scheduling its immediate probe, so a pre-Resume callback is stale even if it returns afterward; only a success from the new scope may promote. Health tasks compare the health fence and current active deadline, while route selection requires route authorization matching the exact current declaration authority, acknowledged canonical primary host, `routable` traffic-scope revision, binding fence, health-policy revision, and active deadline. Neither compares the renewal revision or the deadline value captured when a probe began, so a concurrent ordinary renewal neither discards a valid result nor authorizes an expired lease. A configuration reload that changes the effective health policy without changing the binding atomically advances the policy and traffic-scope revisions, withdraws the route to the stable loading surface, cancels the old scope and authorization, immediately installs a fresh `probe_only` scope under the replacement policy, and requires that policy to pass before routing resumes. At the exact deadline the route becomes ineligible even if the expiry task has not yet removed the record.

Lease loss immediately removes reachability. Normal domain ownership remains, so the stable origin serves an explanatory unavailable surface instead of falling through to another project or an unknown-domain response.

### 5.9 Atomic routing and origin preservation

The domain index remains the source of stable hostname ownership. A separate published-endpoint registry supplies the currently eligible upstream.

An origin-changing configuration application revokes the current lease, binding, traffic scope, and route authorization before publishing the new primary claim. The new origin begins in `waiting_for_publisher`; no atomic configuration snapshot can pair it with authority granted for the old origin. Alias-only changes that preserve the primary semantic origin do not require lease reacquisition because aliases never select the upstream: newly added exact or wildcard aliases redirect to the already-authorized primary origin before routing, and removed aliases simply stop redirecting.

The proxy resolves:

```text
concrete host
  -> DomainTarget(ProjectInstanceId, ServiceName)
  -> redirect-only alias? redirect to primary semantic origin
  -> current healthy PublishedEndpointLease
  -> 127.0.0.1:<private-port>
```

A successful rebind installs the new endpoint and a new binding revision atomically. Every proxy and health-probe connection pool belongs to one traffic-scope revision under one binding revision. A canceled scope is permanently closed and cannot be reused; a successor binding always receives a new binding revision and fresh traffic-scope revision and pools, even when it reuses the same loopback port.

Route suspension and binding retirement have distinct capability lifetimes. Project pause, health withdrawal, or health-policy replacement suspends routing: new requests cannot select the binding, the current traffic scope and route authorization are canceled, its idle pools and ongoing upstream I/O are closed, and WebSocket or other upgraded bridges are actively closed. The lease, binding, and root listener capability remain retained. locald immediately advances the traffic-scope revision and installs a fresh route-ineligible `probe_only` scope whose only upstream work is probing the exact binding under the current health policy. Pause and Resume each create distinct scopes and cancel all candidate rebind scopes, so neither ordinary nor candidate health evidence crosses either boundary. Once a success commits while unpaused, one atomic transaction cancels the recovery scope, advances the scope revision again, installs a fresh `routable` scope with fresh proxy and probe pools, and creates route authorization for precisely that new scope from the verified source health fence. Thus recovery never depends on work that suspension already canceled, never reuses a canceled pool, and never leaves the new scope dependent on health evidence fenced to its predecessor.

Rebind, lease expiry or release, configuration invalidation, project-instance retirement through explicit forget or automatic pruning, and daemon shutdown retire the binding: routing authority and its route-authorization record are removed immediately, the traffic scope is canceled, and its pools and I/O are closed. Every request, delayed connect, probe, stream, and upgrade bridge holds a binding capability guard until it has acknowledged cancellation and can no longer initiate or reuse upstream I/O. The retired binding's root capability and task guards remain retained until the scope and every pool have quiesced; only then may locald release the final capability. A finite response whose upstream body was already fully consumed may finish delivering buffered bytes, but a suspended or retired publisher cannot retain reachability through a stream, upgraded connection, delayed connect, or pooled connection. Proxy responsiveness caches include the binding revision, and health caches include the complete health fence, so a successor or replacement policy cannot inherit stale evidence.

HTTP and WebSocket forwarding on the primary origin preserve the public semantic `Host` and `Origin` by default. locald does not blindly rewrite them to loopback. A request or upgrade handshake on any non-primary exact or wildcard alias is redirected to the canonical primary origin, preserving path and query, before lease or upstream resolution; standard HTTP aliases redirect directly to canonical HTTPS in one hop. Alias API and WebSocket clients must use the canonical origin returned by publication and are not promised transparent redirect handling. A primary-origin request arriving through the standard port-80 front door is likewise redirected to the exact semantic HTTPS origin before any upstream is selected. Plaintext requests are never proxied while labeled as secure.

For standard HTTPS proxying, explicit sandbox proxying, and health probes, locald removes caller-supplied forwarding metadata and supplies canonical forwarding scheme and authority derived from the selected semantic origin: `https` and its exact public authority in standard mode, or the explicitly advertised sandbox scheme and authority in sandbox mode. The exact compatible header encoding is a Stage 2 detail. This synthesized metadata is advisory request context for framework behavior such as absolute-URL and secure-cookie generation; it is not proof that a request traversed locald, because another local process can connect to the private listener and forge the same headers. Applications must not use it alone for authentication, authorization, CSRF bypass, or capability binding. The application owner must explicitly accept the exact semantic origin returned by publication and retain its own request authorization. Exo continues to bind tickets and sessions to the exact workspace and public origin.

For Exo, every workbench ticket and session must be bound to both its exact workspace and the returned public origin. A ticket minted for one worktree origin must fail through another worktree origin even when both leases point at the same Exo daemon endpoint.

### 5.10 Status, dashboard, and agent context

Human, dashboard, and normal agent projections expose:

- service type `published`;
- stable semantic origin;
- publication state `waiting_for_publisher`, `checking_endpoint`, `endpoint_unhealthy`, `ready`, or `route_paused`;
- project pause and ordinary availability reasons;
- an actionable next step that names the external owner boundary.

They never claim that the external process is running or stopped, and never expose:

- private port or raw upstream;
- publisher UID, PID, process birth, executable, or acquisition or rebind attempt handle;
- lease token, daemon epoch, lease generation, or binding revision;
- Exo lane contents, ticket, session, workspace key, or browser grant.

A configured service appears in status before a publisher exists. Observational `inspect`, status polling, domain requests, and log reads never acquire or renew a publisher lease.

locald can report publication lifecycle events, but external process stdout and stderr remain owned by the publisher. A future explicit log-ingestion protocol is outside this RFC.

### 5.11 Exo consumer contract

Exo remains authoritative for the workbench and lanes.

When a project declares a published workbench and locald is available:

1. Exo validates the exact workspace and resolves the focused lane.
2. When no current lease exists, Exo uses locald's ambient resolution to obtain the exact `ProjectInstanceId`, begins acquisition with that expected identity and the workspace locator, and receives the server-issued attempt plus exact semantic origin.
3. Exo installs the exact workspace-and-origin authorization mapping, then starts or reuses its loopback workbench host.
4. Exo acquires by echoing that origin with the listener capability. For a current lease, it renews and schedules the next renewal from the returned publisher-private relative schedule.
5. If locald reports `route_paused`, Exo fails immediately with actionable `locald up` or Resume guidance instead of waiting or returning an unreachable URL.
6. Exo uses its lease handle and expected binding revision to wait for that exact acknowledged binding to become ready; replacement or rebind fails this launch attempt rather than silently observing a successor.
7. Exo mints a fresh workspace-and-origin-bound launch ticket and returns a URL using that origin and ticket fragment.

The lane identifier never enters locald configuration, identity, status, or hostname. A focus change may reuse or atomically rebind the endpoint without renaming the service.

Projects that do not opt in keep Exo's current direct-loopback behavior. An opted-in repository also retains that compatibility path on a machine where Exo establishes that locald is not installed at all. This preserves Exo's existing availability before locald participates: the launch result must identify the direct loopback origin, and locald itself never degrades its standard networking contract or claims that fallback as a locald service. A missing daemon socket alone does not prove absence. If Exo discovers any locald installation but cannot reach the daemon, negotiate a compatible publication API, or resolve the declared service, it fails with actionable start, upgrade, setup, or configuration guidance. After resolving the declaration, failed preparation, publication, health, or origin authorization likewise remains on the stable-origin path and never silently switches away from it. A user who deliberately wants locald's constrained unprivileged behavior must still select sandbox mode explicitly.

## 6. Implementation Boundaries

The first locald implementation is bounded to:

- the narrow typed configuration variant;
- an ephemeral constant-space per-service publisher registry, suspend-inclusive monotonic attempt and lease clock, wake barrier, and deterministic clock-driven tests;
- same-user server-issued acquisition and rebind attempts, acknowledged-origin acquire and rebind, exact-binding readiness wait, renew, and release IPC with ownership-preserving loopback-listener transfer and same-network-namespace proof;
- acquisition preparation through the existing atomic domain-claim and required hosts-synchronization contract;
- continuous endpoint health with health-policy-revision fencing across configuration reloads;
- health-gated proxy resolution with application-level HTTP probes, semantic-origin forwarding, standard HTTP-to-HTTPS redirect, no-redirect probe handling, binding-revision-scoped connection pools, and capability-guarded connection cancellation;
- independent published-service status and safe agent projections;
- config reload with origin-change revocation, project pause, expiry, daemon-restart behavior, and exact-instance publication retirement through explicit project forget and automatic missing-instance pruning;
- one Exo workbench integration proof.

It does not include:

- arbitrary remote hosts, URLs, or non-loopback endpoints;
- remote publication;
- publisher process adoption or signaling;
- service-level published-route pause or execution of stop, restart, or reset against the external runtime;
- publisher log ingestion;
- lane-aware hostnames or locald knowledge of Exo state;
- service dependencies involving published services in either direction;
- persisted live leases across daemon restart;
- a public CLI intended for manual port management;
- RFC 0147 reconciliation.

## 7. Validation Boundaries

The implementation must prove:

- two linked worktrees with the same declared service receive independent stable origins and leases;
- branch change, detached HEAD, rebase, and worktree move preserve identity and origin;
- a lane focus change never changes the hostname;
- one external endpoint may safely fulfill several worktree identities;
- unknown, undeclared, wrong-instance, or non-published services cannot be published; a delayed Begin carrying the retired instance identity is rejected after that path is reused by a replacement instance;
- omitted domains produce the conventional exact origin, while an empty or wildcard-only domain list and a missing primary exact origin fail configuration validation; adding or removing a non-primary exact or wildcard alias leaves the active lease and binding unchanged, and requests or upgrade handshakes on every alias redirect to the primary origin before upstream selection;
- omitted health configuration performs an application-level HTTP `/` probe, while TCP and command probes are rejected for published services;
- dependency edges from a published service and dependency edges from a managed service to a published service both fail configuration validation;
- wrong UID, spoofed PID, PID reuse, forged token, wrong epoch, and stale generation fail closed;
- a raw port, non-listener descriptor, non-loopback listener, shareable listener, descriptor from an unauthenticated publisher, listener whose kernel-observed network namespace differs from the daemon's, and listener whose namespace equivalence cannot be proven on a namespace-capable host are rejected; a same-UID publisher in another Linux network namespace cannot cause locald to probe an unrelated host listener on the same numeric port; and locald's retained capability prevents cross-user address takeover for the active binding in the namespace its proxy uses;
- locald's active standard and sandbox listener ports cannot be acquired or rebound as published endpoints;
- first-use preparation publishes the complete domain claims and finishes every required host mapping before returning its attempt and origin; preparation and configuration application serialize host convergence, revalidate the generation before each complete-set write, and record the synchronized generation; a host-synchronization failure or canceled stale retry preserves the last complete mapping, retains only one bounded terminal attempt result and no lease, and returns actionable setup or conflict guidance; and config-change-during-sync ordering never lets an older generation overwrite a newer one;
- every declared published `ServiceKey` retains at most one acquisition slot and one rebind-attempt slot inside a live lease; the first begin reserves that slot before host convergence or candidate work, concurrent same-principal begins join it, and a competing principal cannot replace it; a lost begin response retried by the same principal returns the same pending, in-flight, or terminal attempt handle, exact semantic origin, remaining lifetime, and terminal result without extending the deadline or repeating authority-changing work; terminal replacement is an explicit compare-and-swap naming the current handle, and stale or duplicate replacement fails; nonexistent services retain no attempt state; and arbitrarily many begin/fail, acquire/release, rebind, and expiry cycles leave constant state with no historical nonce or handle collection;
- acquisition is idempotent only for an exact current server-issued attempt, principal, service, request, acknowledged origin, and listener replay; concurrent exact duplicates join one bootstrap and commit; replay returns the original acquisition revision only while that binding remains current, fails as superseded after rebind, and recomputes the remaining relative renewal schedule without extending the deadline or altering binding, scope, pause, or health; another listener or replaced, expired, retired, wrong-epoch, or delayed handle cannot create a lease; and concurrent different publishers produce one winner;
- no endpoint probe or route authorization exists before the publisher receives the daemon-derived semantic origin, installs it in application authorization, and echoes it in the exact acquisition request; wrong-origin acknowledgement fails, only health evidence from the resulting origin-authorization fence may promote routing, a lost acquisition response remains safe because acknowledgement preceded the request, and a publisher's false acknowledgement remains inside the explicitly accepted same-user threat boundary;
- acquisition preparation or commit racing declaration removal, type change, primary-origin change, alias-only reload, health-policy reload, remove/re-add ABA, pause, Resume, forget, and pruning in both commit orders either commits against the exact revalidated declaration authority, instance identity, origin acknowledgement, and pause state or restarts/fails without installing stale authority; pause-first acquisition installs only a paused probe-only scope, while Resume-first acquisition observes the unpaused state;
- an alias-only or unrelated reload transfers a live lease, acknowledgement, and any route authorization only after proving the routing-relevant declaration unchanged; a health-policy reload preserves the lease, binding, and acknowledged origin but withdraws authorization and reprobes; any configuration change invalidates a pending acquisition attempt; declaration removal, type change, primary-origin change, and remove/re-add retire every attempt and live authority; and route selection rejects any mismatch in admitted generation, published type, acknowledged primary origin, policy, or canonical host;
- the authority-scoped readiness wait succeeds only for current route authorization on the exact lease handle, expected binding revision, and origin acknowledgement; remains pending through checking, unhealthiness, renewal, alias-only transfer, and health-policy reprobe; terminates on timeout, cancellation, replacement, rebind, expiry, release, pause, declaration or primary-origin invalidation, project-instance forgetting or pruning, and daemon restart; and never mistakes a successor's `ready` status for the caller's binding;
- stale renewal, rebind, release, expiry, health callback, process reaping, forget, and pruning cannot affect a successor; renewal atomically replaces the deadline task and an expiry task captured before renewal cannot remove or cancel the renewed lease; exact expiry enters the registry transition, withdraws new route selection, and cancels every then-current HTTP, WebSocket, upgraded, streaming, pooled, probe, candidate, and delayed-connect scope before cleanup reaping; a health probe overlapping ordinary renewal can still update the unchanged acknowledged binding; a late healthy result from a canceled, unacknowledged, or prior-origin scope cannot undo an unhealthy transition or alter its promoted successor; a current probe-only success atomically creates route authorization for the exact successor scope so recovery reaches `ready`; pause and Resume each fence pre-transition ordinary and candidate probes, Resume requires a post-transition success, and a rebind committing while paused installs no route authorization; a health-policy reload withdraws routing, fences out both ordinary and candidate-rebind results from the old policy, creates a fresh probe-capable recovery scope, and requires the replacement policy to pass; and at the exact deadline routing, renewal, and rebind fail closed even before cleanup reaping runs;
- every rebind uses a current server-issued attempt bound to the exact lease and expected revision; the candidate request acknowledges the current daemon-derived origin before any candidate probe; concurrent exact duplicates join one probe and compare-and-swap; a wrong-origin or failed candidate preserves the healthy old route; the successful binding, acknowledgement, and terminal result commit atomically; exact replay of a lost committed response returns the installed revision only while that binding remains current; exact replay of a recorded failure returns that failure without reprobing; mismatched or replaced handles are rejected; and neither an old attempt nor a stale expected revision can observe or mutate a successor;
- suspending a route for pause, unhealthy state, or health-policy replacement closes its traffic scope, route authorization, and pools while retaining the binding's root listener capability, immediately creates a new-revision route-ineligible current-policy probe scope, and atomically promotes a successful unpaused probe into exact-scope route authorization without rebinding or reacquiring the lease; pause keeps only its new current-scope health probes alive, Resume replaces that scope and triggers a fresh probe, and only its success may promote the recovered binding;
- retiring a binding closes its idle proxy and probe pools and actively closes its WebSocket, upgraded, and streaming connections; a warmed connection from revision N is never reused by revision N+1, a delayed connect cannot reach a replacement listener, and the old port remains unbindable until every task that could initiate or reuse upstream I/O has quiesced and released its capability guard;
- renewal does not change health, clear pause, or restore a paused route;
- lease loss and daemon restart preserve the declaration and origin but remove reachability;
- acquisition and renewal set their deadline to their own commit time plus TTL and return server-selected relative `renew_after` and `expires_in` values; acquisition and rebind attempts have their own short suspend-inclusive deadlines that bound pending and in-flight work; a publisher anchors `renew_after` to a suspend-inclusive monotonic request-start, renews conservatively despite delayed responses, and renews immediately when that target has passed; a publisher without a suspend-inclusive timer renews immediately on every wake before ordinary scheduling or another stable-origin launch, and a suspend-pausing timer without that wake behavior fails validation; a lost renewal response retried on the same active handle produces a fresh commit-time deadline and schedule; acquisition replay recomputes remaining time without renewing, returns an immediate-renew schedule while authority remains active but its normal margin is gone, and reports expiry only after the deadline elapses or authority retires; renewal never adds TTL to the prior deadline; wall-clock jumps do not affect authority; suspend time counts against every deadline; wake before or after a deadline, while paused, during Resume, configuration transfer or revocation, or candidate rebind serializes before acting on the registry and fences all pre-suspend work; every still-active binding is reproved in a fresh probe-only scope; and inability to establish suspend-inclusive elapsed time retires all attempts and publication authority, terminates exact waits, and requires fresh server-issued attempts;
- changing the primary semantic origin revokes every attempt, lease, acknowledgement, and binding, closes its connections, and requires fresh preparation, origin installation, acquisition, and health before the old upstream can appear under the new origin;
- explicit forget and automatic pruning each retire every pending, candidate, and live published service for the exact removed `ProjectInstanceId` in the same serialized transition that removes catalog and domain authority; after commit, new or already-selected HTTP, WebSocket, upgraded, streaming, pooled, probe, candidate, and delayed-connect work cannot reach the upstream, authority-scoped waits terminate, and stale attempt or lease operations fail as lease lost;
- forget and pruning races against preparation, acquisition, renewal, rebind, health completion, expiry, and path reuse produce either mutation-before-retirement followed by immediate cancellation or retirement-before-mutation followed by rejection; a delayed removal for instance N cannot remove or revoke replacement instance N+1 at the same path, and only quiescing capability guards—not route authority—may remain after the catalog record disappears;
- no lifecycle action signals the external process;
- generic `locald up` can complete while a published service is independently non-ready, and its success output names every such service with its exact state—including waiting, checking, unhealthy, or paused—instead of presenting a bare `Ready`;
- the owning launch workflow waits for the exact published binding to become ready;
- a launch attempted while the route is paused fails immediately with actionable `locald up` or Resume guidance and does not clear the pause;
- service-level stop, restart, and reset return an honest externally-managed result without changing route, binding, process, or external data;
- status and agent output distinguish all required publication states without leaking private authority or endpoints;
- HTTP, WebSocket, TLS, and Origin behavior work through the semantic domain; standard port-80 requests redirect to the exact HTTPS origin without reaching the upstream; HTTPS and sandbox proxying plus HTTP health probes replace untrusted forwarding headers with locald-generated public scheme and authority; that metadata is documented and tested as advisory rather than proxy authentication; probes send the exact semantic `Host`, including the advertised port when non-default, remain on the selected loopback binding, and never follow redirects;
- an Exo ticket for one worktree cannot be replayed through another worktree;
- Exo still returns a visibly direct-loopback launch when the project has not opted in or Exo establishes that locald is not installed at all; a discovered installation with no reachable or compatible publication API fails with actionable start or upgrade guidance, and after Exo resolves the declaration, publication, health, and origin-authorization failures remain on the stable-origin path without silent fallback.

## 8. Drawbacks

- This deliberately broadens the service model from exclusively managed runtimes to one explicit external-fulfillment mode.
- Generic project readiness can be Ready while a published service is waiting, so status must make the independent provider state unmistakable.
- A same-user malicious process remains inside the accepted initial trust boundary.
- External process logs and restart controls are not available through locald.
- A short lease requires renewal and careful clock-driven race testing.
- Server-issued prepare/acquire and rebind attempts add IPC round trips in exchange for fail-closed origin ordering and bounded replay state.
- Listener-capability transfer and platform socket validation add Unix-specific IPC work to the first implementation.
- HTTP-only health makes a non-HTTP externally owned runtime ineligible for the initial published-service type.
- An unhealthy endpoint preserves identity but cannot provide automatic runtime recovery.
- Exo must add origin-aware ticket and session validation before the stable origin is safe.
- Status must explain a process that can remain alive while locald suppresses or loses its route.

## 9. Alternatives

### 9.1 Put the lane in the hostname

Rejected. Lane focus is mutable Exo state, while the worktree service identity is stable locald state. Coupling them creates hostname churn and leaks Exo semantics into locald.

### 9.2 Let Exo own domains, TLS, and the reverse proxy

Rejected. It duplicates locald's platform authority and creates competing domain and certificate systems.

### 9.3 Store a static upstream URL or port in `locald.toml`

Rejected. It makes ephemeral allocation user-visible, survives crashes incorrectly, and lacks same-user identification, renewal, health, or successor fencing.

### 9.3.1 Publish a raw loopback port over authenticated IPC

Rejected. Peer authentication protects lease mutation but does not preserve ownership of the named address. If the serving process exits while its publisher keeps renewing, another user can bind the released unprivileged port and inherit the trusted semantic origin after health recovery. An ownership-preserving listener capability closes that substitution window.

### 9.4 Model the Exo workbench as `type = "exec"`

Rejected. locald would become responsible for Exo workspace validation, process lifecycle, launch context, and application authorization, or would only pretend to own them.

### 9.5 Reuse attachments or availability demands

Rejected. They model project interest, have different persistence and pause semantics, and do not provide an exact token-and-generation successor fence.

### 9.6 Fake a `ServiceController` around the external process

Rejected. It risks routing stop, recovery, and signal behavior through an ownership abstraction that is false.

### 9.7 Add a generic reverse-proxy target

Rejected. An arbitrary URL expands the SSRF and confused-deputy surface and lacks a publisher lifecycle.

### 9.8 Use a long-lived IPC connection as the whole lease

Not selected. Connection lifetime is a useful supplementary signal, but a bounded renewable lease handles stalled owners and daemon reconnection explicitly within locald's current request-response IPC model.

### 9.8.1 Use client-generated replay nonces

Rejected. Preventing a delayed nonce from becoming a fresh acquisition after lease retirement requires retaining attacker-controlled tombstones. A single server-issued current-attempt slot per declared service rejects stale handles while keeping memory and in-flight work bounded.

### 9.9 Use `[endpoints.workbench]` instead of `type = "published"`

Rejected for this proposal. It avoids extending the meaning of a service fulfillment type but duplicates identity, domain, status, dashboard, and agent abstractions. The explicit published variant preserves the Process Ownership axiom because locald never treats the external runtime as its child.

### 9.10 Make published services gate generic project readiness

Rejected. locald cannot start the external provider, so generic `locald up` would become an impossible cold-start request whenever the provider is absent. The provider-specific launch workflow owns that readiness boundary.

### 9.11 Withdraw a published route through `locald service stop`

Deferred. Automatic publisher reacquisition could immediately defeat an unpersisted withdrawal. A future service-level route pause would need its own explicit, durable, user-visible policy rather than pretending to stop the external process.

## 10. Unresolved Questions

1. What server-selected lease duration and renewal interval balance quick crash cleanup with normal scheduling jitter?
2. Does the first implementation need an explicit live-publisher handoff token, or are same-principal rebind plus dead-principal or expiry takeover sufficient?
3. Should macOS optionally require an installer-managed Exo code-signing allowlist after the same-user version is proven?
4. Should IPv6 loopback be added after the fixed `127.0.0.1` contract is proven?
5. Which exact safe publisher-state fields belong in the existing service status schema versus a dedicated published-service detail object?

## 11. Future Possibilities

- explicit handoff between live publisher generations;
- installer-managed publisher code-signing requirements;
- per-project publication capabilities;
- IPv6 loopback;
- external log ingestion;
- a durable service-level route pause;
- optional dependency edges involving published services;
- authenticated proxy-to-upstream request metadata for consumers that need stronger provenance than advisory forwarding context;
- additional external runtime owners beyond Exo.
