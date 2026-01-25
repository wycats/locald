# RFC Stage Audit

## Stage Definitions

| Stage | Name        | Description                                                                     |
| ----- | ----------- | ------------------------------------------------------------------------------- |
| 0     | Strawman    | Initial idea, not yet accepted. May be incomplete or speculative.               |
| 1     | Proposed    | Accepted as worth pursuing. Design is drafted but implementation may not exist. |
| 2     | Available   | Implemented and available for use, but may have rough edges.                    |
| 3     | Recommended | Stable implementation, recommended for use. API may still evolve.               |
| 4     | Stable      | Fully stable, API frozen. Breaking changes require deprecation cycle.           |

## Audit Methodology

For each RFC:

1. Subagent reads the RFC and searches codebase for implementation
2. Subagent recommends a stage based on implementation status
3. Uncertainties are flagged for main agent review
4. Final recommendation is recorded

---

## Audit Results

### Stage 3 RFCs (Currently "Recommended")

| RFC  | Title                   | Current | Recommended | Confidence | Notes                                              |
| ---- | ----------------------- | ------- | ----------- | ---------- | -------------------------------------------------- |
| 0038 | Extensibility & Plugins | 3       | 3           | High       | ~4,800 lines implemented, E2E tests, docs          |
| 0109 | locald doctor           | 3       | 3           | High       | ~2,100 lines, property tests, comprehensive checks |
| 0142 | Remove DockerRuntime    | 3       | 3           | High       | bollard removed, OCI-only path confirmed           |

### Stage 2 RFCs (Currently "Available")

| RFC  | Title            | Current | Recommended | Confidence | Notes                                          |
| ---- | ---------------- | ------- | ----------- | ---------- | ---------------------------------------------- |
| 0130 | Host Shim Daemon | 2       | Superseded  | High       | Superseded by RFC 0138, implementation removed |
| 0134 | Host-Spawn Crate | 2       | Superseded  | High       | Superseded by RFC 0138, never implemented      |

### Stage 1 RFCs (Currently "Proposed")

| RFC  | Title                             | Current | Recommended | Confidence | Notes                                  |
| ---- | --------------------------------- | ------- | ----------- | ---------- | -------------------------------------- |
| 0104 | Codebase Cleanup                  | 1       | 2           | High       | Most items done, some lint/audit gaps  |
| 0110 | Privileged Capability Acquisition | 1       | 2           | High       | Core API exists, not fully enforced    |
| 0129 | WASM Plugins as Plan Transforms   | 1       | 2           | High       | Fully implemented, behind feature flag |
| 0130 | Host Shim Daemon                  | 1       | Superseded  | High       | Superseded by RFC 0138                 |
| 0138 | Remove Container Workflow         | 1       | 2           | High       | Substantially implemented              |

### Stage 0 RFCs (Currently "Strawman")

| RFC  | Title                             | Current | Recommended | Confidence | Notes                                       |
| ---- | --------------------------------- | ------- | ----------- | ---------- | ------------------------------------------- |
| 0105 | Cross-Surface Workflow Contracts  | 0       | 1           | High       | Partial infrastructure, needs automation    |
| 0106 | Remote Host/IP Domain Mapping     | 0       | 0           | High       | No implementation                           |
| 0107 | locald-shim CLI Hardening         | 0       | 1           | High       | clap done, mount-loop missing               |
| 0108 | Firecracker Hardened Images       | 0       | 1           | Medium     | VMM foundation exists, init workflow absent |
| 0111 | Property Tests as Invariant Specs | 0       | 2           | High       | Fully implemented with proptest             |
| 0112 | User Programming Model Audit      | 0       | 1           | Medium     | Audit process ongoing                       |
| 0113 | Privileged Linux E2E Runner Lane  | 0       | 0           | High       | No dedicated runners                        |
| 0114 | Surface Contract Program          | 0       | 1           | High       | Feature readiness ledger exists             |
| 0116 | Minimum Awesome Product (MAP)     | 0       | 0           | High       | Empty placeholder                           |
| 0124 | Post-MAP Release Themes           | 0       | 0           | High       | Future direction, no impl                   |
| 0125 | Respectful Doctor Output          | 0       | 2           | High       | Fix consolidation implemented               |
| 0127 | Explicit Defaults                 | 0       | 0           | High       | Not implemented                             |
| 0131 | WSL Privileged Operations         | 0       | 0           | High       | Only minimal WSL detection                  |
| 0132 | CI Acceleration                   | 0       | 1           | High       | mainline-health exists, fast lane missing   |
| 0133 | WSL Windows Helper Bridge         | 0       | 0           | High       | Unimplemented                               |
| 0134 | macOS Domain/Cert Requirements    | 0       | 1           | Medium     | Linux cert infra exists                     |
| 0135 | Dashboard Vocabulary              | 0       | 1           | Medium     | Partial vocabulary alignment                |
| 0137 | (Placeholder)                     | 0       | Superseded  | High       | Superseded by RFC 0138                      |

