---
title: "Docs Site: Persona Routing + Jargon Budget"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Documentation
---

# RFC 0103: Docs Site: Persona Routing + Jargon Budget

## 1. Summary

Update the public docs site (`locald-docs/`) to be explicitly **persona-routed** and **jargon-light by default**.

- The landing page should route users by intent (run an app, tweak config, contribute).
- “Getting started” pages should read like a tutorial for a developer who just wants results.
- Project-specific terms (e.g. Rack/Stream/Deck, System Plane) remain, but are introduced as **UI names** with plain-language explanations.

## 2. Motivation

The docs currently leak internal vocabulary from RFCs and vision documents into the first-run experience.
That has two costs:

- **Onboarding friction**: users have to learn a “dialect” before they can do anything.
- **Navigation fatigue**: users can’t easily tell which pages are for them.

We want the docs site to work like a product manual:

- plain language first,
- deep details available when needed,
- terminology consistent with the UI.

## 3. Detailed Design

### Terminology

- **Persona routing**: the landing page routes readers to the next page based on intent.
- **Jargon budget**: a page can introduce a small number of new terms; everything else should be plain language or linked.
- **UI name**: a term that appears in the interface (e.g. “Rack”). These are allowed, but must be explained in one sentence.

### User Experience (UX)

**Landing page**

- Primary CTA: “Run my first app” → the first tutorial.
- Secondary CTA: “Configure locald” → reference entry point.
- Cards for:
  - App Builder
  - System Tweaker
  - Contributor
  - Concepts (optional)

**Getting started**

- Replace “vision/RFC voice” with action-oriented tutorial voice.
- Keep the concrete outcomes front-and-center:
  - port allocation via `$PORT`
  - supervised processes
  - stable `*.localhost` URLs
  - dashboard at `http://locald.localhost`

**Concepts (dashboard model)**

- Keep Rack/Stream/Deck/System Plane as the canonical UI labels.
- Use plain-language explanations and “what you use it for” framing.

### Architecture

No runtime changes. This is documentation-only:

- `locald-docs/` content pages
- Starlight site config for navigation and canonical site URL

### Implementation Details

- Prefer short sections and concrete examples.
- Avoid capitalizing philosophical nouns (“Platform”, “Rigour”, “Production Parity”) unless they are UI labels.
- When a project term must be used (e.g. “System Plane”), immediately define it in plain language.

## 4. Implementation Plan (Stage 2)

- [ ] Add persona-routing homepage copy in `locald-docs/src/content/docs/index.mdx`.
- [ ] Rewrite `locald-docs/src/content/docs/getting-started/index.mdx` to reduce jargon while preserving correctness.
- [ ] Rewrite `locald-docs/src/content/docs/concepts/workspace.md` to introduce Rack/Stream/Deck/System Plane as UI names.
- [ ] Set `site: "http://docs.localhost"` in `locald-docs/astro.config.mjs` to remove sitemap warnings and establish canonical URLs.
- [ ] Verify with `pnpm -C locald-docs build`.

## 5. Context Updates (Stage 3)

This RFC changes documentation, not product behavior.

- [ ] Add/Update contributor guidance about docs layering and tone (candidate: `docs/manual/` or `locald-docs/src/content/docs/internals/development.md`).

## 6. Drawbacks

- Some readers may prefer the “vision-first” style; jargon-light docs may feel less “philosophical”.
- Requires ongoing editorial discipline to prevent regression.

## 6. Alternatives

- Keep current voice and rely on search.
- Create a separate “marketing” site and keep docs fully technical.

## 7. Unresolved Questions

- Do we want an explicit “Glossary” page for UI terms and recurring concepts?
- Should “Concepts” be emphasized on the homepage, or kept strictly optional?

## 8. Future Possibilities

- Add linting or CI checks for banned phrases / overuse of internal terms on top-level tutorial pages.
- Add a short glossary section to high-traffic pages (“Terms used in the UI”).
