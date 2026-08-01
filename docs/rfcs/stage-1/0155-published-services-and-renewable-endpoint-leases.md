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
2. Exo validates that exact workspace, resolves its focused lane, and starts or reuses its loopback workbench host.
3. Exo publishes that endpoint to the worktree's declared locald service.
4. locald verifies the declaration, observes endpoint health, and makes the stable trusted origin ready.
5. Exo mints a fresh workspace-and-origin-bound ticket and returns the stable-origin launch URL.

For a worktree with slug `proposal-session` and project base `exo.localhost`, the user sees:

```text
https://workbench.proposal-session.on.exo.localhost
```

The user never selects, remembers, or passes the private loopback port. Changing the focused lane does not change the hostname. Restarting Exo may briefly show locald's loading or unavailable surface, but successful republication restores the same origin.

### 2.2 Interaction with ordinary locald lifecycle

Published services are provider-activated. locald cannot honestly start an externally owned process, so an absent publisher or unhealthy endpoint does not prevent generic `locald up` or `ensure_available` from making locald-managed services ready. The published service remains independently visible as waiting, checking, unhealthy, paused, or ready. Successful generic ensure output must report every published service whose publication state is not `ready`, including `waiting_for_publisher`, `checking_endpoint`, `endpoint_unhealthy`, and `route_paused`; it must not present a bare `Ready` that conceals any of them.

A consumer that needs the published service must use its owning workflow. For the first consumer, `exo workbench launch` starts or reuses the host, publishes it, waits for locald-observed readiness, and only then returns the launch URL.

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
- **Listener capability**: a kernel-backed reference to an already-bound private loopback listener, transferred by the authenticated publisher and retained by locald so the address cannot be reassigned while that binding remains active.
- **Binding revision**: the compare-and-swap generation of the current loopback upstream within a publisher lease.
- **Traffic scope**: one monotonically revised, cancelable generation of proxy requests, health probes, streams, upgrade bridges, and upstream connection pools under a binding. Each scope is either `probe_only` or `routable`. Suspending a route retires its current scope without retiring the binding or its listener capability; recovery creates a fresh scope revision.
- **Route authorization**: immutable eligibility for one exact `routable` traffic-scope revision, binding revision, and health-policy revision. It is created only by an atomic promotion from a successfully committed health result and cannot be inherited by another scope.
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

Omitting `domains` uses the existing conventional exact service domain. An explicit domain list must contain at least one exact claim; `domains = []` and wildcard-only lists are invalid for a published service because they cannot supply its canonical semantic origin. The first exact claim remains the primary origin under existing domain rules. That primary is the only publisher-authorized application origin and the only usable origin returned by acquisition, launch, status, and agent context. Every other exact or wildcard claim is a redirect-only ingress alias: locald redirects a concrete request to the primary semantic origin, preserving path and query, before selecting any published upstream. The redirected canonical request is a separate request subject to the application's normal origin authorization; aliases are not transparent API or WebSocket origins. Supporting several independently routable application origins would require an explicit multi-origin authorization contract outside this initial proposal.

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

locald never adopts, signals, kills, or restores the external process. Lease expiry, release, configuration removal, project pause, daemon shutdown, and project forgetting affect locald reachability only.

### 5.5 Publication protocol

The daemon exposes a narrow host-only IPC protocol whose conceptual operations are:

```text
AcquirePublishedEndpoint(project_locator, service_name, session_nonce, listener_capability)
RenewPublishedEndpoint(lease_handle)
RebindPublishedEndpoint(lease_handle, expected_binding_revision, listener_capability)
ReleasePublishedEndpoint(lease_handle)
```

The exact request names and encoding remain Stage 2 details. The public contract is:

