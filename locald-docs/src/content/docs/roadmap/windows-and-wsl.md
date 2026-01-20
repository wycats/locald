---
title: Windows & WSL
description: What works today, what’s coming, and what “self-contained WSL” means.
---

> **Roadmap**: Windows-host integration is not yet implemented.

## Summary

`locald` targets Linux and macOS first. Windows is **coming**, and WSL is supported in a **self-contained** mode.

## Self-contained WSL (in-scope)

Self-contained WSL means:

- `locald` runs inside WSL2.
- Your browser is also inside WSL (for example via WSLg).
- Domains and HTTPS work **within WSL’s environment**.

This should work because it matches the normal Linux model: the DNS/cert trust changes apply to the OS that is running the browser.

## Windows browsers (coming)

Getting `https://*.localhost` to work in **Windows browsers** while `locald` runs inside WSL2 likely requires a Windows-side helper that can:

- install trust roots into the Windows trust store,
- coordinate hosts / name resolution as needed,
- and potentially manage port forwarding.

This is tracked as a proposal in the RFCs (see RFC 0133).

## What we should say in the launch docs

- “Windows support is coming.”
- “WSL works best as a self-contained environment (browser inside WSLg).”
- “Windows browsers are not yet supported end-to-end.”
