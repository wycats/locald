# Usage Session Capture

> A lightweight template for capturing what it felt like to use locald during a real work session.

## How to Use

1. **Start a session**: Note the time and what you're trying to accomplish
2. **During**: Voice memo or quick notes when something feels off OR surprisingly good
3. **After**: 5-minute brain dump using the prompts below
4. **Submit**: Drop the file in `docs/research/sessions/` or just paste into chat

---

## Session Template

### Meta

- **Date**:
- **Duration**: (rough)
- **Goal**: What were you trying to accomplish?
- **Project(s)**: Which project(s) were you working on?

### The Happy Path

What worked smoothly? What felt invisible/effortless?

### Friction Points

Where did you get stuck, annoyed, or have to think about locald instead of your work?

For each:

- What happened?
- What did you expect?
- What did you have to do instead?

### Concept Confusions

Were there any moments where the mental model didn't match reality? Where you thought something would work one way but it didn't?

### Wishlist Moments

Did you catch yourself wishing for something that doesn't exist?

### Quotes

Any specific thoughts you remember having? (Even if they seem trivial—"ugh, why is this..." is valuable data)

---

## Example Session

### Meta

- **Date**: 2026-01-23
- **Duration**: ~2 hours
- **Goal**: Adding OAuth to shop-frontend
- **Project(s)**: shop-frontend, shop-backend

### The Happy Path

- Adding Redis via plugin was seamless—just added the TOML line, saved, it was there
- HTTPS worked without thinking about it
- Logs were right there when I needed to debug the callback

### Friction Points

1. **Couldn't find the HTTPS URL easily**
   - Expected: Obvious in the dashboard
   - Reality: Had to look at the terminal output
2. **Service restart was slow after changing the backend**
   - Expected: Sub-second
   - Reality: ~3 seconds, enough to break flow

### Concept Confusions

- Thought `depends_on` would also inject the dependency's URL into my env, but had to do that separately with `${services.api.url}`

### Wishlist Moments

- Wished I could click a service and immediately copy its URL
- Wanted a "restart this one service" button

### Quotes

- "Wait, which port is the backend on again?"
- "Why do I have to go to the terminal for this?"