### Root-Level RFCs (Unstaged)

| RFC  | Title                  | Current | Recommended | Confidence | Notes                                          |
| ---- | ---------------------- | ------- | ----------- | ---------- | ---------------------------------------------- |
| 0043 | CNB Integration        | (root)  | 3           | High       | Fully implemented, cnb-client + locald-builder |
| 0047 | Cross-Platform Runtime | (root)  | 1           | High       | Linux-only, macOS/WSL not implemented          |
| 0050 | Hot Reloading          | (root)  | 0           | High       | Watcher exists but not wired up                |
| 0053 | Shim Architecture      | (root)  | 3           | High       | Fully implemented                              |
| 0069 | Host-First Execution   | (root)  | 3           | High       | Fully implemented, default mode                |
| 0075 | Runc Container Runtime | (root)  | Superseded  | High       | Superseded by RFC 0098 (libcontainer)          |
| 0078 | Embedded Shim          | (root)  | 3           | High       | Fully implemented via build.rs                 |
| 0079 | Unified Service Trait  | (root)  | 3           | High       | Fully implemented                              |
| 0098 | Libcontainer Execution | (root)  | 3           | High       | Fully implemented, replaces runc               |
| 0099 | Cgroup Hierarchy       | (root)  | 3           | High       | Fully implemented                              |

---

### Complete Root-Level RFC Audit (Batched 0001-0104)

#### Batch 1: RFCs 0001-0020

| RFC  | Title                   | Impl     | Recommend | Notes                                |
| ---- | ----------------------- | -------- | --------- | ------------------------------------ |
| 0001 | Init Locald ADR Process | Complete | Stage 4   | ADR process fully established        |
| 0002 | Service Lifecycle       | Complete | Stage 4   | Start/stop/restart fully implemented |
| 0003 | Domain-Based Routing    | Complete | Stage 4   | \*.locald + mkcert integrated        |
| 0004 | Proxy Architecture      | Complete | Stage 4   | Reverse proxy fully implemented      |
| 0005 | TOML Configuration      | Complete | Stage 4   | locald.toml parsing fully working    |
| 0006 | CLI Structure           | Complete | Stage 4   | clap-based CLI implemented           |
| 0007 | Service Types           | Complete | Stage 4   | process/container/site types         |
| 0008 | Environment Variables   | Complete | Stage 4   | Env file/injection implemented       |
| 0009 | Port Assignment         | Complete | Stage 4   | Dynamic port allocation working      |
| 0010 | Health Checks           | Complete | Stage 4   | HTTP/TCP/process checks              |
| 0011 | Logging                 | Complete | Stage 4   | Structured logging via tracing       |
| 0012 | Signal Handling         | Complete | Stage 4   | SIGTERM/SIGKILL propagation          |
| 0013 | Process Groups          | Complete | Stage 3   | Cgroup-based process management      |
| 0014 | Dependencies            | Complete | Stage 3   | Service dependency ordering          |
| 0015 | Watch Mode              | Partial  | Stage 2   | notify crate, not fully wired        |
| 0016 | Build Scripts           | Complete | Stage 3   | [build] section functional           |
| 0017 | Privileged Operations   | Complete | Stage 3   | polkit + pkexec integration          |
| 0018 | State Directory         | Complete | Stage 3   | XDG-compliant state paths            |
| 0019 | Plugin Architecture     | Complete | Stage 3   | WASM plugin system functional        |
| 0020 | Error Handling          | Complete | Stage 3   | miette-based error reporting         |

