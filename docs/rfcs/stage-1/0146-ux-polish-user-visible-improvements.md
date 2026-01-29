---
title: UX Polish: User-Visible Improvements
stage: 1
feature: ux
exo:
    tool: exo rfc create
    protocol: 1
---

# RFC 0146: UX Polish: User-Visible Improvements

## Summary

Address user-visible inconsistencies and gaps identified during a comprehensive UX audit. The goal is to make locald feel polished and "just work" before expanding the feature set.

## Motivation

A recon audit identified several areas where the user experience is uneven:

1. **Managed services don't expose connection info** - Users can't easily discover DATABASE_URL or connection strings
2. **Documentation promises features that aren't implemented** - Live reload for static sites
3. **Dashboard shows incorrect service types** - Heuristic-based labeling instead of actual config
4. **CLI surface is inconsistent** - Some commands use miette errors, others use eprintln
5. **CLI documentation is incomplete** - Many commands undocumented

These gaps create friction and undermine the "clone → locald up" experience.

## Design

### P0: Critical UX Gaps

#### 1. Managed Postgres Connection String UX

**Problem**: Postgres services don't expose `DATABASE_URL` to dependent services automatically.

**Solution**:
- Add `connection_url` field to service status for data services
- Auto-inject `DATABASE_URL` env var for services that depend on Postgres
- Surface connection info in `locald status` and dashboard

#### 2. Static Site Live Reload

**Problem**: Docs promise live reload but it's not implemented.

**Solution** (choose one):
- **Option A**: Implement WebSocket-based reload signaling in the toolbar
- **Option B**: Remove "live reload" claims from documentation until implemented

### P1: Notable Inconsistencies

#### 3. Dashboard Service Type Accuracy

**Problem**: Dashboard infers service type from name/port heuristics, not actual config.

**Solution**:
- Add `service_type` field to status API response
- Update dashboard to display actual type from config

#### 4. CLI Error Consistency

**Problem**: Error handling varies between commands (miette vs eprintln).

**Solution**:
- Standardize all CLI errors through `CliError` type
- Ensure consistent formatting and help text

#### 5. CLI Documentation Coverage

**Problem**: Many commands (`plugin`, `admin`, `ai`, `serve`) are undocumented.

**Solution**:
- Audit full CLI surface against docs
- Either document commands or mark them as internal/experimental

#### 6. `locald add postgres` Missing

**Problem**: Can add `exec`, `site`, `container` services but not `postgres`.

**Solution**:
- Add `postgres` variant to `locald add` command

### P2: Nice-to-haves (Out of Scope for Initial Phase)

- Dashboard toast/banner system for errors
- `locald serve` port allocation improvements

## Implementation Plan

1. **Service status API enhancements** - Add `service_type` and `connection_url` fields
2. **Postgres env injection** - Auto-inject DATABASE_URL for postgres dependencies  
3. **Dashboard updates** - Display accurate service types and connection info
4. **CLI error standardization** - Migrate all commands to CliError
5. **Documentation sync** - Update CLI docs to match reality
6. **`locald add postgres`** - Implement missing command variant

## Alternatives Considered

- **Do nothing**: Leave inconsistencies as-is. Rejected because UX quality is a core value.
- **Full rewrite**: Redesign CLI/dashboard from scratch. Rejected as too disruptive.

## References

- Recon audit findings (2026-01-28)
- AGENTS.md UX principles
