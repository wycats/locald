# Axiom 15: Heroku Terminology (When It Fits)

**When an established Heroku term precisely answers a front-door question, we adopt it rather than inventing new vocabulary.**

## Rationale

Heroku terms are widely learned and map cleanly onto the kinds of workflows `locald` wants to make legible (start/stop, logs, admin actions).

Using an existing term is not “copying Heroku”; it is naming discipline:

- **Lower cognitive load**: users can transfer a correct mental model.
- **Fewer invented synonyms**: prevents docs/UI/CLI drift.
- **Better abstraction boundaries**: forces us to teach the _concept_ before the mechanism.

## Implications

- Prefer **one canonical spelling** for front-door verbs/nouns.
- When a Heroku term conflicts with current implementation, treat that as a product decision (rename vs keep) rather than letting drift persist.
- Example: `run` (one-off “admin actions”) vs `exec` (attach for debugging) should stay distinct.
