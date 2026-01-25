---
title: "Documentation: Sticky Language Tabs"
stage: 3
feature: Documentation
---

# RFC: Documentation: Sticky Language Tabs

## 1. Summary

Use Astro Starlight's `<Tabs syncKey="lang">` component to persist language selection.

## 2. Motivation

Users typically care about one language (e.g., Node.js). Switching tabs on every code block is annoying.

## 3. Detailed Design

Wrap code blocks in `<Tabs>`. Use `syncKey` to link them.

### Terminology

- **Sticky Tabs**: Tabs that remember their selection.

### User Experience (UX)

Select "Node.js" once, and all examples show Node.js.

### Architecture

Docs site component.

### Implementation Details

Starlight feature.

## 4. Drawbacks

None.

## 5. Alternatives

- Separate pages for each language.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