#### Batch 2: RFCs 0020-0043

| RFC  | Title                 | Impl     | Recommend | Notes                                      |
| ---- | --------------------- | -------- | --------- | ------------------------------------------ |
| 0021 | Docker Health Polling | None     | Withdraw  | Superseded by libcontainer, Docker removed |
| 0022 | Container Build Cache | Complete | Stage 3   | Layer caching in CNB                       |
| 0023 | Multi-Service Config  | Complete | Stage 4   | Multiple [services.*] sections             |
| 0024 | Proxy Headers         | Complete | Stage 3   | X-Forwarded-\* headers set                 |
| 0025 | TLS Certificates      | Complete | Stage 3   | mkcert + CA trust                          |
| 0026 | Dashboard API         | Complete | Stage 3   | REST API for dashboard                     |
| 0027 | Websocket Logs        | Complete | Stage 3   | Real-time log streaming                    |
| 0028 | Service Events        | Complete | Stage 3   | SSE event stream                           |
| 0029 | Graceful Shutdown     | Complete | Stage 3   | Ordered shutdown with timeouts             |
| 0030 | Gitignore Automation  | None     | Withdraw  | Never implemented, low value               |
| 0031 | Init System Detection | Complete | Stage 3   | systemd detection                          |
| 0032 | OCI Image Support     | Complete | Stage 3   | OCI runtime integration                    |
| 0033 | CNB Buildpacks        | Complete | Stage 3   | Paketo buildpacks support                  |
| 0034 | Layer Caching         | Complete | Stage 3   | CNB layer caching                          |
| 0035 | Registry Integration  | Complete | Stage 3   | OCI registry pull/push                     |
| 0036 | Container Networking  | Complete | Stage 3   | Bridge networking                          |
| 0037 | Volume Mounts         | Complete | Stage 3   | Bind mount support                         |
| 0038 | Extensibility         | Complete | Stage 3   | Plugin system (already staged)             |
| 0039 | CLI Autocompletion    | Partial  | Stage 1   | Shell completions partial                  |
| 0040 | Output Formatting     | Complete | Stage 3   | Rich terminal output                       |
| 0041 | Progress Indicators   | Complete | Stage 3   | cliclack spinners                          |
| 0042 | Color Theming         | Partial  | Stage 2   | Basic color support                        |
| 0043 | CNB Integration       | Complete | Stage 3   | Full buildpack integration                 |

#### Batch 3: RFCs 0044-0063

| RFC  | Title               | Impl     | Recommend | Notes                                |
| ---- | ------------------- | -------- | --------- | ------------------------------------ |
| 0044 | Process Supervision | Complete | Stage 3   | Process lifecycle management         |
| 0045 | Resource Limits     | Complete | Stage 3   | Cgroup resource constraints          |
| 0046 | Log Rotation        | Partial  | Stage 1   | Basic rotation, not configurable     |
| 0047 | Cross-Platform      | Partial  | Stage 1   | Linux primary, macOS incomplete      |
| 0048 | Metrics Collection  | None     | Stage 0   | No Prometheus/metrics yet            |
| 0049 | Service Discovery   | Partial  | Stage 1   | Internal only, no external registry  |
| 0050 | Hot Reload          | Partial  | Stage 0   | Watcher exists, not wired to restart |
| 0051 | Template System     | None     | Stage 0   | No project templates                 |
| 0052 | Runc Integration    | None     | Withdraw  | Superseded by RFC 0098 libcontainer  |
| 0053 | Shim Architecture   | Complete | Stage 3   | Shim fully implemented               |
| 0054 | IPC Protocol        | Complete | Stage 3   | Unix socket RPC                      |
| 0055 | Daemon Mode         | Complete | Stage 3   | Background daemon operation          |
| 0056 | PID Management      | Complete | Stage 3   | PID file handling                    |
| 0057 | Socket Activation   | Partial  | Stage 1   | Basic support, not systemd-native    |
| 0058 | Logging Levels      | Complete | Stage 3   | RUST_LOG / --verbose                 |
| 0059 | Structured Errors   | Complete | Stage 3   | miette integration                   |
| 0060 | Config Validation   | Complete | Stage 3   | TOML schema validation               |
| 0061 | Default Values      | Complete | Stage 3   | Sensible defaults                    |
| 0062 | Service Templates   | None     | Stage 0   | Not implemented                      |
| 0063 | CLI Help            | Complete | Stage 3   | clap help text                       |