1. Every operation obtains the Unix-socket peer identity from the kernel.
2. The peer UID must equal the daemon UID.
3. The daemon obtains the peer PID from kernel credentials and captures a high-resolution process-birth identity.
4. Acquisition resolves the locator server-side, verifies the exact project instance, and requires `type = "published"` for the named service.
5. The endpoint input is an ownership-preserving capability for an already-bound TCP listener. On supported Unix hosts, the publisher transfers a duplicate listener file descriptor over the authenticated IPC connection. locald validates that it is a listening socket bound exclusively to `127.0.0.1` on a nonzero port, derives the private port from the socket, and retains its duplicate for the binding's lifetime. A raw port number is never publication authority. The retained descriptor prevents another process or UID from rebinding the selected address if the publisher closes its copy; locald never accepts application traffic from its copy. Exact ancillary-message encoding and platform socket checks remain Stage 2 details, but implementations must preserve this kernel-backed ownership property.
6. Acquisition and rebind reject a listener whose derived port equals any current standard or sandbox HTTP or HTTPS front-door listener, preventing a published route from recursively targeting locald itself. Hostnames, arbitrary IPs, URLs, Unix sockets as upstreams, caller-selected schemes, and socket configurations that permit an unrelated listener to share the binding are rejected by construction.
7. Acquisition establishes the lease, returns the stable semantic origin plus a redacted opaque lease handle, installs the binding's initial `probe_only` traffic scope, and begins locald-observed health evaluation. A retry-stable publisher session nonce makes a lost acquisition response idempotent.
8. The owning workflow waits for the exact binding to become ready before returning a user-facing launch URL. Renewal never self-attests readiness.
9. Renewal extends only the exact current lease. It does not revive an expired lease, change a binding, clear project pause, or restore routing while paused.
10. Rebind validates and probes a candidate capability, then compare-and-swaps the binding. A failed candidate is closed by locald and preserves the last healthy upstream. A successful swap retains the new capability, invalidates the old binding and its traffic scope, closes binding-scoped idle connection pools, and cancels binding-scoped work. The old listener capability remains retained until every request, probe, stream, upgrade bridge, and other task that could initiate or reuse upstream I/O has quiesced. Only then may locald release the final old capability guard.
11. Release removes only the exact current lease.
12. Every asynchronous mutation compares the fence appropriate to the authority it can invalidate, as defined below.

The protocol uses four purpose-specific fences:

- the **lease identity fence** contains the random daemon epoch, monotonically increasing per-service lease generation, unguessable token, and exact publisher principal;
- the **binding fence** adds the binding revision and identifies one exact retained listener capability;
- the **health fence** adds to the binding fence the exact health-policy revision and traffic-scope revision under which a probe began;
- the **expiry fence** adds the renewal revision and expiry deadline to the lease identity fence.

A lease is active only while its current deadline is strictly later than daemon time. Deadline expiry is authoritative even before the cleanup task runs; the reaper removes state but does not define when authority ends. Renewal compares the lease identity fence and atomically confirms the lease is still active before advancing only the renewal revision and deadline. An expiry task removes a lease only after atomically confirming that its complete expiry fence is still current and that the current deadline has elapsed. A timer captured before a successful renewal therefore cannot expire the renewed lease. Health results compare the health fence and confirm that the current lease is still active at commit time; ordinary renewal does not make an otherwise current health result stale. Changing the effective health policy advances both the health-policy and traffic-scope revisions, cancels the prior scope, marks the binding `checking_endpoint`, and makes it ineligible for routing until a probe in the replacement `probe_only` scope succeeds. A result from an older policy or canceled scope cannot commit after that transition. A successful current probe may authorize routing only through an atomic promotion transaction that verifies its complete health fence and active lease, observes the project unpaused, advances the traffic-scope revision, installs the successor `routable` scope, and creates route authorization keyed to that successor. Advancing the revision makes every other callback from the source scope stale; health evidence is never copied outside this verified transaction. A candidate rebind probe runs in its own `probe_only` scope and captures the current health-policy revision and pause state. Every project pause or Resume transition cancels all candidate scopes, so a candidate begun across either boundary cannot commit. The rebind commit compares the lease identity fence, expected binding revision, captured health-policy revision, and candidate scope identity, then reconfirms that the lease and candidate scope remain active and the pause state is unchanged. If unpaused, it atomically installs the candidate binding, its fresh `routable` traffic scope, and route authorization derived from that exact successful candidate result. If paused, it installs the candidate binding with a fresh `probe_only` scope and no route authorization. A candidate probed under a replaced policy, canceled scope, changed pause state, or expired lease cannot commit. Release and process-exit reaping compare the exact current lease identity. Configuration invalidation compares the exact configuration generation before removing the lease then current for that declaration, so delayed removal from an older configuration cannot revoke a newly reintroduced service. Removing the declaration, changing its type, or changing its primary semantic origin revokes the current lease and binding; locald never exposes the old binding under a new origin. The publisher must reacquire, receive the new origin, pass health again, and reauthorize its application before routing resumes. Any delayed work tied to an endpoint also compares its binding revision. A stale or expired publisher cannot regain authority, mutate a successor, or publish delayed health for it.

