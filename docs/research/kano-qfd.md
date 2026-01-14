# Kano-QFD Analysis: locald Extensibility & Plugin Ecosystem

> **Date**: January 2026  
> **Phase Context**: Completing Phase 29.1 (Plugin Mechanism)  
> **Purpose**: Strategic prioritization for Phase 29.2+ using Kano Model and Quality Function Deployment

---

## Executive Summary

locald has matured from a process manager into a comprehensive local development platform. This analysis evaluates the product's feature set through the **Kano Model** lens (customer satisfaction drivers) and **QFD** (Quality Function Deployment) for technical prioritization.

**Key Finding**: The plugin mechanism (29.1) is now complete, enabling extensibility. The next highest-leverage work is **Phase 29.2 (Packaging)** — without it, the plugin system's value cannot be distributed to teams.

---

## 1. Kano Model Feature Categorization

### Legend

| Category | Description | Impact on Satisfaction |
|----------|-------------|----------------------|
| **Must-Be** | Expected features; absence causes dissatisfaction | Prevents failure |
| **One-Dimensional** | Linear relationship: more = better | Drives adoption |
| **Attractive** | Delighters; unexpected features that create loyalty | Creates advocates |
| **Indifferent** | No impact on satisfaction | Low priority |
| **Reverse** | Features some users actively dislike | Avoid or make optional |

### Feature Matrix

| Feature | Category | Status | Notes |
|---------|----------|--------|-------|
| **Core Runtime** | | | |
| `locald up` (start services) | Must-Be | ✅ Complete | Foundational |
| Process supervision | Must-Be | ✅ Complete | Restarts, signals |
| `*.localhost` routing | Must-Be | ✅ Complete | Semantic DNS |
| Dynamic port allocation | Must-Be | ✅ Complete | No manual ports |
| Log streaming | Must-Be | ✅ Complete | CLI + Dashboard |
| **Configuration** | | | |
| `locald.toml` per-project | Must-Be | ✅ Complete | Source of truth |
| Cascading config | One-Dimensional | ✅ Complete | Global → Project |
| Environment injection | One-Dimensional | ✅ Complete | DATABASE_URL etc. |
| **Developer Experience** | | | |
| Web dashboard | One-Dimensional | ✅ Complete | Svelte 5 + xterm.js |
| `locald try` (ad-hoc) | Attractive | ✅ Complete | Experiment safely |
| `locald doctor` | Attractive | ✅ Complete | Self-diagnosis |
| Hot reload on config change | Attractive | ✅ Complete | Auto-restart |
| **Builtin Services** | | | |
| Postgres (embedded) | One-Dimensional | ✅ Complete | Zero-config DB |
| Docker containers | One-Dimensional | ✅ Complete | `image:` config |
| **Security** | | | |
| Privilege separation (shim) | Must-Be | ✅ Complete | Least privilege |
| Sandbox mode | Attractive | ✅ Complete | CI-friendly |
| **Extensibility** | | | |
| Plugin mechanism (WASM) | Attractive | ✅ Complete | **Phase 29.1** |
| `locald package` | Attractive | 🔲 Pending | **Phase 29.2** |
| Flavored distributions | Attractive | 🔲 Pending | **Phase 29.3** |
| **Installation** | | | |
| Single binary | Must-Be | ✅ Complete | Ship one file |
| `locald selfupgrade` | One-Dimensional | 🔲 Pending | Phase 30 |
| Auto-updates | Indifferent | 🔲 Pending | Optional |
| **Advanced** | | | |
| VMM (microVMs) | Attractive | 🔲 Pending | Phase 102 |
| Interactive PTY (web) | Indifferent | 🔲 Pending | Nice-to-have |

---

## 2. QFD Correlation Matrix

### Customer Needs → Technical Requirements

| Customer Need | Plugin Runtime | Package Format | Distribution Channel | Self-Update |
|--------------|:--------------:|:--------------:|:--------------------:|:-----------:|
| "My team uses our own services" | ★★★★★ | ★★★★★ | ★★★★☆ | ☆☆☆☆☆ |
| "I want to share my setup" | ★★☆☆☆ | ★★★★★ | ★★★★★ | ☆☆☆☆☆ |
| "It should just work" | ★★★☆☆ | ★★★★★ | ★★★★★ | ★★★★☆ |
| "Stay up to date easily" | ☆☆☆☆☆ | ☆☆☆☆☆ | ★★★☆☆ | ★★★★★ |
| "Extend without forking" | ★★★★★ | ★★★★☆ | ★★★☆☆ | ☆☆☆☆☆ |

### Priority Scores

| Technical Requirement | Priority Score | Implementation Phase |
|----------------------|:--------------:|:-------------------:|
| Plugin Runtime (WASM) | ★★★★★ | ✅ 29.1 Complete |
| Package Format | ★★★★★ | 🎯 29.2 (Next) |
| Distribution Bundles | ★★★★☆ | 29.3 |
| Self-Upgrade | ★★★☆☆ | 30 |

---

## 3. Strategic Recommendations

### Immediate Priority: Phase 29.2 (Packaging)

**Rationale**: The plugin mechanism only delivers value when plugins can be *shared*. Phase 29.2 is the keystone:

```
Plugin (29.1) → Package (29.2) → Distribution (29.3)
     ✅              🎯               ↓
    "I can build"  "I can share"   "Teams can adopt"
```

**Scope for 29.2**:
1. Define `.locald-package` archive format (WASM + config + metadata)
2. Implement `locald package /path` to bundle plugins + locald.toml
3. Implement `locald package install <archive>` to unpack and register
4. Document packaging workflow for plugin authors

### Fast-Follow: "Retire Docker" Plugin

A high-impact **Attractive** feature would be a plugin that generates service plans for common infrastructure:

- **redis-plugin**: `type = "redis"` → spawns Redis via container or binary
- **postgres-plugin**: Migrate embedded Postgres to plugin (dogfooding)
- **mysql-plugin**: New capability via plugin

This validates the entire plugin pipeline with real users and creates an ecosystem flywheel.

### Deferred: Phase 102 (VMM Networking)

Per Kano, VMM features are **Attractive** but not yet demanded by the core user base. The foundation (virtio-block) is in place; networking can wait until the plugin ecosystem is proven.

---

## 4. Risk Analysis

| Risk | Mitigation |
|------|------------|
| Package format lock-in | Use standard formats (tar.gz + TOML manifest) |
| Plugin API instability | Mark as `experimental` until 30.x |
| Team adoption friction | Provide `locald init --from-package` |

---

## 5. Conclusion

locald's core is solid. The strategic focus should be:

1. ✅ **Phase 29.1** (Plugin Runtime) — Complete
2. 🎯 **Phase 29.2** (Packaging) — **Highest leverage**
3. ⏳ **Phase 29.3** (Distributions) — Enables team adoption
4. ⏳ **Phase 30** (Installation & Updates) — Quality of life

The Kano analysis confirms: all **Must-Be** features are complete. Further investment should target **Attractive** features that create advocates and ecosystem growth — starting with making plugins distributable.

---

## References

- [RFC 0028: Plugin System](../rfcs/stage-3/0028-plugin-system.md)
- [RFC 0129: Plugin Contract](../rfcs/stage-2/0129-plugin-contract.md)
- [Phase 29 Implementation Plan](../agent-context/current/implementation-plan.toml)