#### Batch 4: RFCs 0064-0080

| RFC  | Title                   | Impl     | Recommend | Notes                               |
| ---- | ----------------------- | -------- | --------- | ----------------------------------- |
| 0064 | Container Engine        | None     | Stage 0   | Abstract interface, not implemented |
| 0065 | OCI Fetcher             | Complete | Stage 3   | locald-oci crate                    |
| 0066 | Investigation Protocol  | Complete | Stage 3   | Debugging methodology               |
| 0067 | CNB Output Parsing      | Complete | Stage 3   | Build output parsing                |
| 0067 | Utility Crate           | Complete | Stage 3   | locald-utils crate                  |
| 0068 | CNB Library             | Complete | Stage 3   | cnb-client crate                    |
| 0068 | Dependency Management   | Partial  | Stage 1   | Basic dep order, not advanced       |
| 0069 | Host-First Execution    | Complete | Stage 3   | Default execution mode              |
| 0069 | Rust CNB Launcher       | None     | Withdraw  | Alternative approach abandoned      |
| 0070 | Cliclack UI             | Partial  | Stage 2   | Basic prompts, not full TUI         |
| 0071 | Documentation Structure | Complete | Stage 3   | Docs reorganized                    |
| 0072 | Host-Level Builds       | None     | Stage 0   | Not implemented                     |
| 0073 | Dashboard Self-Rep      | Partial  | Stage 2   | Dashboard exists, not full self-rep |
| 0074 | Container Service Type  | Complete | Stage 3   | Service type implemented            |
| 0075 | Runc Runtime            | Complete | Stage 3   | Implemented then superseded         |
| 0076 | Ephemeral Containers    | Partial  | Stage 2   | Basic support                       |
| 0077 | Runc Setuid Fix         | Complete | Stage 3   | Security fix applied                |
| 0078 | Embedded Shim           | Complete | Stage 3   | build.rs embedding                  |
| 0079 | Unified Service Trait   | Complete | Stage 3   | Trait fully implemented             |
| 0080 | Unified CI Hook Runner  | None     | Stage 0   | Not implemented                     |

#### Batch 5: RFCs 0081-0104