A different publisher may take over immediately only after the daemon proves the previous publisher process is gone; otherwise it waits for expiry or an explicit future handoff mechanism. Successful takeover creates a new lease generation.

The initial threat boundary is same-daemon-user local development. UID, kernel-observed PID, process-birth identity, and the unguessable handle prevent cross-user and stale-process mutations. They do not authorize one same-user program over another: any process already executing as the daemon user may attempt to claim an available published declaration. Consequently, another same-user program can race or squat an available declaration, deny service, or serve content under its semantic origin. This proposal accepts that boundary for the first consumer and does not describe the publisher as trusted. Stronger code-signing allowlists or per-project capabilities are future hardening.

The transferred listener capability proves that the authenticated peer possesses the selected loopback listener without requiring the peer process itself to accept application traffic; external owners may intentionally hand the publisher-side copy to a child server. The accepted same-user boundary means a publisher can deliberately delegate that copy to another same-user process, but an unrelated process cannot take over the address merely because the serving child exits. locald independently probes health before routing. HTTP probes connect only through the selected loopback binding, send the exact semantic `Host` authority—hostname plus the advertised port when non-default—and send daemon-generated public-origin forwarding metadata. They never follow redirects. Redirect responses are unhealthy in the initial protocol rather than permission to probe another endpoint.

### 5.6 Lifecycle and availability

Published-service fulfillment is held in a separate ephemeral registry keyed by `ServiceKey`. It does not reuse attachment state or the current key-addressed availability lease.

The declaration and domain claims persist. Active publisher leases and upstream bindings do not persist across daemon restart.

Publication renewal is passive runtime maintenance:

- it does not change Exo lane focus;
- it does not create an editor, agent, or CLI attachment;
- it does not start, restore, or keep locald-managed sibling processes alive;
- it does not clear project pause or restore routing while paused.

Published-service readiness is independent from generic project readiness. `locald up` and `ensure_available` start and wait for the services locald can ensure. Every non-ready published state remains visible in project status and successful ensure output but does not turn the generic ensure into an impossible request. The consumer-specific launch workflow waits for its published service to become ready.

Project pause immediately suppresses the published route but never signals the external runtime. The pause transaction cancels the current traffic scope, route authorization, and every candidate rebind scope, then advances the scope revision and installs a paused `probe_only` scope for any retained binding. An existing publisher may continue renewing while paused. Project pause persists across daemon restart under the existing availability contract; a reacquired lease remains suppressed until explicit Resume. Resume atomically clears pause, cancels the paused scope and every candidate scope, advances the scope revision, installs a fresh unpaused `probe_only` scope for the exact current binding, and schedules an immediate probe. Only a success from that post-Resume scope may promote routing; otherwise the stable origin continues to explain what is missing.

The initial version does not implement service-level route suppression or external process control. `locald service stop <published-service>`, `locald service restart <published-service>`, and `locald service reset <published-service>` leave the route, binding, external process, and external data unchanged and explain that the service is externally managed. This avoids both a hidden pause policy that automatic publisher reacquisition could defeat and a destructive reset that locald has no authority to perform.

A publication request can load and register the declared project without first running ordinary `EnsureProject`. This avoids a bootstrap cycle:

1. the external owner validates its workspace and starts its loopback endpoint;
2. it acquires the declared publication;
3. if the project is paused, the owning workflow stops immediately with actionable `locald up` or Resume guidance; publication does not clear the pause;
4. locald probes the endpoint;
5. the owner waits for the published service to become ready;
6. the owner returns the semantic-origin launch URL.

