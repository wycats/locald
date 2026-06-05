# Style Guide Pages — Handoff

This note orients anyone (person or agent) picking up the locald docs
style-guide work. It explains what was built, the decisions behind it, the
non-obvious traps, and where to go next. It is written to be read, not skimmed
for keywords — if you read it top to bottom you'll have the context the original
author had.

## What this work is

The locald docs site (`locald-docs/`, Astro + Starlight) now has a set of
**visual style-guide pages** under `src/content/docs/style-guide/`. They are not
ordinary prose docs — each page is a faithful rendering of a designed "poster,"
turned into real, responsive HTML/CSS using locald's own design tokens. The
point is that the style guide *is itself* an example of the style it documents.

The five pages, in order:

1. `overview.mdx` (01) — brand principle, core vocabulary, system topology.
2. `brand-foundations.mdx` (02) — color, type, the brand mark.
3. `product-surface-grammar.mdx` (03) — the Rack / Stream / Deck / System Plane
   surface vocabulary and the nouns that live in each.
4. `public-site-direction.mdx` (04) — the marketing-site direction: hero,
   dashboard mock, URL topology, principles, CTA.
5. `components-interface-patterns.mdx` — the concrete interface patterns
   (service rows, URL pills, status treatments).

Supporting code:

- `locald-docs/src/components/{brand,diagrams,grammar,direction,styleguide}/` —
  the Astro components each page composes.
- `locald-docs/src/styles/locald-tokens.css` — the design tokens (color, type,
  spacing) as locald-namespaced CSS custom properties.
- `locald-docs/src/styles/locald-components.css` — the MDX layout utilities and
  per-page section styles.

All of this lives on the `docs/style-guide-pages` branch
([PR #104](https://github.com/wycats/locald/pull/104)). It does not touch any
Rust code and has no file overlap with the tray/quiet-up work (PR #105).

## The source of truth, and where it is not

The pages were built from **design posters**, one per surface. The authoritative
references are:

- The poster images themselves.
- `guide-2-foundations.png` specifically is the **authoritative source for color
  and type** — when a token value and a poster disagree elsewhere, foundations
  wins for color/type.
- `styleguide.md` — a written specification that accompanies the posters.

**These files are intentionally not in the repository.** They live in
`.private/style/` locally, which is gitignored, and they were transferred
between machines out-of-band (via `croc`). So: if you are picking this up on a
machine that doesn't have them, you will not find them in the tree, and you
should not try to reconstruct the design from memory or from the rendered pages
alone. Ask for the posters to be transferred. Building a new surface "from
memory" is the one thing most likely to drift from the intended design — re-view
the relevant poster for each surface every time.

## Decisions worth knowing

- **Domains are always `*.localhost`.** The posters sometimes show `.local` or
  `.test`; ignore that. The written guide rule — and locald's actual product
  behavior — is `*.localhost`, and that is what the pages use (e.g.
  `api.localhost`, `web.localhost`). This is a deliberate divergence from the
  posters, not an oversight.
- **Tokens are locald-namespaced.** Custom properties are prefixed so they don't
  collide with Starlight's own variables. Reach for an existing token before
  introducing a new value.
- **The pages are responsive renderings, not pixel-copies.** The goal is
  faithfulness to the design's *intent* and system, rendered as real components
  that reflow — not a screenshot. When a poster's fixed layout fights responsive
  behavior, preserve the intent.

## Traps (these will bite you)

- **Starlight's `* + *` margin rule.** Starlight injects
  `margin-top: 16px` onto adjacent siblings inside markdown content via a
  selector like `.sl-markdown-content :not(...) + :not(...)`. When you embed a
  CSS grid in MDX, this lands `margin-top` on the grid's *children* and breaks
  the layout. The fix, applied repeatedly across these pages, is to explicitly
  reset `margin: 0` on grid children. If a grid looks subtly misaligned in the
  rendered page but correct in isolation, this is almost certainly why.
- **Build command.** Build with
  `RUSTFLAGS="-L /lib64" pnpm build` from `locald-docs/`. The `RUSTFLAGS` prefix
  avoids an `-lz` linker failure in this environment. A clean build currently
  produces ~84 pages; if your count drops, you probably broke a page's
  frontmatter or an import.
- **Component layout vs. content margins.** Several components (noun grids,
  foundations panels, grammar bands) stack a label over a demo specifically to
  avoid overflow. If you "simplify" them back to side-by-side, re-check the
  narrow viewport — overflow was the reason for the stacked layout.

## How to verify your changes

1. From `locald-docs/`, run `RUSTFLAGS="-L /lib64" pnpm build` and confirm the
   page count holds.
2. Open the affected page and compare it against its poster — the actual image,
   not your memory of it.
3. Check a narrow viewport for overflow and the `* + *` margin artifact.

## Where to go next

- PR #104 is open against `main` and is self-contained. It can be reviewed and
  merged independently of the tray/quiet-up work.
- If you're adding a new style-guide surface, follow the existing pattern:
  compose a page in `.mdx` from components, push layout into
  `locald-components.css`, and pull color/type from the tokens — re-viewing the
  source poster as you go.