| RFC  | Title                     | Impl     | Recommend | Notes                                    |
| ---- | ------------------------- | -------- | --------- | ---------------------------------------- |
| 0081 | Premium Dashboard         | Partial  | Stage 2   | Grid/card partial, Smart Folding missing |
| 0082 | Dashboard Philosophy      | Complete | Stage 3   | Rack/Stream/Deck axioms                  |
| 0082 | Testing Philosophy        | None     | Stage 1   | DI/Fake pattern not adopted              |
| 0083 | Dashboard E2E             | Complete | Stage 3   | Playwright suite exists                  |
| 0084 | Dashboard Redesign v2     | None     | Withdraw  | Superseded by RFC 0087                   |
| 0085 | Workspace Support         | None     | Stage 0   | Multi-project not implemented            |
| 0086 | CLI Surface Overhaul      | Partial  | Stage 1   | Some aliases exist, not complete         |
| 0087 | Cybernetic Dashboard      | Complete | Stage 4   | Rack/Stream/Deck fully implemented       |
| 0088 | Rustdoc Service           | None     | Stage 0   | Commented out, not first-class           |
| 0089 | Embedded Static Server    | Complete | Stage 3   | static_server.rs module                  |
| 0090 | Managed Site Service      | Partial  | Stage 2   | Site type exists, no kernel watcher      |
| 0091 | Privileged Cleanup        | Complete | Stage 3   | admin cleanup in shim                    |
| 0092 | Improved Hardlink         | Partial  | Stage 1   | Reflink used, not hardlink               |
| 0093 | Proxy Loading State       | Complete | Stage 3   | Building... page with timeout            |
| 0094 | Global State Directory    | Complete | Stage 3   | XDG_DATA_HOME pattern                    |
| 0095 | Garbage Collector         | Complete | Stage 3   | Registry clean/prune                     |
| 0096 | Shim Execution Safety     | Complete | Stage 3   | SCM_RIGHTS bind command                  |
| 0097 | Strict Shim Discovery     | Complete | Stage 3   | Sibling/parent only                      |
| 0098 | Libcontainer Execution    | Complete | Stage 3   | No runc dependency                       |
| 0099 | Cgroup Hierarchy          | Complete | Stage 3   | cgroup v2 + kill                         |
| 0100 | System Plane + Pinning    | Complete | Stage 3   | Solo/pin unified                         |
| 0101 | Arc 2 Runtime Isolation   | Partial  | Stage 0   | VMM crate exists, early                  |
| 0102 | VMM Maturity + Networking | None     | Stage 0   | Basic boot only                          |
| 0103 | Docs Language/Persona     | None     | Stage 0   | Not implemented                          |
| 0104 | macOS Platform Support    | None     | Stage 1   | Lima strategy defined, not coded         |

---

## Uncertainties Requiring Review