### 5.7 Lifecycle matrix

| Event | locald route and status | External process | Recovery |
|---|---|---|---|
| Declaration, no publisher | Stable unavailable surface; `waiting_for_publisher` | Unknown to locald | Start through the owning workflow |
| Lease acquired, probe pending | Stable loading surface; `checking_endpoint` | Untouched | Wait for locald-observed health |
| Probe fails | Stable degraded surface; `endpoint_unhealthy` | Untouched | External owner repairs or rebinds; locald keeps probing |
| Probe succeeds | Proxy exact binding; `ready` | Untouched | No action |
| Lease expires or publisher dies | Stable unavailable surface; `waiting_for_publisher` | Never signaled | Publisher starts a new lease generation |
| Project pause | Stable paused surface; `route_paused` | Untouched; lease may renew | Explicit project Resume |
| Project resume | Fresh probe-only scope; route restored only after its exact binding passes a post-Resume probe | Untouched | Otherwise wait for publisher or current-scope health |
| Service stop, restart, or reset command | Actionable externally-managed result; no route, binding, or data change | Untouched | Use the owning workflow |
| Daemon restart | Project pause persists; declaration and origin remain; lease is absent | Untouched | Publisher reacquires with new daemon epoch; route remains paused until Resume |
| Declaration removed or type changed | Domain and publication removed through normal config application | Untouched | Restore a valid declaration before publishing |
| Primary semantic origin changed | Old lease and binding revoked; new origin waits for a publisher | Untouched | Publisher reacquires, authorizes the new origin, and passes health |

### 5.8 Endpoint and health states

locald exposes publication state without claiming process knowledge:

| Declaration | Lease | Probe | Route | Public state |
|---|---|---|---|---|
| present | absent | none | stable unavailable surface | `waiting_for_publisher` |
| present | active | pending | stable loading surface | `checking_endpoint` |
| present | active | failed | stable degraded surface | `endpoint_unhealthy` |
| present | active | healthy | proxy exact binding | `ready` |
| present | any | any | stable paused surface | `route_paused` |

Health is daemon-observed and continuous. Renewal never self-attests health. A current-scope healthy-to-unhealthy result atomically records the failure, withdraws the upstream from routing, cancels the current traffic scope and its route authorization, and installs a fresh `probe_only` scope revision while preserving the lease, binding, listener capability, and semantic origin. A late result from the canceled scope cannot restore routing. Repeated failures may remain within the current `probe_only` scope. When the project is not paused, a successful current `probe_only` result performs the atomic promotion described above: it cancels the probe scope, installs a fresh `routable` scope, and creates authorization for that exact new scope from the just-verified success. While paused, successful probes may update observed health but cannot promote or create route authorization. Resume replaces the paused scope with a new revision before scheduling its immediate probe, so a pre-Resume callback is stale even if it returns afterward; only a success from the new scope may promote. Health tasks compare the health fence and current active deadline, while route selection requires route authorization matching the exact current `routable` traffic-scope revision, binding fence, health-policy revision, and current active deadline. Neither compares the renewal revision or the deadline value captured when a probe began, so a concurrent ordinary renewal neither discards a valid result nor authorizes an expired lease. A configuration reload that changes the effective health policy without changing the binding atomically advances the policy and traffic-scope revisions, withdraws the route to the stable loading surface, cancels the old scope and authorization, immediately installs a fresh `probe_only` scope under the replacement policy, and requires that policy to pass before routing resumes. At the exact deadline the route becomes ineligible even if the expiry task has not yet removed the record.

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