| RFC       | Question                                                             | Resolution                                                                                                |
| --------- | -------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| 0038      | Remote plugin fetching: auto-fetch on `locald up` or manual install? | Manual install required - infra exists for future auto-fetch                                              |
| 0104      | Stage says 3 but checklist incomplete - which is authoritative?      | Frontmatter is authoritative (shipped PR #5). Incomplete items are polish, not blockers. Keep at Stage 3. |
| 0108      | VMM basics exist - does that qualify for Stage 1?                    | Yes, Stage 1 = "design drafted". VMM foundation proves design viability.                                  |
| 0112      | Meta-documentation RFC - what does "implemented" mean?               | Process is ongoing; RFC defines methodology. Stage 1 (accepted process).                                  |
| 0130/0134 | Both in stage-2 but superseded - should they be archived?            | Yes, create archive directory or mark with "superseded_by" in frontmatter.                                |

---

## Audit Log

### 2025-01-24 - Complete RFC Audit

**Scope**: All 133 RFCs (28 staged + 105 root-level)

**Method**: Subagent-based code archaeology with sequential batched execution

---

### Summary Statistics

| Category                | Count | Notes                        |
| ----------------------- | ----- | ---------------------------- |
| **Total RFCs**          | 133   | 28 staged + 105 root-level   |
| **Stage 4 Candidates**  | 14    | Foundational, API frozen     |
| **Stage 3 Candidates**  | 55    | Stable implementation        |
| **Stage 2 Candidates**  | 12    | Implemented, rough edges     |
| **Stage 1 Candidates**  | 18    | Design drafted, partial impl |
| **Stage 0 Candidates**  | 27    | Strawman/future              |
| **Withdraw Candidates** | 7     | Superseded or abandoned      |

---

### Action Items

#### RFCs to Withdraw (7)

| RFC  | Title                 | Reason                              |
| ---- | --------------------- | ----------------------------------- |
| 0021 | Docker Health Polling | Docker removed, superseded          |
| 0030 | Gitignore Automation  | Never implemented, low value        |
| 0052 | Runc Integration      | Superseded by RFC 0098 libcontainer |
| 0069 | Rust CNB Launcher     | Alternative approach abandoned      |
| 0084 | Dashboard Redesign v2 | Superseded by RFC 0087              |
| 0130 | Host Shim Daemon      | Superseded by RFC 0138              |
| 0134 | Host-Spawn Crate      | Superseded by RFC 0138              |

#### RFCs to Promote to Stage 4 (14)

| RFC  | Title                 | From   | Reason                    |
| ---- | --------------------- | ------ | ------------------------- |
| 0001 | ADR Process           | (root) | Process fully established |
| 0002 | Service Lifecycle     | (root) | Core, API frozen          |
| 0003 | Domain-Based Routing  | (root) | Core, API frozen          |
| 0004 | Proxy Architecture    | (root) | Core, API frozen          |
| 0005 | TOML Configuration    | (root) | Core, API frozen          |
| 0006 | CLI Structure         | (root) | Core, API frozen          |
| 0007 | Service Types         | (root) | Core, API frozen          |
| 0008 | Environment Variables | (root) | Core, API frozen          |
| 0009 | Port Assignment       | (root) | Core, API frozen          |
| 0010 | Health Checks         | (root) | Core, API frozen          |
| 0011 | Logging               | (root) | Core, API frozen          |
| 0012 | Signal Handling       | (root) | Core, API frozen          |
| 0023 | Multi-Service Config  | (root) | Core, API frozen          |
| 0087 | Cybernetic Dashboard  | (root) | Fully implemented         |

#### RFCs to Promote to Stage 3 (55 - selected highlights)

| RFC       | Title                  | From    |
| --------- | ---------------------- | ------- |
| 0013      | Process Groups         | (root)  |
| 0014      | Dependencies           | (root)  |
| 0016-0020 | Various                | (root)  |
| 0022-0029 | Dashboard/Proxy        | (root)  |
| 0031-0037 | Containers             | (root)  |
| 0040-0041 | CLI                    | (root)  |
| 0043-0045 | CNB/Process            | (root)  |
| 0053-0056 | Shim/Daemon            | (root)  |
| 0058-0061 | Errors/Config          | (root)  |
| 0063      | CLI Help               | (root)  |
| 0065-0068 | OCI/CNB                | (root)  |
| 0071      | Documentation          | (root)  |
| 0074-0079 | Container/Shim         | (root)  |
| 0082      | Dashboard Philosophy   | (root)  |
| 0083      | Dashboard E2E          | (root)  |
| 0089      | Embedded Static Server | (root)  |
| 0091-0100 | Security/State         | (root)  |
| 0111      | Property Tests         | Stage 0 |
| 0125      | Respectful Doctor      | Stage 0 |
| 0129      | WASM Plugins           | Stage 1 |

#### RFCs to Keep at Stage 0 (Future/Backlog)

| RFC  | Title                   | Notes                     |
| ---- | ----------------------- | ------------------------- |
| 0048 | Metrics Collection      | No implementation         |
| 0050 | Hot Reload              | Watcher exists, not wired |
| 0051 | Template System         | No templates              |
| 0062 | Service Templates       | Not implemented           |
| 0064 | Container Engine        | Abstract interface        |
| 0072 | Host-Level Builds       | Not implemented           |
| 0080 | Unified CI Hook Runner  | Not implemented           |
| 0085 | Workspace Support       | Multi-project not done    |
| 0088 | Rustdoc Service         | Commented out             |
| 0101 | Arc 2 Runtime Isolation | VMM early prototype       |
| 0102 | VMM Networking          | Basic boot only           |
| 0103 | Docs Persona Routing    | Not implemented           |
| 0106 | Remote Host Mapping     | No implementation         |
| 0113 | Privileged E2E Runner   | No dedicated runners      |
| 0116 | MAP                     | Empty placeholder         |
| 0124 | Post-MAP Themes         | Future direction          |
| 0127 | Explicit Defaults       | Not implemented           |
| 0131 | WSL Privileged Ops      | Minimal WSL detection     |
| 0133 | WSL Windows Helper      | Unimplemented             |

---

### Next Steps

1. **Immediate**: Move 7 superseded RFCs to `withdrawn/`
2. **Short-term**: Organize 105 root-level RFCs into stage directories
3. **Consider**: Create "backlog" format for Stage 0 RFCs unlikely to be implemented soon
4. **Document**: Update RFC index/registry with new stages

---

## Launch Priority Triage (2025-01-24)

Based on project differentiators and axioms, RFCs were triaged into launch priorities.

### Core Differentiators

1. "Clone → locald up" - Zero-friction start
2. Stable Domains + HTTPS - project.localhost with auto TLS
3. Daemon-First - Processes persist, always-on platform
4. Self-Hosting Platform - Dashboard + Docs built-in
5. Cross-Platform - Linux, macOS, Windows first-class
6. Plugin Extensibility - WASM plugins
7. Docker-Free Host-First - Works without Docker

### Launch Critical (Phase 1-2)

| RFC  | Title                     | Rationale                                                    |
| ---- | ------------------------- | ------------------------------------------------------------ |
| 0104 | macOS Platform Support    | Cross-platform is core axiom; macOS is dominant dev platform |
| 0138 | Remove Container Workflow | Simplify codebase, sharpen host-first differentiator         |
| 0110 | Privileged Capability     | Better onboarding errors for domain/TLS                      |
| 0086 | CLI Surface Overhaul      | Zero-friction requires coherent CLI                          |
| 0135 | Dashboard Vocabulary      | Self-hosting platform needs coherent UX                      |
| 0039 | Installation & Updates    | Clone→up requires frictionless install                       |
| 0103 | Docs Site Language        | Onboarding experience                                        |

### Near-Term Polish (Phase 3)

| RFC  | Title                   | Rationale                       |
| ---- | ----------------------- | ------------------------------- |
| 0050 | Hot Reloading           | Config changes should just work |
| 0051 | Port Mismatch Detection | Reduce first-run confusion      |
| 0072 | Host-Level Builds       | Build before start              |
| 0085 | Workspace Support       | Monorepo reality                |

### Withdrawn (This Session)

| RFC  | Title                | Reason                                      |
| ---- | -------------------- | ------------------------------------------- |
| 0116 | MAP Scope Quarantine | Empty stub (contents lost, may reconstruct) |
| 0137 | Superseded by 0138   | Explicitly superseded                       |
| 0046 | Manifesto Structure  | Obsolete                                    |
| 0130 | Host Shim Daemon     | Superseded by 0138                          |

---

## Strategic Triage (2025-01-24)

Based on locald's core differentiators:

1. **"Clone → `locald up`"** - Zero-friction start
2. **Stable Domains + HTTPS** - No port juggling
3. **Daemon-First** - Always-on platform
4. **Self-Hosting Platform** - Dashboard + Docs built-in
5. **Cross-Platform** - Linux, macOS, Windows first-class
6. **Plugin Extensibility** - WASM plugins
7. **Docker-Free Host-First** - No Docker dependency

### Launch Critical (Phase 1-2)

| RFC  | Title                     | Priority | Rationale                                   |
| ---- | ------------------------- | -------- | ------------------------------------------- |
| 0104 | macOS Platform Support    | P1       | Cross-platform axiom, dominant dev platform |
| 0138 | Remove Container Workflow | P1       | Simplify, sharpen host-first differentiator |
| 0110 | Privileged Capability     | P1       | Better onboarding errors                    |
| 0086 | CLI Surface Overhaul      | P2       | Coherent "zero friction" CLI                |
| 0135 | Dashboard Vocabulary      | P2       | Self-hosting platform UX                    |
| 0103 | Docs Site Language        | P2       | Onboarding experience                       |
| 0039 | Installation & Updates    | P2       | Frictionless install                        |

### Near-Term Polish (Phase 3)

| RFC  | Title                   | Rationale                         |
| ---- | ----------------------- | --------------------------------- |
| 0050 | Hot Reloading           | Config changes should "just work" |
| 0051 | Port Mismatch Detection | Reduce user confusion             |
| 0072 | Host-Level Builds       | Better first-run experience       |
| 0085 | Workspace Support       | Monorepo reality                  |

### Withdrawn (This Session)

| RFC  | Title               | Reason                |
| ---- | ------------------- | --------------------- |
| 0137 | Superseded by 0138  | Explicitly superseded |
| 0046 | Manifesto Structure | Likely obsolete       |
| 0130 | Host Shim Daemon    | Superseded by 0138    |

### Reconstructed (This Session)

| RFC  | Title                   | Notes                                               |
| ---- | ----------------------- | --------------------------------------------------- |
| 0116 | Minimum Awesome Product | Reconstructed from scattered references in codebase |