Rebind, lease expiry or release, configuration invalidation, and daemon shutdown retire the binding: routing authority and its route-authorization record are removed immediately, the traffic scope is canceled, and its pools and I/O are closed. Every request, delayed connect, probe, stream, and upgrade bridge holds a binding capability guard until it has acknowledged cancellation and can no longer initiate or reuse upstream I/O. The retired binding's root capability and task guards remain retained until the scope and every pool have quiesced; only then may locald release the final capability. A finite response whose upstream body was already fully consumed may finish delivering buffered bytes, but a suspended or retired publisher cannot retain reachability through a stream, upgraded connection, delayed connect, or pooled connection. Proxy responsiveness caches include the binding revision, and health caches include the complete health fence, so a successor or replacement policy cannot inherit stale evidence.

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
- publisher UID, PID, process birth, executable, or session nonce;
- lease token, daemon epoch, lease generation, or binding revision;
- Exo lane contents, ticket, session, workspace key, or browser grant.

A configured service appears in status before a publisher exists. Observational `inspect`, status polling, domain requests, and log reads never acquire or renew a publisher lease.

locald can report publication lifecycle events, but external process stdout and stderr remain owned by the publisher. A future explicit log-ingestion protocol is outside this RFC.

### 5.11 Exo consumer contract

Exo remains authoritative for the workbench and lanes.

When a project declares a published workbench and locald is available:

1. Exo validates the exact workspace and resolves the focused lane.
2. Exo starts or reuses its loopback workbench host.
3. Exo acquires or renews the worktree's published service lease.
4. If locald reports `route_paused`, Exo fails immediately with actionable `locald up` or Resume guidance instead of waiting or returning an unreachable URL.
5. Exo waits for locald to report the exact binding ready.
6. Exo mints a fresh workspace-and-origin-bound launch ticket and returns a URL using that origin and ticket fragment.

The lane identifier never enters locald configuration, identity, status, or hostname. A focus change may reuse or atomically rebind the endpoint without renaming the service.

Projects that do not opt in keep Exo's current direct-loopback behavior. Once a published declaration is present, it selects the standard locald platform contract: an absent or inconsistent locald installation, failed publication, failed health, or failed origin authorization returns actionable locald setup or recovery guidance and never silently switches to a high-port loopback origin. Any future direct-loopback escape hatch for an opted-in project must be part of the explicitly selected sandbox mode, with its constrained origin made visible; it is not part of the normal workflow defined here.

## 6. Implementation Boundaries

The first locald implementation is bounded to:

- the narrow typed configuration variant;
- an ephemeral per-service publisher registry and deterministic clock-driven tests;
- same-user acquire, renew, rebind, and release IPC with ownership-preserving loopback-listener transfer;
- continuous endpoint health with health-policy-revision fencing across configuration reloads;
- health-gated proxy resolution with application-level HTTP probes, semantic-origin forwarding, standard HTTP-to-HTTPS redirect, no-redirect probe handling, binding-revision-scoped connection pools, and capability-guarded connection cancellation;
- independent published-service status and safe agent projections;
- config reload with origin-change revocation, project pause, expiry, and daemon-restart behavior;
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
- unknown, undeclared, wrong-instance, or non-published services cannot be published;
- omitted domains produce the conventional exact origin, while an empty or wildcard-only domain list and a missing primary exact origin fail configuration validation; adding or removing a non-primary exact or wildcard alias leaves the active lease and binding unchanged, and requests or upgrade handshakes on every alias redirect to the primary origin before upstream selection;
- omitted health configuration performs an application-level HTTP `/` probe, while TCP and command probes are rejected for published services;
- dependency edges from a published service and dependency edges from a managed service to a published service both fail configuration validation;
- wrong UID, spoofed PID, PID reuse, forged token, wrong epoch, and stale generation fail closed;
- a raw port, non-listener descriptor, non-loopback listener, shareable listener, and descriptor from an unauthenticated publisher are rejected, while locald's retained capability prevents cross-user address takeover for the active binding;
- locald's active standard and sandbox listener ports cannot be acquired or rebound as published endpoints;
- retry-stable acquisition is idempotent and concurrent different publishers produce one winner;
- stale renewal, rebind, release, expiry, health callback, and process reaping cannot affect a successor; an expiry timer captured before renewal cannot remove the renewed lease; a health probe overlapping ordinary renewal can still update the unchanged binding; a late healthy result from a canceled routable or probe-only scope cannot undo an unhealthy transition or alter its promoted successor; a current probe-only success atomically creates route authorization for the exact successor scope so recovery reaches `ready`; pause and Resume each fence pre-transition ordinary and candidate probes, Resume requires a post-transition success, and a rebind committing while paused installs no route authorization; a health-policy reload withdraws routing, fences out both ordinary and candidate-rebind results from the old policy, creates a fresh probe-capable recovery scope, and requires the replacement policy to pass; and at the exact deadline routing, renewal, and rebind fail closed even before the reaper runs;
- a failed candidate rebind preserves the healthy route and a successful rebind is atomic;
- suspending a route for pause, unhealthy state, or health-policy replacement closes its traffic scope, route authorization, and pools while retaining the binding's root listener capability, immediately creates a new-revision route-ineligible current-policy probe scope, and atomically promotes a successful unpaused probe into exact-scope route authorization without rebinding or reacquiring the lease; pause keeps only its new current-scope health probes alive, Resume replaces that scope and triggers a fresh probe, and only its success may promote the recovered binding;
- retiring a binding closes its idle proxy and probe pools and actively closes its WebSocket, upgraded, and streaming connections; a warmed connection from revision N is never reused by revision N+1, a delayed connect cannot reach a replacement listener, and the old port remains unbindable until every task that could initiate or reuse upstream I/O has quiesced and released its capability guard;
- renewal does not change health, clear pause, or restore a paused route;
- lease loss and daemon restart preserve the declaration and origin but remove reachability;
- changing the primary semantic origin revokes the old lease and binding, closes its connections, and requires reacquisition and health before the old upstream can appear under the new origin;
- no lifecycle action signals the external process;
- generic `locald up` can complete while a published service is independently non-ready, and its success output names every such service with its exact state—including waiting, checking, unhealthy, or paused—instead of presenting a bare `Ready`;
- the owning launch workflow waits for the exact published binding to become ready;
- a launch attempted while the route is paused fails immediately with actionable `locald up` or Resume guidance and does not clear the pause;
- service-level stop, restart, and reset return an honest externally-managed result without changing route, binding, process, or external data;
- status and agent output distinguish all required publication states without leaking private authority or endpoints;
- HTTP, WebSocket, TLS, and Origin behavior work through the semantic domain; standard port-80 requests redirect to the exact HTTPS origin without reaching the upstream; HTTPS and sandbox proxying plus HTTP health probes replace untrusted forwarding headers with locald-generated public scheme and authority; that metadata is documented and tested as advisory rather than proxy authentication; probes send the exact semantic `Host`, including the advertised port when non-default, remain on the selected loopback binding, and never follow redirects;
- an Exo ticket for one worktree cannot be replayed through another worktree;
- Exo still returns a direct loopback launch when the project has not opted in, while an opted-in project fails with actionable guidance when locald is absent or inconsistent.

## 8. Drawbacks

- This deliberately broadens the service model from exclusively managed runtimes to one explicit external-fulfillment mode.
- Generic project readiness can be Ready while a published service is waiting, so status must make the independent provider state unmistakable.
- A same-user malicious process remains inside the accepted initial trust boundary.
- External process logs and restart controls are not available through locald.
- A short lease requires renewal and careful clock-driven race testing.
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

### 9.9 Use `[endpoints.workbench]` instead of `type = "published"`

Rejected for this proposal. It avoids extending the meaning of a service fulfillment type but duplicates identity, domain, status, dashboard, and agent abstractions. The explicit published variant preserves the Process Ownership axiom because locald never treats the external runtime as its child.

### 9.10 Make published services gate generic project readiness

Rejected. locald cannot start the external provider, so generic `locald up` would become an impossible cold-start request whenever the provider is absent. The provider-specific launch workflow owns that readiness boundary.

### 9.11 Withdraw a published route through `locald service stop`

Deferred. Automatic publisher reacquisition could immediately defeat an unpersisted withdrawal. A future service-level route pause would need its own explicit, durable, user-visible policy rather than pretending to stop the external process.

## 10. Unresolved Questions

1. What server-selected lease duration and renewal interval balance quick crash cleanup with sleep and wake tolerance?
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
